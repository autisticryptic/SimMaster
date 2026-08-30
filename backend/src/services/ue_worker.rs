//! Per-UE worker process management (isolation architecture, Option B).
//!
//! Each UE gets its own `simadmin ue-worker` child process. The parent uses
//! `pre_exec` + `setns(CLONE_NEWNET)` so the child is born inside the UE
//! network namespace *before* it creates any socket. Every SIP REGISTER,
//! RTP/RTCP, IKE/ESP and DNS socket therefore belongs to that UE's network
//! stack, and two UEs can use identical IPs, gateways and P-CSCF addresses
//! without ever colliding.
//!
//! The control channel is a Unix socket using length-prefixed JSON frames.
//! The worker is deliberately small in this phase: it proves namespace
//! isolation (Hello + `ip` status), applies ordered net-config batches inside
//! the UE namespace, and creates sockets there on demand. The main process
//! keeps hardware access (bearer/QMI), configuration and the API, while the
//! IMS state machines hold fds whose kernel-side socket lives in the UE
//! namespace (fd passing via `SCM_RIGHTS`).
//!
//! Frame format (both directions):
//!
//! ```text
//! frame = [u32 LE payload_len][payload(JSON)]
//! ```
//!
//! Every frame is sent with a single `sendmsg`. A `SocketCreateResult` puts
//! the newly created fd in the same frame's `SCM_RIGHTS` cmsg. The receiving
//! side peeks the header until a full frame is available, then consumes
//! exactly one frame with `recvmsg` so fds are never detached from their
//! message. Non-Linux builds keep the pure JSON protocol for tests and always
//! answer socket creation with `Unsupported`.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

/// Registry of live workers keyed by the stable line id.  The registry is
/// intentionally independent from the VoWiFi module: data proxy, VoLTE and
/// future 5G access legs all need the same UE owner, while VoWiFi may choose
/// to enable its TUN/socket path separately.
#[derive(Debug, Clone, Copy, Default)]
pub struct UeWorkerFeatures {
    /// Shared LTE/NR 3GPP IMS data plane. The worker does not care which radio
    /// access created the bearer; both use the same namespace/socket contract.
    pub three_gpp_ims: bool,
    pub data_proxy: bool,
    pub trunk_sockets: bool,
}

#[derive(Clone)]
struct RegisteredLineWorker {
    handle: UeWorkerHandle,
    features: UeWorkerFeatures,
}

static LINE_WORKERS: std::sync::OnceLock<StdMutex<HashMap<String, RegisteredLineWorker>>> =
    std::sync::OnceLock::new();

fn line_workers() -> &'static StdMutex<HashMap<String, RegisteredLineWorker>> {
    LINE_WORKERS.get_or_init(|| StdMutex::new(HashMap::new()))
}

pub fn register_line_worker(
    line_id: &str,
    worker: Option<UeWorkerHandle>,
    features: UeWorkerFeatures,
) {
    let mut workers = line_workers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match worker {
        Some(worker) => {
            workers.insert(
                line_id.to_string(),
                RegisteredLineWorker {
                    handle: worker,
                    features,
                },
            );
        }
        None => {
            workers.remove(line_id);
        }
    }
}

pub fn worker_for_line(line_id: &str) -> Option<UeWorkerHandle> {
    line_workers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(line_id)
        .map(|worker| worker.handle.clone())
}

pub fn worker_for_line_feature(
    line_id: &str,
    feature: fn(UeWorkerFeatures) -> bool,
) -> Option<UeWorkerHandle> {
    line_workers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(line_id)
        .filter(|worker| feature(worker.features))
        .map(|worker| worker.handle.clone())
}

#[cfg(not(unix))]
use crate::platform::netns::NetnsName;
#[cfg(unix)]
use crate::platform::netns::{self, NetnsName};

/// How long the parent waits for the worker handshake after spawning.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `shutdown` waits for a graceful worker exit before killing it.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
/// Worker-side connect retry budget (the exec path can take a moment).
const CONNECT_ATTEMPTS: usize = 25;
const CONNECT_DELAY: Duration = Duration::from_millis(200);

/// Environment variables consumed by the hidden `ue-worker` subcommand.
pub const ENV_LINE_ID: &str = "SIMADMIN_UE_LINE_ID";
pub const ENV_NETNS: &str = "SIMADMIN_UE_NETNS";
pub const ENV_CONTROL: &str = "SIMADMIN_UE_CONTROL";

/// How long a parent `apply_net_config` call waits for the worker's result.
pub const NET_CONFIG_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a parent `create_socket` call waits for the worker's fd.
pub const SOCKET_CREATE_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the parent blocking reader waits for the next control frame.
const CONTROL_READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Maximum accepted control-frame payload (16 MiB; real frames are < 64 KiB).
const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;
/// Maximum number of SCM_RIGHTS fds attached to one control frame.
const MAX_SOCKET_FDS: usize = 4;

/// `recvmsg` flags used when collecting SCM_RIGHTS fds.
///
/// The parent keeps IMS/RTP/proxy sockets alive for the life of a registration
/// while spawning new `ue-worker` children across restarts. Without
/// close-on-exec every replacement worker inherits those descriptors, so the
/// parent closing a socket no longer releases it and a retired registration can
/// stay ESTAB behind a worker that never created it.
#[cfg(all(unix, target_os = "linux"))]
const RECV_FD_FLAGS: libc::c_int = libc::MSG_CMSG_CLOEXEC;
#[cfg(all(unix, not(target_os = "linux")))]
const RECV_FD_FLAGS: libc::c_int = 0;

/// A single ordered network operation executed by the worker *inside its own
/// UE network namespace*. The worker is already `setns`-ed, so every `ip`
/// command here applies to the UE namespace only and cannot leak into another
/// line's stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum NetConfigOp {
    LinkSetUp {
        ifname: String,
    },
    LinkSetDown {
        ifname: String,
    },
    LinkSetMtu {
        ifname: String,
        mtu: u32,
    },
    /// `ip address replace <cidr> dev <ifname>` — idempotent.
    AddrReplace {
        ifname: String,
        cidr: String,
    },
    /// Best-effort `ip address del`; a missing address is not an error.
    AddrDel {
        ifname: String,
        cidr: String,
    },
    /// `ip route replace <target> via <via> dev <dev> src <src> table <t>`.
    RouteReplace {
        target: String,
        via: Option<String>,
        dev: Option<String>,
        src: Option<String>,
        table: Option<u32>,
    },
    /// Best-effort `ip route del`; a missing route is not an error.
    RouteDel {
        target: String,
        via: Option<String>,
        dev: Option<String>,
        src: Option<String>,
        table: Option<u32>,
    },
    DefaultRouteReplace {
        via: String,
        dev: String,
    },
    /// Point-to-point WWAN default route without an explicit next hop.
    DefaultRouteDeviceReplace {
        dev: String,
        ipv6: bool,
        metric: u32,
    },
    /// `ip route flush table <t>`; omitting the table flushes `table main`.
    FlushRoutes {
        table: Option<u32>,
    },
    /// Best-effort removal of routes owned by one interface only. Unlike a
    /// main-table flush this preserves the UE veth/VoWiFi paths.
    FlushRoutesForDevice {
        ifname: String,
        ipv6: bool,
    },
    /// Execute one validated `ip xfrm ...` argv inside this worker's namespace.
    /// The command is deliberately limited to the `xfrm` family so the parent
    /// cannot turn the worker control channel into an arbitrary shell.
    Xfrm {
        args: Vec<String>,
        best_effort: bool,
    },
}

/// The correlated outcome of a worker-side net-config batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NetConfigOutcome {
    pub request_id: u64,
    pub ok: bool,
    /// stdout of each successful op, in order.
    pub output: Vec<String>,
    pub error: Option<String>,
}

/// Socket kind the worker should create inside the UE namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UeSocketKind {
    Udp,
    Tcp,
}

/// Address family for the worker-created socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UeSocketFamily {
    Ipv4,
    Ipv6,
}

/// Request to create and initialize one socket *inside the UE namespace*.
///
/// The worker applies `SO_BINDTODEVICE` before `bind`, so the local address
/// is always resolved on the requested UE interface. UDP `connect` uses
/// `connect(2)` (which also pins the local source address); TCP uses
/// `connect_timeout(2)` bounded by `connect_timeout_secs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UeSocketSpec {
    pub kind: UeSocketKind,
    pub family: UeSocketFamily,
    /// Local bind address (`None` lets the kernel pick).
    pub bind: Option<SocketAddr>,
    /// Remote endpoint to connect the socket to inside the UE namespace.
    pub connect: Option<SocketAddr>,
    /// Interface name the socket must use inside the UE namespace, e.g.
    /// `save<hex>` for IKE or `sa_vwf<hex>` for SIP/RTP.
    pub bind_to_device: Option<String>,
    pub reuse_address: bool,
    /// TCP connect timeout in seconds; default 10s when `None`.
    pub connect_timeout_secs: Option<u64>,
}

impl UeSocketSpec {
    pub fn udp_bound(local: SocketAddr, bind_to_device: Option<String>) -> Self {
        Self {
            kind: UeSocketKind::Udp,
            family: socket_family(local),
            bind: Some(local),
            connect: None,
            bind_to_device,
            reuse_address: true,
            connect_timeout_secs: None,
        }
    }

    pub fn udp_connected(
        local: SocketAddr,
        remote: SocketAddr,
        bind_to_device: Option<String>,
    ) -> Self {
        let mut spec = Self::udp_bound(local, bind_to_device);
        spec.connect = Some(remote);
        spec
    }

    pub fn tcp_connected(
        local: SocketAddr,
        remote: SocketAddr,
        bind_to_device: Option<String>,
        connect_timeout_secs: u64,
    ) -> Self {
        Self {
            kind: UeSocketKind::Tcp,
            family: socket_family(local),
            bind: Some(local),
            connect: Some(remote),
            bind_to_device,
            reuse_address: true,
            connect_timeout_secs: Some(connect_timeout_secs),
        }
    }
}

fn socket_family(addr: SocketAddr) -> UeSocketFamily {
    match addr {
        SocketAddr::V4(_) => UeSocketFamily::Ipv4,
        SocketAddr::V6(_) => UeSocketFamily::Ipv6,
    }
}

/// A socket created in the UE namespace and handed to the main process.
#[derive(Debug)]
pub enum UeSocket {
    Udp(tokio::net::UdpSocket),
    Tcp(tokio::net::TcpStream),
}

/// Platform-neutral fd carrier used by the parent-side pending map.
#[cfg(unix)]
pub type SocketFd = std::os::fd::OwnedFd;
/// Non-Unix placeholder (socket creation is always `Unsupported` there).
#[cfg(not(unix))]
pub type SocketFd = ();

/// Result of the worker's socket factory, resolved by request id.
#[derive(Debug)]
pub struct SocketCreateOutcome {
    pub request_id: u64,
    pub ok: bool,
    pub error: Option<String>,
    pub fd: Option<SocketFd>,
}

/// Control-protocol messages, framed as length-prefixed JSON over a Unix
/// socket. `SocketCreateResult` carries no fd in JSON; the fd travels in the
/// same frame's `SCM_RIGHTS` cmsg.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UeWorkerMessage {
    /// Worker → parent, sent immediately after connect.
    Hello {
        line_id: String,
        netns: String,
        pid: u32,
    },
    /// Parent → worker: report the network status visible inside the UE
    /// namespace.
    NetStatusRequest,
    /// Worker → parent: interfaces/addresses/routes *inside the UE netns*.
    NetStatus {
        interfaces: Vec<String>,
        addresses: Vec<String>,
        default_routes: Vec<String>,
    },
    /// Parent → worker: apply a batch of ordered net-config operations in the
    /// UE namespace. Correlated by `request_id`; the worker always answers
    /// with `NetConfigResult`.
    NetConfigRequest {
        request_id: u64,
        ops: Vec<NetConfigOp>,
    },
    /// Worker → parent: outcome of a `NetConfigRequest`.
    NetConfigResult {
        outcome: NetConfigOutcome,
    },
    /// Parent → worker: create a socket inside the UE namespace. Correlated
    /// by `request_id`; the worker answers with `SocketCreateResult` plus the
    /// fd in `SCM_RIGHTS` when successful.
    SocketCreateRequest {
        request_id: u64,
        spec: UeSocketSpec,
    },
    /// Worker → parent: outcome of a `SocketCreateRequest` (fd in cmsg).
    SocketCreateResult {
        request_id: u64,
        ok: bool,
        error: Option<String>,
    },
    /// Parent → worker / worker → parent liveness probe.
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
    /// Parent → worker: graceful exit.
    Shutdown {
        reason: String,
    },
}

/// A snapshot of what the UE worker currently sees in its namespace.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NetStatusSnapshot {
    pub interfaces: Vec<String>,
    pub addresses: Vec<String>,
    pub default_routes: Vec<String>,
}

/// Runtime status of one UE worker, exposed to diagnostics and the API.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct UeWorkerStatus {
    pub line_id: String,
    pub netns: String,
    pub control_socket: String,
    pub pid: Option<u32>,
    pub ready: bool,
    pub connected_at: Option<String>,
    pub last_message_at: Option<String>,
    pub last_net_status: Option<NetStatusSnapshot>,
    /// True after the last `apply_net_config` batch succeeded in the UE
    /// namespace; the field tracks the most recent attempt.
    pub last_net_config_ok: bool,
    /// Error of the most recent net-config attempt, if any.
    pub last_net_config_error: Option<String>,
    /// True after the last worker socket creation succeeded.
    pub last_socket_ok: bool,
    /// Error of the most recent worker socket creation, if any.
    pub last_socket_error: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug)]
pub enum UeWorkerError {
    Unsupported,
    Io(std::io::Error),
    NamespaceMissing(String),
    HandshakeTimeout,
    Protocol(String),
}

impl std::fmt::Display for UeWorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => write!(f, "per-UE workers require Linux"),
            Self::Io(error) => write!(f, "{error}"),
            Self::NamespaceMissing(name) => write!(f, "network namespace {name} does not exist"),
            Self::HandshakeTimeout => write!(f, "UE worker handshake timed out"),
            Self::Protocol(detail) => write!(f, "UE worker protocol error: {detail}"),
        }
    }
}

impl std::error::Error for UeWorkerError {}

impl From<std::io::Error> for UeWorkerError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// Pending request/response correlation entries on the parent side.
enum PendingRequest {
    NetConfig(oneshot::Sender<NetConfigOutcome>),
    Socket(oneshot::Sender<SocketCreateOutcome>),
}

struct WorkerCore {
    line_id: String,
    namespace: NetnsName,
    control_path: PathBuf,
    /// Serializes spawn, shutdown and failure cleanup for one worker
    /// generation.  The blocking control reader never holds this lock; it
    /// schedules an async, PID-checked cleanup instead.
    lifecycle: tokio::sync::Mutex<()>,
    child: tokio::sync::Mutex<Option<tokio::process::Child>>,
    tx: StdMutex<Option<mpsc::UnboundedSender<UeWorkerMessage>>>,
    pending: StdMutex<HashMap<u64, PendingRequest>>,
    request_seq: AtomicU64,
    /// Incremented on every successful spawn. Starts at 0, so a handle that
    /// has never spawned a process is distinguishable from the first
    /// generation. See [`UeWorkerBinding`] for why a counter is required.
    generation: AtomicU64,
    state: StdMutex<UeWorkerStatus>,
}

/// Cloneable manager handle for one UE worker. One handle per line is owned by
/// [`LineRuntimeRegistry`]; the worker process itself is a single child.
#[derive(Clone)]
pub struct UeWorkerHandle {
    core: Arc<WorkerCore>,
}

/// A worker handle captured together with the process generation that was live
/// at capture time.
///
/// The handle alone is not enough to detect a restart: it is created once per
/// line and reused for every respawn, so two clones always share one core. A
/// runtime that moved an interface into the namespace, or that binds outbound
/// sockets through the worker, must be able to tell the process it bound to
/// from a replacement that never received that configuration.
#[derive(Clone)]
pub struct UeWorkerBinding {
    worker: UeWorkerHandle,
    generation: u64,
}

impl UeWorkerBinding {
    pub fn worker(&self) -> &UeWorkerHandle {
        &self.worker
    }

    pub fn namespace(&self) -> &NetnsName {
        self.worker.namespace()
    }

    /// True when the bound process is still the one the handle owns now.
    pub fn is_current(&self) -> bool {
        self.worker.generation() == self.generation
    }

    /// True when both bindings name the same line worker *and* the same
    /// process generation.
    pub fn matches(&self, other: &Self) -> bool {
        self.worker.same_instance(&other.worker) && self.generation == other.generation
    }
}

impl UeWorkerHandle {
    /// Build a handle for a line. The control socket lives in the temp
    /// directory and is namespaced by the parent pid + line id so two SimAdmin
    /// instances never fight over the same socket.
    pub fn for_line(line_id: &str, namespace: NetnsName) -> Self {
        let control_path =
            std::env::temp_dir().join(format!("simadmin-ue-{}-{line_id}.sock", std::process::id()));
        let control_socket = control_path.display().to_string();
        Self {
            core: Arc::new(WorkerCore {
                line_id: line_id.to_string(),
                namespace,
                control_path,
                lifecycle: tokio::sync::Mutex::new(()),
                child: tokio::sync::Mutex::new(None),
                tx: StdMutex::new(None),
                pending: StdMutex::new(HashMap::new()),
                request_seq: AtomicU64::new(1),
                generation: AtomicU64::new(0),
                state: StdMutex::new(UeWorkerStatus {
                    line_id: line_id.to_string(),
                    control_socket,
                    ..UeWorkerStatus::default()
                }),
            }),
        }
    }

    pub fn line_id(&self) -> &str {
        &self.core.line_id
    }

    pub fn namespace(&self) -> &NetnsName {
        &self.core.namespace
    }

    /// Whether two handles refer to the same worker lifecycle/connection.
    ///
    /// A line refresh can replace the registry entry with a new handle while
    /// an access runtime still owns the old one. Comparing the shared core
    /// lets those runtimes reject a stale namespace instead of silently
    /// sending traffic through a worker generation no longer owned by the
    /// line registry.
    ///
    /// This only compares the *manager*, not the worker process: the same core
    /// survives crashes and respawns. Runtimes that bind sockets or interfaces
    /// to a specific process must capture a [`UeWorkerBinding`] instead.
    pub fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }

    /// Generation of the worker process currently owned by this handle.
    /// `0` means no process has been spawned yet.
    pub fn generation(&self) -> u64 {
        self.core.generation.load(Ordering::SeqCst)
    }

    /// Capture this handle together with the generation running right now.
    pub fn bind(&self) -> UeWorkerBinding {
        UeWorkerBinding {
            generation: self.generation(),
            worker: self.clone(),
        }
    }

    pub async fn status(&self) -> UeWorkerStatus {
        self.core.state.lock().unwrap().clone()
    }

    /// Queue a message to the worker. Returns false when the control channel
    /// is not (yet) up.
    pub fn send(&self, message: UeWorkerMessage) -> bool {
        self.core
            .tx
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|tx| tx.send(message).is_ok())
    }

    /// Apply an ordered batch of network configuration operations inside the
    /// UE namespace. The worker executes them as a single correlated request;
    /// this call returns when the worker reports the outcome (or times out).
    pub async fn apply_net_config(
        &self,
        ops: Vec<NetConfigOp>,
    ) -> Result<NetConfigOutcome, UeWorkerError> {
        if ops.is_empty() {
            return Ok(NetConfigOutcome {
                request_id: 0,
                ok: true,
                output: Vec::new(),
                error: None,
            });
        }
        let request_id = self.core.request_seq.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel::<NetConfigOutcome>();
        {
            let mut guard = self.core.pending.lock().unwrap();
            guard.insert(request_id, PendingRequest::NetConfig(tx));
        }
        let sent = self.send(UeWorkerMessage::NetConfigRequest { request_id, ops });
        if !sent {
            let mut guard = self.core.pending.lock().unwrap();
            guard.remove(&request_id);
            return Err(UeWorkerError::Protocol(
                "worker control channel is not up".to_string(),
            ));
        }
        match tokio::time::timeout(NET_CONFIG_TIMEOUT, rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => Err(UeWorkerError::Protocol(
                "worker dropped the net-config request".to_string(),
            )),
            Err(_) => Err(UeWorkerError::Protocol(format!(
                "net-config request {request_id} timed out"
            ))),
        }
    }

    /// Ask the worker for an immediate namespace snapshot and wait briefly for
    /// the reader task to publish it. This is used after moving a native WWAN
    /// interface so feature gates never rely on a stale pre-migration view.
    pub async fn refresh_net_status(&self) -> Result<NetStatusSnapshot, UeWorkerError> {
        self.core.state.lock().unwrap().last_net_status = None;
        if !self.send(UeWorkerMessage::NetStatusRequest) {
            return Err(UeWorkerError::Protocol(
                "worker control channel is not up".to_string(),
            ));
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let status = self.status().await;
            if let Some(snapshot) = status.last_net_status {
                return Ok(snapshot);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(UeWorkerError::Protocol(
                    "worker net-status request timed out".to_string(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Create and initialize a socket inside the UE namespace and hand its fd
    /// back to this process. The returned socket is a normal tokio socket; its
    /// kernel state belongs to the UE stack exclusively.
    pub async fn create_socket(&self, spec: UeSocketSpec) -> Result<UeSocket, UeWorkerError> {
        #[cfg(unix)]
        {
            self.create_socket_unix(spec).await
        }
        #[cfg(not(unix))]
        {
            let _ = spec;
            Err(UeWorkerError::Unsupported)
        }
    }

    #[cfg(unix)]
    async fn create_socket_unix(&self, spec: UeSocketSpec) -> Result<UeSocket, UeWorkerError> {
        let request_id = self.core.request_seq.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel::<SocketCreateOutcome>();
        {
            let mut guard = self.core.pending.lock().unwrap();
            guard.insert(request_id, PendingRequest::Socket(tx));
        }
        let sent = self.send(UeWorkerMessage::SocketCreateRequest {
            request_id,
            spec: spec.clone(),
        });
        if !sent {
            let mut guard = self.core.pending.lock().unwrap();
            guard.remove(&request_id);
            return Err(UeWorkerError::Protocol(
                "worker control channel is not up".to_string(),
            ));
        }
        let outcome = match tokio::time::timeout(SOCKET_CREATE_TIMEOUT, rx).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => {
                return Err(UeWorkerError::Protocol(
                    "worker dropped the socket-create request".to_string(),
                ));
            }
            Err(_) => {
                self.core.pending.lock().unwrap().remove(&request_id);
                return Err(UeWorkerError::Protocol(format!(
                    "socket-create request {request_id} timed out"
                )));
            }
        };
        if !outcome.ok {
            return Err(UeWorkerError::Protocol(format!(
                "socket create failed: {}",
                outcome.error.as_deref().unwrap_or("unknown worker error")
            )));
        }
        let fd = outcome.fd.ok_or_else(|| {
            UeWorkerError::Protocol("socket create ok but fd missing".to_string())
        })?;
        // `tokio::net::*::from_std` REQUIRES a non-blocking fd: it registers the
        // fd with the reactor but never sets the flag itself. These fds arrive
        // over SCM_RIGHTS from the worker, and O_NONBLOCK lives on the shared
        // open file description, so whatever mode the worker left is what we get.
        // Assert it here rather than trusting the sender: a blocking fd parks a
        // tokio *worker thread* inside recvfrom() instead of parking the task,
        // and once every core worker is parked the reactor stops being driven --
        // unrelated work wedges, including the HTTP API accept loop, which then
        // leaves connections sitting completed-but-unaccepted in the kernel
        // backlog. That failure looks nothing like its cause, so pin it down here.
        match spec.kind {
            UeSocketKind::Udp => {
                let std_socket = std::net::UdpSocket::from(fd);
                std_socket.set_nonblocking(true)?;
                Ok(UeSocket::Udp(tokio::net::UdpSocket::from_std(std_socket)?))
            }
            UeSocketKind::Tcp => {
                let std_stream = std::net::TcpStream::from(fd);
                std_stream.set_nonblocking(true)?;
                Ok(UeSocket::Tcp(tokio::net::TcpStream::from_std(std_stream)?))
            }
        }
    }

    /// Wait until the worker process has connected and sent `Hello`.
    pub async fn wait_ready(&self, timeout: Duration) -> Result<(), UeWorkerError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let status = self.status().await;
            if status.ready {
                return Ok(());
            }
            if status.last_error.is_some() {
                return Err(UeWorkerError::Protocol(
                    "worker failed before readiness".to_string(),
                ));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(UeWorkerError::HandshakeTimeout);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Spawn the worker process and start the background accept/handshake.
    /// Returns after the process is created; readiness is reported
    /// asynchronously once the worker connects and sends `Hello`.
    pub async fn spawn(&self) -> Result<(), UeWorkerError> {
        #[cfg(unix)]
        {
            self.spawn_unix().await
        }
        #[cfg(not(unix))]
        {
            Err(UeWorkerError::Unsupported)
        }
    }

    /// Stop the worker gracefully (time-boxed), then remove its socket.
    pub async fn shutdown(&self) -> Result<(), UeWorkerError> {
        #[cfg(unix)]
        {
            self.shutdown_unix().await
        }
        #[cfg(not(unix))]
        {
            Ok(())
        }
    }

    /// True when the process is alive and the control channel is up.
    pub async fn is_running(&self) -> bool {
        let mut status = self.status().await;
        #[cfg(unix)]
        if !status.ready && status.pid.is_some() && status.last_error.is_some() {
            self.reap_failed_generation(&status).await;
            status = self.status().await;
        }
        status.ready || status.pid.is_some()
    }

    /// Retire a worker whose control channel has already failed.  The line
    /// registry calls this before deciding whether to spawn a replacement, so
    /// the observable `pid` cannot remain stuck while asynchronous cleanup is
    /// still queued behind a busy runtime.
    #[cfg(unix)]
    async fn reap_failed_generation(&self, status: &UeWorkerStatus) {
        if status.ready {
            return;
        }
        let Some(pid) = status.pid else {
            return;
        };
        let Some(reason) = status.last_error.clone() else {
            return;
        };
        let _ = self.core.fail_generation(pid, reason).await;
    }

    #[cfg(unix)]
    async fn spawn_unix(&self) -> Result<(), UeWorkerError> {
        use tokio::net::UnixListener;
        use tokio::process::Command;

        let _lifecycle = self.core.lifecycle.lock().await;
        let status = self.status().await;
        if status.ready || status.pid.is_some() {
            return Ok(());
        }
        if !netns::exists(&self.core.namespace) {
            return Err(UeWorkerError::NamespaceMissing(
                self.core.namespace.to_string(),
            ));
        }
        let _ = tokio::fs::remove_file(&self.core.control_path).await;
        let listener = UnixListener::bind(&self.core.control_path)?;
        let exe = std::env::current_exe()?;

        let mut command = Command::new(exe);
        command
            .arg("ue-worker")
            .env(ENV_LINE_ID, &self.core.line_id)
            .env(ENV_NETNS, self.core.namespace.as_str())
            .env(ENV_CONTROL, &self.core.control_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
        let enter = netns::setns_pre_exec(&self.core.namespace);
        // SAFETY: the closure only performs open/setns/close in the fork child
        // before exec; it is single-threaded and async-signal-safe.
        unsafe {
            command.pre_exec(enter);
        }
        let mut child = command.spawn()?;
        let Some(pid) = child.id() else {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(UeWorkerError::Protocol(
                "spawned UE worker did not expose a pid".to_string(),
            ));
        };
        {
            let mut guard = self.core.child.lock().await;
            *guard = Some(child);
        }
        // Publish the new generation before the status update so any consumer
        // that observes a live pid also observes the generation owning it.
        self.core.generation.fetch_add(1, Ordering::SeqCst);
        {
            let mut state = self.core.state.lock().unwrap();
            state.pid = Some(pid);
            state.ready = false;
            state.connected_at = None;
            state.last_error = None;
            state.last_net_status = None;
            state.last_socket_ok = false;
            state.last_socket_error = None;
        }

        let core = Arc::clone(&self.core);
        tokio::spawn(async move {
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, listener.accept()).await {
                Ok(Ok((stream, _))) => {
                    // Tokio's UnixStream no longer exposes try_clone, so the
                    // std stream is cloned first: one fd backs the blocking
                    // recvmsg reader, the original is re-wrapped for the
                    // async writer. into_std() keeps the socket nonblocking,
                    // which tokio::from_std requires.
                    let std_stream = match stream.into_std() {
                        Ok(std_stream) => std_stream,
                        Err(error) => {
                            let reason = format!("control stream conversion failed: {error}");
                            let _ = core.fail_generation(pid, reason).await;
                            return;
                        }
                    };
                    let read_stream = match std_stream.try_clone() {
                        Ok(stream) => stream,
                        Err(error) => {
                            let reason = format!("control stream clone failed: {error}");
                            let _ = core.fail_generation(pid, reason).await;
                            return;
                        }
                    };
                    let write_stream = match tokio::net::UnixStream::from_std(std_stream) {
                        Ok(stream) => stream,
                        Err(error) => {
                            let reason = format!("control write stream conversion failed: {error}");
                            let _ = core.fail_generation(pid, reason).await;
                            return;
                        }
                    };
                    // Shutdown or a previous cleanup may have retired this
                    // process while stream conversion was in progress.  Do
                    // not publish its writer over a replacement generation.
                    let (_read_half, write_half) = write_stream.into_split();
                    let (tx, rx) = mpsc::unbounded_channel::<UeWorkerMessage>();
                    {
                        // Keep the lifecycle lock only around the generation
                        // check and writer publication.  The reader owns the
                        // shared core after this point and must not inherit
                        // the lock guard into its blocking task.
                        let _lifecycle = core.lifecycle.lock().await;
                        if !core.generation_is_current(pid) {
                            return;
                        }
                        let mut guard = core.tx.lock().unwrap();
                        *guard = Some(tx);
                    }
                    tokio::spawn(writer_loop(write_half, rx));
                    let runtime = tokio::runtime::Handle::current();
                    tokio::task::spawn_blocking(move || {
                        run_parent_reader(read_stream, core, runtime, pid)
                    });
                }
                Ok(Err(error)) => {
                    let reason = format!("accept failed: {error}");
                    let _ = core.fail_generation(pid, reason).await;
                }
                Err(_) => {
                    let _ = core
                        .fail_generation(pid, "handshake timeout".to_string())
                        .await;
                }
            }
        });
        Ok(())
    }

    #[cfg(unix)]
    async fn shutdown_unix(&self) -> Result<(), UeWorkerError> {
        let _lifecycle = self.core.lifecycle.lock().await;
        self.send(UeWorkerMessage::Shutdown {
            reason: "manager_shutdown".to_string(),
        });
        let mut guard = self.core.child.lock().await;
        if let Some(child) = guard.as_mut() {
            match tokio::time::timeout(SHUTDOWN_TIMEOUT, child.wait()).await {
                Ok(_) => {}
                Err(_) => {
                    let _ = child.start_kill();
                    let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, child.wait()).await;
                }
            }
        }
        *guard = None;
        *self.core.tx.lock().unwrap() = None;
        self.core.pending.lock().unwrap().clear();
        let _ = tokio::fs::remove_file(&self.core.control_path).await;
        let mut state = self.core.state.lock().unwrap();
        state.ready = false;
        state.pid = None;
        state.connected_at = None;
        state.last_message_at = None;
        state.last_net_status = None;
        state.last_net_config_ok = false;
        state.last_net_config_error = None;
        state.last_socket_ok = false;
        state.last_socket_error = None;
        state.last_error = None;
        Ok(())
    }
}

impl WorkerCore {
    fn generation_is_current(&self, expected_pid: u32) -> bool {
        worker_generation_matches(
            self.state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pid,
            expected_pid,
        )
    }

    /// Retire exactly one failed worker generation.  PID matching is vital:
    /// a delayed handshake task or an old blocking reader may finish after a
    /// replacement worker is already live, and must never clear or kill it.
    async fn fail_generation(&self, expected_pid: u32, reason: String) -> std::io::Result<bool> {
        let _lifecycle = self.lifecycle.lock().await;
        if !self.generation_is_current(expected_pid) {
            return Ok(false);
        }
        let mut guard = self.child.lock().await;
        let current_pid = guard.as_ref().and_then(tokio::process::Child::id);
        if current_pid.is_some() && current_pid != Some(expected_pid) {
            return Ok(false);
        }

        *self
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        fail_pending_requests(self, &reason);
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.ready = false;
            state.last_error = Some(reason);
        }
        if let Some(child) = guard.as_mut() {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, child.wait()).await;
        }
        *guard = None;
        let _ = tokio::fs::remove_file(&self.control_path).await;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // The lifecycle lock prevents a replacement spawn until all old
        // channel/socket cleanup is complete.
        if state.pid == Some(expected_pid) {
            state.pid = None;
            state.connected_at = None;
            state.last_net_status = None;
            state.last_net_config_ok = false;
            state.last_net_config_error = None;
            state.last_socket_ok = false;
            state.last_socket_error = None;
        }
        Ok(true)
    }
}

fn worker_generation_matches(current_pid: Option<u32>, expected_pid: u32) -> bool {
    current_pid == Some(expected_pid)
}

fn mark_worker_generation_failed(
    state: &mut UeWorkerStatus,
    expected_pid: u32,
    reason: String,
) -> bool {
    if !worker_generation_matches(state.pid, expected_pid) {
        return false;
    }
    state.ready = false;
    state.last_error = Some(reason);
    true
}

/// Parent-side reader. Runs on a blocking thread because it needs
/// `recvmsg` with `MSG_PEEK` plus `SCM_RIGHTS` support, which tokio's
/// `AsyncReadExt` cannot expose. Each frame is consumed with exactly one
/// `recvmsg` so fds never get detached from their message.
#[cfg(unix)]
fn run_parent_reader(
    stream: std::os::unix::net::UnixStream,
    core: Arc<WorkerCore>,
    runtime: tokio::runtime::Handle,
    expected_pid: u32,
) {
    let mut exit_reason = "worker_control_closed".to_string();
    loop {
        if !core.generation_is_current(expected_pid) {
            exit_reason = "worker generation superseded".to_string();
            break;
        }
        match recv_control_frame(&stream, CONTROL_READ_TIMEOUT) {
            Ok(Some((payload, fds))) => {
                let message = match serde_json::from_slice::<UeWorkerMessage>(&payload) {
                    Ok(message) => message,
                    Err(error) => {
                        exit_reason = format!("invalid control frame from worker: {error}");
                        drop(fds);
                        break;
                    }
                };
                if !core.generation_is_current(expected_pid) {
                    exit_reason = "worker generation superseded".to_string();
                    drop(fds);
                    break;
                }
                if matches!(
                    &message,
                    UeWorkerMessage::Hello { pid, .. } if *pid != expected_pid
                ) {
                    exit_reason = format!(
                        "worker hello pid mismatch: expected {expected_pid}, received {}",
                        match &message {
                            UeWorkerMessage::Hello { pid, .. } => *pid,
                            _ => unreachable!(),
                        }
                    );
                    drop(fds);
                    break;
                }
                handle_parent_message(&core, expected_pid, message, fds);
            }
            Ok(None) => break,
            // An otherwise healthy worker may legitimately be idle for much
            // longer than CONTROL_READ_TIMEOUT. Keep the channel alive and
            // probe liveness instead of treating an idle poll as EOF.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if !core.generation_is_current(expected_pid) {
                    exit_reason = "worker generation superseded".to_string();
                    break;
                }
                let nonce = core.request_seq.fetch_add(1, Ordering::Relaxed);
                let sent = core
                    .tx
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_ref()
                    .is_some_and(|tx| tx.send(UeWorkerMessage::Ping { nonce }).is_ok());
                if !sent {
                    exit_reason = "worker control writer unavailable".to_string();
                    break;
                }
                continue;
            }
            Err(error) => {
                exit_reason = format!("worker control read failed: {error}");
                break;
            }
        }
    }
    let current_generation = {
        let mut state = core
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        mark_worker_generation_failed(&mut state, expected_pid, exit_reason.clone())
    };
    if !current_generation {
        return;
    }

    // Never block a Tokio worker from this blocking reader.  The async cleanup
    // is generation-checked, so a late old reader cannot affect a replacement
    // process.  During a normal shutdown the child is already gone and this
    // becomes a no-op, preserving the clean (last_error = None) status.
    runtime.spawn(async move {
        if let Err(error) = core.fail_generation(expected_pid, exit_reason).await {
            tracing::debug!(
                expected_pid,
                error = %error,
                "Failed to retire disconnected UE worker generation"
            );
        }
    });
}

/// Resolve every in-flight request immediately when the worker channel dies.
/// Otherwise callers wait for the full request timeout even though recovery
/// can already start on the next line-registry refresh.
fn fail_pending_requests(core: &WorkerCore, reason: &str) {
    let pending = {
        let mut guard = core
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.drain().collect::<Vec<_>>()
    };
    for (request_id, request) in pending {
        match request {
            PendingRequest::NetConfig(sender) => {
                let _ = sender.send(NetConfigOutcome {
                    request_id,
                    ok: false,
                    output: Vec::new(),
                    error: Some(reason.to_string()),
                });
            }
            PendingRequest::Socket(sender) => {
                let _ = sender.send(SocketCreateOutcome {
                    request_id,
                    ok: false,
                    error: Some(reason.to_string()),
                    fd: None,
                });
            }
        }
    }
}

/// Dispatch a worker-side message on the parent reader thread.
#[cfg(unix)]
fn handle_parent_message(
    core: &WorkerCore,
    expected_pid: u32,
    message: UeWorkerMessage,
    fds: Vec<i32>,
) {
    use chrono::Utc;

    match message {
        UeWorkerMessage::Hello {
            line_id,
            netns,
            pid,
        } => {
            if line_id != core.line_id {
                tracing::warn!(
                    expected = %core.line_id,
                    received = %line_id,
                    "UE worker identified a different line; ignoring hello"
                );
                return;
            }
            let mut state = core
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !worker_generation_matches(state.pid, expected_pid) {
                return;
            }
            state.ready = true;
            state.netns = netns;
            state.connected_at = Some(Utc::now().to_rfc3339());
            state.last_message_at = Some(Utc::now().to_rfc3339());
            state.last_error = None;
            tracing::info!(line_id = %core.line_id, pid, "UE worker ready inside its namespace");
        }
        UeWorkerMessage::Pong { nonce } => {
            let mut state = core
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !worker_generation_matches(state.pid, expected_pid) {
                return;
            }
            state.last_message_at = Some(Utc::now().to_rfc3339());
            state.last_error = None;
            tracing::trace!(line_id = %core.line_id, nonce, "UE worker pong");
        }
        UeWorkerMessage::NetStatus {
            interfaces,
            addresses,
            default_routes,
        } => {
            let mut state = core
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !worker_generation_matches(state.pid, expected_pid) {
                return;
            }
            state.last_message_at = Some(Utc::now().to_rfc3339());
            state.last_net_status = Some(NetStatusSnapshot {
                interfaces,
                addresses,
                default_routes,
            });
            tracing::debug!(line_id = %core.line_id, "UE worker reported namespace status");
        }
        UeWorkerMessage::NetConfigResult { outcome } => {
            let request_id = outcome.request_id;
            let ok = outcome.ok;
            let error = outcome.error.clone();
            let sender = core.pending.lock().unwrap().remove(&request_id);
            let mut state = core
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !worker_generation_matches(state.pid, expected_pid) {
                drop(sender);
                return;
            }
            state.last_message_at = Some(Utc::now().to_rfc3339());
            state.last_net_config_ok = ok;
            state.last_net_config_error = error.clone();
            state.last_error = None;
            tracing::info!(
                line_id = %core.line_id,
                request_id,
                ok,
                error = error.as_deref().unwrap_or(""),
                "UE worker applied net-config batch"
            );
            if let Some(PendingRequest::NetConfig(sender)) = sender {
                let _ = sender.send(NetConfigOutcome {
                    request_id,
                    ok,
                    output: outcome.output,
                    error,
                });
            }
        }
        UeWorkerMessage::SocketCreateResult {
            request_id,
            ok,
            error,
        } => {
            use std::os::fd::FromRawFd;
            let mut owned_fds = fds
                .into_iter()
                .map(|fd| unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) })
                .collect::<Vec<_>>();
            let fd = if ok { owned_fds.pop() } else { None };
            let sender = core.pending.lock().unwrap().remove(&request_id);
            let mut state = core
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !worker_generation_matches(state.pid, expected_pid) {
                drop(sender);
                drop(fd);
                return;
            }
            state.last_message_at = Some(Utc::now().to_rfc3339());
            state.last_socket_ok = ok;
            state.last_socket_error = error.clone();
            state.last_error = None;
            match sender {
                Some(PendingRequest::Socket(sender)) => {
                    let _ = sender.send(SocketCreateOutcome {
                        request_id,
                        ok,
                        error,
                        fd,
                    });
                }
                _ => {
                    tracing::warn!(
                        line_id = %core.line_id,
                        request_id,
                        "UE worker socket result arrived after the parent gave up"
                    );
                    drop(fd);
                }
            }
        }
        other => {
            tracing::trace!(
                line_id = %core.line_id,
                protocol_message = ?other,
                "Unexpected parent-side control message"
            );
        }
    }
}

/// Parent writer: length-prefixed JSON frames to the worker. The parent never
/// sends fds, so a plain bounded write is sufficient.
#[cfg(unix)]
async fn writer_loop(
    mut stream: tokio::net::unix::OwnedWriteHalf,
    mut rx: mpsc::UnboundedReceiver<UeWorkerMessage>,
) {
    use tokio::io::AsyncWriteExt;

    while let Some(message) = rx.recv().await {
        let Ok(payload) = serde_json::to_vec(&message) else {
            continue;
        };
        let mut frame = Vec::with_capacity(4 + payload.len());
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(&payload);
        if stream.write_all(&frame).await.is_err() {
            break;
        }
    }
}

/// Entry point for the hidden `ue-worker` subcommand. Reads its parameters
/// from the environment (set by the parent before `exec`).
pub async fn run_worker_from_env() -> anyhow::Result<()> {
    let line_id = std::env::var(ENV_LINE_ID)
        .map_err(|_| anyhow::anyhow!("{ENV_LINE_ID} is required for ue-worker"))?;
    let netns_name =
        std::env::var(ENV_NETNS).map_err(|_| anyhow::anyhow!("{ENV_NETNS} is required"))?;
    let control =
        std::env::var(ENV_CONTROL).map_err(|_| anyhow::anyhow!("{ENV_CONTROL} is required"))?;
    run_worker(&line_id, &netns_name, Path::new(&control)).await
}

/// Run the worker loop. The process is already inside its UE namespace when
/// this is called (the parent entered it in `pre_exec`).
#[cfg(unix)]
pub async fn run_worker(line_id: &str, netns_name: &str, control: &Path) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;

    tracing::info!(line_id, netns = %netns_name, "UE worker starting inside its namespace");
    let stream = connect_with_retry(control)
        .await
        .map_err(|error| anyhow::anyhow!("UE worker connect failed: {error}"))?;
    // The worker only talks to the parent sequentially, so a blocking std
    // stream keeps sendmsg/timeout semantics simple; tokio adds nothing here.
    let mut stream = stream
        .into_std()
        .map_err(|error| anyhow::anyhow!("UE worker stream conversion failed: {error}"))?;
    stream
        .set_nonblocking(false)
        .map_err(|error| anyhow::anyhow!("UE worker stream blocking mode failed: {error}"))?;
    let write_stream = stream
        .try_clone()
        .map_err(|error| anyhow::anyhow!("UE worker write clone failed: {error}"))?;
    let _ = write_stream.set_write_timeout(Some(Duration::from_secs(10)));
    send_frame_std(
        &write_stream,
        &UeWorkerMessage::Hello {
            line_id: line_id.to_string(),
            netns: netns_name.to_string(),
            pid: std::process::id(),
        },
        &[],
    )?;

    loop {
        let Some(payload) = read_control_frame_std(&mut stream)? else {
            break;
        };
        let message = serde_json::from_slice::<UeWorkerMessage>(&payload)
            .map_err(|error| anyhow::anyhow!("invalid control frame: {error}"))?;
        match message {
            UeWorkerMessage::Ping { nonce } => {
                send_frame_std(&write_stream, &UeWorkerMessage::Pong { nonce }, &[])?;
            }
            UeWorkerMessage::NetStatusRequest => {
                let status = collect_net_status().await;
                send_frame_std(
                    &write_stream,
                    &UeWorkerMessage::NetStatus {
                        interfaces: status.interfaces,
                        addresses: status.addresses,
                        default_routes: status.default_routes,
                    },
                    &[],
                )?;
            }
            UeWorkerMessage::NetConfigRequest { request_id, ops } => {
                let (ok, output, error) = execute_net_config(ops).await;
                send_frame_std(
                    &write_stream,
                    &UeWorkerMessage::NetConfigResult {
                        outcome: NetConfigOutcome {
                            request_id,
                            ok,
                            output,
                            error,
                        },
                    },
                    &[],
                )?;
            }
            UeWorkerMessage::SocketCreateRequest { request_id, spec } => {
                match create_socket_fd(&spec) {
                    Ok(fd) => {
                        let raw = fd.as_raw_fd();
                        let result = send_frame_std(
                            &write_stream,
                            &UeWorkerMessage::SocketCreateResult {
                                request_id,
                                ok: true,
                                error: None,
                            },
                            &[raw],
                        );
                        // SCM_RIGHTS duplicates the fd for the receiver; our
                        // copy is closed here.
                        drop(fd);
                        result?;
                    }
                    Err(error) => {
                        send_frame_std(
                            &write_stream,
                            &UeWorkerMessage::SocketCreateResult {
                                request_id,
                                ok: false,
                                error: Some(error.to_string()),
                            },
                            &[],
                        )?;
                    }
                }
            }
            UeWorkerMessage::Shutdown { reason } => {
                tracing::info!(line_id, reason = %reason, "UE worker shutdown requested");
                break;
            }
            _ => {}
        }
    }
    tracing::info!(line_id, "UE worker exiting");
    Ok(())
}

#[cfg(not(unix))]
pub async fn run_worker(_line_id: &str, _netns_name: &str, _control: &Path) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("UE workers require Linux"))
}

#[cfg(unix)]
async fn connect_with_retry(path: &Path) -> std::io::Result<tokio::net::UnixStream> {
    use tokio::net::UnixStream;

    let mut last_error = None;
    for _ in 0..CONNECT_ATTEMPTS {
        match UnixStream::connect(path).await {
            Ok(stream) => return Ok(stream),
            Err(error) => {
                last_error = Some(error);
                tokio::time::sleep(CONNECT_DELAY).await;
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "control socket connect retries exhausted",
        )
    }))
}

/// Worker-side reader: length-prefixed frames. The worker never receives fds,
/// so plain blocking reads are safe here.
#[cfg(unix)]
fn read_control_frame_std(
    stream: &mut std::os::unix::net::UnixStream,
) -> anyhow::Result<Option<Vec<u8>>> {
    use std::io::Read;

    let mut header = [0u8; 4];
    match stream.read_exact(&mut header) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let len = u32::from_le_bytes(header) as usize;
    if len > MAX_FRAME_LEN {
        anyhow::bail!("control frame payload too large: {len}");
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload)?;
    Ok(Some(payload))
}

/// Serialize a message and send it with a single `sendmsg`, optionally
/// attaching fds in `SCM_RIGHTS`. Used by the worker (which owns fds).
#[cfg(unix)]
fn send_frame_std(
    stream: &std::os::unix::net::UnixStream,
    message: &UeWorkerMessage,
    fds: &[i32],
) -> anyhow::Result<()> {
    let payload = serde_json::to_vec(message)?;
    sendmsg_frame(stream, &payload, fds)?;
    Ok(())
}

/// Create a socket inside this process's (UE) namespace and initialize it per
/// the spec: SO_REUSEADDR, SO_BINDTODEVICE, bind, optional UDP connect or
/// TCP connect-with-timeout. The returned fd is owned by the caller.
#[cfg(unix)]
fn create_socket_fd(spec: &UeSocketSpec) -> std::io::Result<std::os::fd::OwnedFd> {
    let preferred_addr = spec.connect.or(spec.bind);
    let domain = match preferred_addr {
        Some(addr) => socket2::Domain::for_address(addr),
        None => match spec.family {
            UeSocketFamily::Ipv4 => socket2::Domain::IPV4,
            UeSocketFamily::Ipv6 => socket2::Domain::IPV6,
        },
    };
    let ty = match spec.kind {
        UeSocketKind::Udp => socket2::Type::DGRAM,
        UeSocketKind::Tcp => socket2::Type::STREAM,
    };
    let protocol = match spec.kind {
        UeSocketKind::Udp => socket2::Protocol::UDP,
        UeSocketKind::Tcp => socket2::Protocol::TCP,
    };
    let socket = socket2::Socket::new(domain, ty, Some(protocol))?;
    if spec.reuse_address {
        socket.set_reuse_address(true)?;
    }
    if let Some(device) = &spec.bind_to_device {
        set_bind_to_device(&socket, device)?;
    }
    if let Some(bind) = &spec.bind {
        socket.bind(&socket2::SockAddr::from(*bind))?;
    }
    match spec.kind {
        UeSocketKind::Udp => {
            if let Some(connect) = &spec.connect {
                socket.connect(&socket2::SockAddr::from(*connect))?;
            }
        }
        UeSocketKind::Tcp => {
            if let Some(connect) = &spec.connect {
                let timeout = spec
                    .connect_timeout_secs
                    .map(Duration::from_secs)
                    .unwrap_or(Duration::from_secs(10));
                socket.connect_timeout(&socket2::SockAddr::from(*connect), timeout)?;
            }
        }
    }
    // O_NONBLOCK must be set before this fd crosses SCM_RIGHTS: the flag lives on
    // the open file description, so the parent inherits whatever we leave here,
    // and the parent hands the fd straight to `tokio::net::*::from_std`, which
    // requires non-blocking. A blocking fd there blocks a tokio worker *thread*
    // in recvfrom() rather than parking the task.
    //
    // This must stay AFTER the connect above. `socket2::connect_timeout` toggles
    // non-blocking mode internally and restores the socket to *blocking* before
    // it returns, so setting the flag any earlier would be silently undone.
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

#[cfg(target_os = "linux")]
fn set_bind_to_device(socket: &socket2::Socket, device: &str) -> std::io::Result<()> {
    use std::{ffi::CString, os::fd::AsRawFd};

    let name = CString::new(device).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "interface contains NUL")
    })?;
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            name.as_ptr().cast(),
            name.as_bytes_with_nul().len() as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn set_bind_to_device(_socket: &socket2::Socket, _device: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "SO_BINDTODEVICE is Linux-only",
    ))
}

/// Encode a payload into a length-prefixed control frame.
fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Send one completed frame with a single `sendmsg`. When `fds` is non-empty,
/// the descriptors travel in the same frame's `SCM_RIGHTS` ancillary data.
#[cfg(unix)]
fn sendmsg_frame(
    stream: &std::os::unix::net::UnixStream,
    payload: &[u8],
    fds: &[i32],
) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;

    let frame = encode_frame(payload);
    let mut iov = [libc::iovec {
        iov_base: frame.as_ptr() as *mut libc::c_void,
        iov_len: frame.len(),
    }];
    let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
    header.msg_iov = iov.as_mut_ptr();
    header.msg_iovlen = 1;
    let mut cmsg_buf = vec![0u8; cmsg_space_for(fds.len())];
    if !fds.is_empty() {
        header.msg_control = cmsg_buf.as_mut_ptr().cast();
        header.msg_controllen = cmsg_buf
            .len()
            .try_into()
            .expect("control message buffer exceeds platform limit");
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&header);
            if cmsg.is_null() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "failed to allocate control message header",
                ));
            }
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len =
                libc::CMSG_LEN((fds.len() * std::mem::size_of::<libc::c_int>()) as libc::c_uint)
                    .try_into()
                    .expect("control message length exceeds platform limit");
            let data = libc::CMSG_DATA(cmsg) as *mut libc::c_int;
            std::ptr::copy_nonoverlapping(fds.as_ptr(), data, fds.len());
        }
    }
    let sent = unsafe { libc::sendmsg(stream.as_raw_fd(), &header, 0) };
    if sent < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if sent as usize != frame.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "partial control frame send",
        ));
    }
    Ok(())
}

/// Receive exactly one frame plus any `SCM_RIGHTS` fds attached to it.
///
/// The header is peeked first (without consuming), then the reader waits until
/// the complete frame is available and consumes it with one `recvmsg`. This
/// keeps ancillary fds attached to their frame even on a stream socket.
#[cfg(unix)]
fn recv_control_frame(
    stream: &std::os::unix::net::UnixStream,
    timeout: Duration,
) -> std::io::Result<Option<(Vec<u8>, Vec<i32>)>> {
    use std::os::unix::io::AsRawFd;

    let fd = stream.as_raw_fd();
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let available = wait_byte_count(fd, &mut pfd, 4, timeout)?;
    if available == 0 {
        return Ok(None);
    }

    // Peek the length header without consuming the frame.
    let mut header = [0u8; 4];
    let mut peek_iov = [libc::iovec {
        iov_base: header.as_mut_ptr().cast(),
        iov_len: header.len(),
    }];
    let mut peek_header: libc::msghdr = unsafe { std::mem::zeroed() };
    peek_header.msg_iov = peek_iov.as_mut_ptr();
    peek_header.msg_iovlen = 1;
    let peeked = unsafe { libc::recvmsg(fd, &mut peek_header, libc::MSG_PEEK) };
    if peeked < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if peeked == 0 {
        return Ok(None);
    }
    let payload_len = u32::from_le_bytes(header) as usize;
    if payload_len > MAX_FRAME_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("control frame payload too large: {payload_len}"),
        ));
    }
    let total = 4_usize.saturating_add(payload_len);
    let available = wait_byte_count(fd, &mut pfd, total, timeout)?;
    if available == 0 {
        return Ok(None);
    }

    let mut buf = vec![0u8; total];
    let mut iov = [libc::iovec {
        iov_base: buf.as_mut_ptr().cast(),
        iov_len: buf.len(),
    }];
    let mut header_msg: libc::msghdr = unsafe { std::mem::zeroed() };
    header_msg.msg_iov = iov.as_mut_ptr();
    header_msg.msg_iovlen = 1;
    let mut cmsg_buf = vec![0u8; cmsg_space_for(MAX_SOCKET_FDS)];
    header_msg.msg_control = cmsg_buf.as_mut_ptr().cast();
    header_msg.msg_controllen = cmsg_buf
        .len()
        .try_into()
        .expect("control message buffer exceeds platform limit");
    let received = unsafe { libc::recvmsg(fd, &mut header_msg, RECV_FD_FLAGS) };
    if received < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if received == 0 {
        return Ok(None);
    }
    if received as usize != total {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("partial control frame: got {received}/{total}"),
        ));
    }
    let fds = extract_scm_rights(&header_msg);
    Ok(Some((buf[4..].to_vec(), fds)))
}

/// Poll the fd until at least `min_bytes` are available, returning the
/// current FIONREAD byte count (0 means EOF).
#[cfg(unix)]
fn wait_byte_count(
    fd: libc::c_int,
    pfd: &mut libc::pollfd,
    min_bytes: usize,
    timeout: Duration,
) -> std::io::Result<usize> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_millis();
        if remaining == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "control frame read timed out",
            ));
        }
        let ready = unsafe { libc::poll(pfd, 1, remaining.min(i32::MAX as u128) as i32) };
        if ready < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if ready == 0 {
            // poll(2) timeout means "still idle", not EOF. Loop once more so
            // the deadline branch returns WouldBlock and the parent can send
            // a Ping without tearing down a healthy worker.
            continue;
        }
        let mut available: libc::c_int = 0;
        if unsafe { libc::ioctl(fd, libc::FIONREAD, &mut available) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let available = available.max(0) as usize;
        if available == 0 {
            return Ok(0);
        }
        if available >= min_bytes {
            return Ok(available);
        }
    }
}

#[cfg(unix)]
fn cmsg_space_for(count: usize) -> usize {
    let bytes = count * std::mem::size_of::<libc::c_int>();
    unsafe { libc::CMSG_SPACE(bytes as libc::c_uint) as usize }
}

/// Collect all `SCM_RIGHTS` fds from a received message header. Ownership of
/// the returned fds transfers to the caller.
#[cfg(unix)]
fn extract_scm_rights(header: &libc::msghdr) -> Vec<i32> {
    let mut fds = Vec::new();
    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(header) };
    while !cmsg.is_null() {
        unsafe {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let payload_len = (*cmsg)
                    .cmsg_len
                    .saturating_sub(libc::CMSG_LEN(0).try_into().expect("cmsg length overflow"));
                let count = payload_len as usize / std::mem::size_of::<libc::c_int>();
                if count > 0 {
                    let data = libc::CMSG_DATA(cmsg) as *const libc::c_int;
                    for index in 0..count {
                        fds.push(*data.add(index));
                    }
                }
            }
            cmsg = libc::CMSG_NXTHDR(header, cmsg);
        }
    }
    fds
}

/// Collect the network view *inside this process's* namespace. Used by the
/// worker to prove which interfaces/addresses belong to the UE.
#[cfg(unix)]
async fn collect_net_status() -> NetStatusSnapshot {
    use tokio::process::Command;

    let mut snapshot = NetStatusSnapshot::default();
    if let Ok(output) = Command::new("ip").args(["-json", "address"]).output().await {
        if output.status.success() {
            if let Ok(value) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout) {
                for iface in &value {
                    if let Some(name) = iface.get("ifname").and_then(|value| value.as_str()) {
                        snapshot.interfaces.push(name.to_string());
                    }
                    if let Some(infos) = iface.get("addr_info").and_then(|value| value.as_array()) {
                        for info in infos {
                            if let Some(local) = info.get("local").and_then(|value| value.as_str())
                            {
                                snapshot.addresses.push(local.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    if let Ok(output) = Command::new("ip")
        .args(["-json", "route", "show", "default"])
        .output()
        .await
    {
        if output.status.success() {
            if let Ok(value) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout) {
                for route in &value {
                    let via = route
                        .get("via")
                        .and_then(|value| value.as_str())
                        .unwrap_or("*");
                    let dev = route
                        .get("dev")
                        .and_then(|value| value.as_str())
                        .unwrap_or("*");
                    snapshot.default_routes.push(format!("via {via} dev {dev}"));
                }
            }
        }
    }
    snapshot
}

/// Execute a net-config batch. Runs inside the worker's own namespace, so the
/// commands target the UE stack exclusively. Each op captures its stdout; a
/// failed op aborts the batch and reports the first error.
#[cfg(unix)]
async fn execute_net_config(ops: Vec<NetConfigOp>) -> (bool, Vec<String>, Option<String>) {
    use tokio::process::Command;

    let mut output = Vec::with_capacity(ops.len());
    let ip_path = discover_ip().await.unwrap_or_else(|| "ip".to_string());
    for op in ops {
        let argv = match net_config_argv(&op) {
            Ok(argv) => argv,
            Err(error) => return (false, output, Some(error)),
        };
        let result = Command::new(&ip_path).args(&argv).output().await;
        match result {
            Ok(command_output) if command_output.status.success() => {
                output.push(
                    String::from_utf8_lossy(&command_output.stdout)
                        .trim()
                        .to_string(),
                );
            }
            Ok(command_output) => {
                let stderr = String::from_utf8_lossy(&command_output.stderr);
                let reason = if is_benign_net_config_error(&op, &stderr) {
                    output.push(format!("ignored: {}", stderr.trim()));
                    continue;
                } else {
                    stderr.trim().to_string()
                };
                return (
                    false,
                    output,
                    Some(format!("{} {}: {reason}", ip_path, argv.join(" "))),
                );
            }
            Err(error) => {
                return (
                    false,
                    output,
                    Some(format!("{} {}: {error}", ip_path, argv.join(" "))),
                );
            }
        }
    }
    (true, output, None)
}

#[cfg(not(unix))]
async fn execute_net_config(_ops: Vec<NetConfigOp>) -> (bool, Vec<String>, Option<String>) {
    (
        false,
        Vec::new(),
        Some("UE workers require Linux".to_string()),
    )
}

/// Build the `ip` argv for one op. Safe command construction: every argument
/// is a static token or a value serialized from the worker protocol, never a
/// shell string.
#[cfg(unix)]
fn net_config_argv(op: &NetConfigOp) -> Result<Vec<String>, String> {
    let argv = match op {
        NetConfigOp::LinkSetUp { ifname } => {
            vec!["link".into(), "set".into(), ifname.clone(), "up".into()]
        }
        NetConfigOp::LinkSetDown { ifname } => {
            vec!["link".into(), "set".into(), ifname.clone(), "down".into()]
        }
        NetConfigOp::LinkSetMtu { ifname, mtu } => vec![
            "link".into(),
            "set".into(),
            "dev".into(),
            ifname.clone(),
            "mtu".into(),
            mtu.to_string(),
        ],
        NetConfigOp::AddrReplace { ifname, cidr } => vec![
            "address".into(),
            "replace".into(),
            cidr.clone(),
            "dev".into(),
            ifname.clone(),
        ],
        NetConfigOp::AddrDel { ifname, cidr } => vec![
            "address".into(),
            "del".into(),
            cidr.clone(),
            "dev".into(),
            ifname.clone(),
        ],
        NetConfigOp::RouteReplace {
            target,
            via,
            dev,
            src,
            table,
        } => route_argv(
            "replace",
            target,
            via.as_deref(),
            dev.as_deref(),
            src.as_deref(),
            *table,
        ),
        NetConfigOp::RouteDel {
            target,
            via,
            dev,
            src,
            table,
        } => route_argv(
            "del",
            target,
            via.as_deref(),
            dev.as_deref(),
            src.as_deref(),
            *table,
        ),
        NetConfigOp::DefaultRouteReplace { via, dev } => vec![
            "route".into(),
            "replace".into(),
            "default".into(),
            "via".into(),
            via.as_str().into(),
            "dev".into(),
            dev.as_str().into(),
        ],
        NetConfigOp::DefaultRouteDeviceReplace { dev, ipv6, metric } => {
            let mut argv = Vec::with_capacity(8);
            if *ipv6 {
                argv.push("-6".into());
            }
            argv.extend([
                "route".into(),
                "replace".into(),
                "default".into(),
                "dev".into(),
                dev.clone(),
                "metric".into(),
                metric.to_string(),
            ]);
            argv
        }
        NetConfigOp::FlushRoutes { table } => {
            let table = table
                .map(|value| value.to_string())
                .unwrap_or_else(|| "main".to_string());
            vec!["route".into(), "flush".into(), "table".into(), table]
        }
        NetConfigOp::FlushRoutesForDevice { ifname, ipv6 } => {
            let mut argv = Vec::with_capacity(5);
            if *ipv6 {
                argv.push("-6".into());
            }
            argv.extend(["route".into(), "flush".into(), "dev".into(), ifname.clone()]);
            argv
        }
        NetConfigOp::Xfrm { args, best_effort } => {
            validate_xfrm_argv(args, *best_effort)?;
            args.clone()
        }
    };
    Ok(argv)
}

/// Keep the worker control channel scoped to the exact XFRM operations emitted
/// by the IMS stack. Values are passed directly to `Command`, never a shell,
/// but restricting the command family still prevents a compromised parent
/// request from using this privileged worker as a generic `ip` executor.
#[cfg(unix)]
fn validate_xfrm_argv(args: &[String], best_effort: bool) -> Result<(), String> {
    let valid = matches!(
        args,
        [family, object, action, ..]
            if family == "xfrm"
                && matches!(object.as_str(), "state" | "policy")
                && matches!(action.as_str(), "add" | "delete" | "flush")
    );
    if !valid {
        return Err(format!(
            "UE worker rejected unsupported xfrm argv: {}",
            args.join(" ")
        ));
    }
    if best_effort && !matches!(args.get(2).map(String::as_str), Some("delete" | "flush")) {
        return Err("UE worker best-effort xfrm is limited to cleanup operations".to_string());
    }
    Ok(())
}

#[cfg(unix)]
fn route_argv(
    action: &str,
    target: &str,
    via: Option<&str>,
    dev: Option<&str>,
    src: Option<&str>,
    table: Option<u32>,
) -> Vec<String> {
    let ipv6 = target.contains(':')
        || via.is_some_and(|value| value.contains(':'))
        || src.is_some_and(|value| value.contains(':'));
    let mut argv: Vec<String> = Vec::new();
    if ipv6 {
        argv.push("-6".into());
    }
    argv.extend(["route".into(), action.into(), target.into()]);
    if let Some(via) = via {
        argv.push("via".into());
        argv.push(via.into());
    }
    if let Some(dev) = dev {
        argv.push("dev".into());
        argv.push(dev.into());
    }
    if let Some(src) = src {
        argv.push("src".into());
        argv.push(src.into());
    }
    if let Some(table) = table {
        argv.push("table".into());
        argv.push(table.to_string());
    }
    argv
}

/// A few op types are inherently idempotent (`address del`, `route del`):
/// "cannot find" / "no such" style errors are tolerated there.
#[cfg(unix)]
fn is_benign_net_config_error(op: &NetConfigOp, stderr: &str) -> bool {
    let stderr = stderr.to_ascii_lowercase();
    let benign = stderr.contains("cannot find")
        || stderr.contains("no such")
        || stderr.contains("not exist")
        || stderr.contains("file exists")
        || stderr.contains("already exists")
        || stderr.contains("does not exist");
    matches!(
        op,
        NetConfigOp::Xfrm {
            best_effort: true,
            ..
        }
    ) || (benign
        && matches!(
            op,
            NetConfigOp::AddrDel { .. }
                | NetConfigOp::RouteDel { .. }
                | NetConfigOp::LinkSetDown { .. }
                | NetConfigOp::FlushRoutes { .. }
                | NetConfigOp::FlushRoutesForDevice { .. }
        ))
}

#[cfg(unix)]
async fn discover_ip() -> Option<String> {
    for candidate in ["ip", "/sbin/ip", "/usr/sbin/ip", "/usr/bin/ip"] {
        if tokio::process::Command::new(candidate)
            .arg("--version")
            .output()
            .await
            .map(|output| output.status.success())
            .unwrap_or(false)
        {
            return Some(candidate.to_string());
        }
    }
    None
}

#[cfg(not(unix))]
async fn collect_net_status() -> NetStatusSnapshot {
    NetStatusSnapshot::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_round_trips_json_frames() {
        let message = UeWorkerMessage::Ping { nonce: 42 };
        let payload = serde_json::to_vec(&message).unwrap();
        let frame = encode_frame(&payload);
        assert_eq!(frame.len(), 4 + payload.len());
        let len = u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize;
        assert_eq!(len, payload.len());
        let decoded: UeWorkerMessage = serde_json::from_slice(&frame[4..]).unwrap();
        assert_eq!(decoded, message);

        let status = UeWorkerMessage::NetStatus {
            interfaces: vec!["wwan0".to_string()],
            addresses: vec!["10.0.0.5".to_string()],
            default_routes: vec!["via 10.0.0.1 dev wwan0".to_string()],
        };
        let payload = serde_json::to_vec(&status).unwrap();
        let decoded: UeWorkerMessage = serde_json::from_slice(&payload).unwrap();
        assert_eq!(decoded, status);
    }

    #[test]
    fn socket_spec_round_trips_json() {
        let spec = UeSocketSpec::udp_connected(
            "0.0.0.0:500".parse().unwrap(),
            "10.200.1.1:500".parse().unwrap(),
            Some("saveabc".to_string()),
        );
        let payload = serde_json::to_vec(&spec).unwrap();
        let decoded: UeSocketSpec = serde_json::from_slice(&payload).unwrap();
        assert_eq!(decoded, spec);
        let message = UeWorkerMessage::SocketCreateRequest {
            request_id: 9,
            spec,
        };
        let payload = serde_json::to_vec(&message).unwrap();
        let decoded: UeWorkerMessage = serde_json::from_slice(&payload).unwrap();
        assert_eq!(decoded, message);

        let result = UeWorkerMessage::SocketCreateResult {
            request_id: 9,
            ok: true,
            error: None,
        };
        let payload = serde_json::to_vec(&result).unwrap();
        let decoded: UeWorkerMessage = serde_json::from_slice(&payload).unwrap();
        assert_eq!(decoded, result);
    }

    #[test]
    fn net_config_ops_round_trip_json() {
        let message = UeWorkerMessage::NetConfigRequest {
            request_id: 7,
            ops: vec![
                NetConfigOp::LinkSetUp {
                    ifname: "saveabc".to_string(),
                },
                NetConfigOp::AddrReplace {
                    ifname: "saveabc".to_string(),
                    cidr: "10.200.1.2/30".to_string(),
                },
                NetConfigOp::DefaultRouteReplace {
                    via: "10.200.1.1".to_string(),
                    dev: "saveabc".to_string(),
                },
                NetConfigOp::RouteReplace {
                    target: "10.100.1.1".to_string(),
                    via: None,
                    dev: Some("sa_vwfabc".to_string()),
                    src: Some("10.0.0.5".to_string()),
                    table: None,
                },
                NetConfigOp::Xfrm {
                    args: vec![
                        "xfrm".to_string(),
                        "policy".to_string(),
                        "flush".to_string(),
                    ],
                    best_effort: true,
                },
            ],
        };
        let payload = serde_json::to_vec(&message).unwrap();
        let decoded: UeWorkerMessage = serde_json::from_slice(&payload).unwrap();
        assert_eq!(decoded, message);

        let result = UeWorkerMessage::NetConfigResult {
            outcome: NetConfigOutcome {
                request_id: 7,
                ok: false,
                output: vec![],
                error: Some("boom".to_string()),
            },
        };
        let payload = serde_json::to_vec(&result).unwrap();
        let decoded: UeWorkerMessage = serde_json::from_slice(&payload).unwrap();
        assert_eq!(decoded, result);
    }

    #[cfg(unix)]
    #[test]
    fn xfrm_argv_is_restricted_to_state_and_policy_operations() {
        let add = NetConfigOp::Xfrm {
            args: vec!["xfrm".into(), "state".into(), "add".into()],
            best_effort: false,
        };
        assert_eq!(net_config_argv(&add).unwrap(), vec!["xfrm", "state", "add"]);

        let arbitrary = NetConfigOp::Xfrm {
            args: vec!["link".into(), "delete".into(), "wwan0".into()],
            best_effort: true,
        };
        assert!(net_config_argv(&arbitrary).is_err());

        let ignored_add = NetConfigOp::Xfrm {
            args: vec!["xfrm".into(), "policy".into(), "add".into()],
            best_effort: true,
        };
        assert!(net_config_argv(&ignored_add).is_err());
    }

    #[test]
    fn handle_namespace_is_stable_per_line() {
        let namespace = NetnsName::for_line("sa-ue", "line-abc");
        let handle = UeWorkerHandle::for_line("line-abc", namespace);
        assert_eq!(handle.line_id(), "line-abc");
        assert!(handle
            .core
            .control_path
            .to_string_lossy()
            .contains("line-abc"));
    }

    #[test]
    fn worker_cleanup_only_matches_the_expected_generation() {
        assert!(worker_generation_matches(Some(41), 41));
        assert!(!worker_generation_matches(Some(42), 41));
        assert!(!worker_generation_matches(None, 41));
    }

    /// A handle is created once per line and reused for every respawn, so two
    /// clones always share one core. Only the captured generation can tell a
    /// restarted worker from the one a runtime bound its sockets to.
    #[test]
    fn binding_detects_a_respawned_worker_behind_the_same_handle() {
        let namespace = NetnsName::for_line("sa-ue", "line-abc");
        let handle = UeWorkerHandle::for_line("line-abc", namespace);
        handle.core.generation.fetch_add(1, Ordering::SeqCst);
        let bound = handle.bind();
        assert!(bound.is_current());
        assert!(bound.matches(&handle.clone().bind()));

        // The worker crashes and the line registry spawns a replacement.
        handle.core.generation.fetch_add(1, Ordering::SeqCst);
        assert!(!bound.is_current());
        assert!(!bound.matches(&handle.bind()));
        // The manager itself is unchanged, which is why pointer equality alone
        // cannot be used to validate a captured binding.
        assert!(bound.worker().same_instance(&handle));
    }

    #[test]
    fn bindings_never_match_across_lines() {
        let first = UeWorkerHandle::for_line("line-a", NetnsName::for_line("sa-ue", "line-a"));
        let second = UeWorkerHandle::for_line("line-b", NetnsName::for_line("sa-ue", "line-b"));
        assert!(!first.bind().matches(&second.bind()));
        assert!(!first.same_instance(&second));
    }

    #[test]
    fn failed_generation_marker_does_not_touch_a_replacement() {
        let mut current = UeWorkerStatus {
            pid: Some(41),
            ready: true,
            ..Default::default()
        };
        assert!(!mark_worker_generation_failed(
            &mut current,
            40,
            "old reader closed".to_string(),
        ));
        assert!(current.ready);
        assert!(current.last_error.is_none());

        assert!(mark_worker_generation_failed(
            &mut current,
            41,
            "current reader closed".to_string(),
        ));
        assert!(!current.ready);
        assert_eq!(current.last_error.as_deref(), Some("current reader closed"));
    }

    #[cfg(unix)]
    #[test]
    fn idle_control_stream_times_out_without_looking_like_eof() {
        let (reader, _writer) = std::os::unix::net::UnixStream::pair().unwrap();
        let error = recv_control_frame(&reader, Duration::from_millis(20)).unwrap_err();
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
    }

    #[cfg(unix)]
    #[test]
    fn closed_control_stream_is_reported_as_eof() {
        let (reader, writer) = std::os::unix::net::UnixStream::pair().unwrap();
        drop(writer);
        let frame = recv_control_frame(&reader, Duration::from_millis(100)).unwrap();
        assert!(frame.is_none());
    }

    #[tokio::test]
    #[cfg(not(target_os = "linux"))]
    async fn spawn_reports_unsupported_off_linux() {
        let namespace = NetnsName::for_line("sa-ue", "line-abc");
        let handle = UeWorkerHandle::for_line("line-abc", namespace);
        assert!(matches!(
            handle.spawn().await,
            Err(UeWorkerError::Unsupported)
        ));
    }

    #[tokio::test]
    #[cfg(not(target_os = "linux"))]
    async fn create_socket_reports_unsupported_off_linux() {
        let namespace = NetnsName::for_line("sa-ue", "line-abc");
        let handle = UeWorkerHandle::for_line("line-abc", namespace);
        let spec = UeSocketSpec::udp_bound("0.0.0.0:500".parse().unwrap(), None);
        assert!(matches!(
            handle.create_socket(spec).await,
            Err(UeWorkerError::Unsupported)
        ));
    }

    /// Read `O_NONBLOCK` back off an fd the way the kernel reports it, rather
    /// than trusting the value we think we set.
    #[cfg(unix)]
    fn fd_is_nonblocking(fd: std::os::fd::RawFd) -> bool {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert!(
            flags >= 0,
            "F_GETFL failed: {}",
            std::io::Error::last_os_error()
        );
        flags & libc::O_NONBLOCK != 0
    }

    /// Every fd leaving `create_socket_fd` crosses SCM_RIGHTS and is handed
    /// straight to `tokio::net::*::from_std`, which requires a non-blocking fd
    /// and does not set the flag itself. A blocking fd there parks a tokio
    /// *worker thread* inside `recvfrom` instead of parking the task; with one
    /// such socket per core worker the reactor stops being driven entirely and
    /// unrelated subsystems wedge -- observed on the 410 as an HTTP API that
    /// accepted TCP handshakes in the kernel but never answered a request.
    /// The cause is arbitrarily far from the symptom, so assert it at the source.
    #[test]
    #[cfg(unix)]
    fn created_udp_sockets_are_nonblocking() {
        use std::os::fd::AsRawFd;

        let spec = UeSocketSpec::udp_bound("127.0.0.1:0".parse().unwrap(), None);
        let fd = create_socket_fd(&spec).expect("bind loopback udp");
        assert!(
            fd_is_nonblocking(fd.as_raw_fd()),
            "a blocking UDP fd would starve a tokio worker thread"
        );
    }

    /// The TCP path needs its own case: `socket2::connect_timeout` toggles
    /// non-blocking mode internally and restores the socket to *blocking*
    /// before returning, so a `set_nonblocking` call placed before the connect
    /// is silently undone. Only a connected socket exercises that ordering.
    #[test]
    #[cfg(unix)]
    fn created_tcp_sockets_are_nonblocking_after_connect_timeout() {
        use std::os::fd::AsRawFd;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
        let target = listener.local_addr().expect("local addr");

        let spec = UeSocketSpec {
            kind: UeSocketKind::Tcp,
            family: socket_family(target),
            bind: None,
            connect: Some(target),
            bind_to_device: None,
            reuse_address: true,
            connect_timeout_secs: Some(5),
        };
        let fd = create_socket_fd(&spec).expect("connect loopback tcp");
        assert!(
            fd_is_nonblocking(fd.as_raw_fd()),
            "connect_timeout leaves the socket blocking; the flag must be set after it"
        );
    }
}
