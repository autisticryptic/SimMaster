//! Per-UE worker process management (isolation architecture, Option B).
//!
//! Each UE gets its own `simadmin --ue-worker` child process. The parent uses
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

/// Environment variables consumed by the hidden `--ue-worker` subcommand.
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
    /// `ip route flush table <t>`; omitting the table flushes `table main`.
    FlushRoutes {
        table: Option<u32>,
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
    child: tokio::sync::Mutex<Option<tokio::process::Child>>,
    tx: StdMutex<Option<mpsc::UnboundedSender<UeWorkerMessage>>>,
    pending: StdMutex<HashMap<u64, PendingRequest>>,
    request_seq: AtomicU64,
    state: StdMutex<UeWorkerStatus>,
}

/// Cloneable manager handle for one UE worker. One handle per line is owned by
/// [`LineRuntimeRegistry`]; the worker process itself is a single child.
#[derive(Clone)]
pub struct UeWorkerHandle {
    core: Arc<WorkerCore>,
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
                child: tokio::sync::Mutex::new(None),
                tx: StdMutex::new(None),
                pending: StdMutex::new(HashMap::new()),
                request_seq: AtomicU64::new(1),
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
        match spec.kind {
            UeSocketKind::Udp => {
                let std_socket = std::net::UdpSocket::from(fd);
                Ok(UeSocket::Udp(tokio::net::UdpSocket::from_std(std_socket)?))
            }
            UeSocketKind::Tcp => {
                let std_stream = std::net::TcpStream::from(fd);
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
        let status = self.status().await;
        status.ready || status.pid.is_some()
    }

    #[cfg(unix)]
    async fn spawn_unix(&self) -> Result<(), UeWorkerError> {
        use tokio::net::UnixListener;
        use tokio::process::Command;

        if self.status().await.ready {
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
            .arg("--ue-worker")
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
        let child = command.spawn()?;
        {
            let mut guard = self.core.child.lock().await;
            *guard = Some(child);
        }
        let pid = self
            .core
            .child
            .lock()
            .await
            .as_ref()
            .and_then(|child| child.id());
        {
            let mut state = self.core.state.lock().unwrap();
            state.pid = pid;
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
                            {
                                let mut state = core.state.lock().unwrap();
                                state.last_error =
                                    Some(format!("control stream conversion failed: {error}"));
                            }
                            let _ = core.kill_child().await;
                            return;
                        }
                    };
                    let read_stream = match std_stream.try_clone() {
                        Ok(stream) => stream,
                        Err(error) => {
                            {
                                let mut state = core.state.lock().unwrap();
                                state.last_error =
                                    Some(format!("control stream clone failed: {error}"));
                            }
                            let _ = core.kill_child().await;
                            return;
                        }
                    };
                    let write_stream = match tokio::net::UnixStream::from_std(std_stream) {
                        Ok(stream) => stream,
                        Err(error) => {
                            {
                                let mut state = core.state.lock().unwrap();
                                state.last_error = Some(format!(
                                    "control write stream conversion failed: {error}"
                                ));
                            }
                            let _ = core.kill_child().await;
                            return;
                        }
                    };
                    let (_read_half, write_half) = write_stream.into_split();
                    let (tx, rx) = mpsc::unbounded_channel::<UeWorkerMessage>();
                    {
                        let mut guard = core.tx.lock().unwrap();
                        *guard = Some(tx);
                    }
                    tokio::spawn(writer_loop(write_half, rx));
                    tokio::task::spawn_blocking(move || run_parent_reader(read_stream, core));
                }
                Ok(Err(error)) => {
                    {
                        let mut state = core.state.lock().unwrap();
                        state.last_error = Some(format!("accept failed: {error}"));
                    }
                    let _ = core.kill_child().await;
                }
                Err(_) => {
                    {
                        let mut state = core.state.lock().unwrap();
                        state.last_error = Some("handshake timeout".to_string());
                    }
                    let _ = core.kill_child().await;
                }
            }
        });
        Ok(())
    }

    #[cfg(unix)]
    async fn shutdown_unix(&self) -> Result<(), UeWorkerError> {
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
    /// Best-effort kill used when the worker never completes the handshake.
    async fn kill_child(&self) -> std::io::Result<()> {
        let mut guard = self.child.lock().await;
        if let Some(child) = guard.as_mut() {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, child.wait()).await;
        }
        *guard = None;
        Ok(())
    }
}

/// Parent-side reader. Runs on a blocking thread because it needs
/// `recvmsg` with `MSG_PEEK` plus `SCM_RIGHTS` support, which tokio's
/// `AsyncReadExt` cannot expose. Each frame is consumed with exactly one
/// `recvmsg` so fds never get detached from their message.
#[cfg(unix)]
fn run_parent_reader(stream: std::os::unix::net::UnixStream, core: Arc<WorkerCore>) {
    loop {
        match recv_control_frame(&stream, CONTROL_READ_TIMEOUT) {
            Ok(Some((payload, fds))) => {
                let message = match serde_json::from_slice::<UeWorkerMessage>(&payload) {
                    Ok(message) => message,
                    Err(error) => {
                        let mut state = core.state.lock().unwrap();
                        state.last_error =
                            Some(format!("invalid control frame from worker: {error}"));
                        drop(fds);
                        break;
                    }
                };
                handle_parent_message(&core, message, fds);
            }
            Ok(None) => break,
            Err(error) => {
                let mut state = core.state.lock().unwrap();
                state.last_error = Some(format!("worker control read failed: {error}"));
                break;
            }
        }
    }
    let mut state = core.state.lock().unwrap();
    state.ready = false;
    state.last_error = Some("worker_control_closed".to_string());
    *core.tx.lock().unwrap() = None;
}

/// Dispatch a worker-side message on the parent reader thread.
#[cfg(unix)]
fn handle_parent_message(core: &WorkerCore, message: UeWorkerMessage, fds: Vec<i32>) {
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
            let mut state = core.state.lock().unwrap();
            state.ready = true;
            state.netns = netns;
            state.pid = Some(pid);
            state.connected_at = Some(Utc::now().to_rfc3339());
            state.last_message_at = Some(Utc::now().to_rfc3339());
            state.last_error = None;
            tracing::info!(line_id = %core.line_id, pid, "UE worker ready inside its namespace");
        }
        UeWorkerMessage::Pong { nonce } => {
            let mut state = core.state.lock().unwrap();
            state.last_message_at = Some(Utc::now().to_rfc3339());
            state.last_error = None;
            tracing::trace!(line_id = %core.line_id, nonce, "UE worker pong");
        }
        UeWorkerMessage::NetStatus {
            interfaces,
            addresses,
            default_routes,
        } => {
            let mut state = core.state.lock().unwrap();
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
            let mut state = core.state.lock().unwrap();
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
            let mut state = core.state.lock().unwrap();
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

/// Entry point for the hidden `--ue-worker` subcommand. Reads its parameters
/// from the environment (set by the parent before `exec`).
pub async fn run_worker_from_env() -> anyhow::Result<()> {
    let line_id = std::env::var(ENV_LINE_ID)
        .map_err(|_| anyhow::anyhow!("{ENV_LINE_ID} is required for --ue-worker"))?;
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
        header.msg_controllen = cmsg_buf.len();
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
                    as usize;
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
    header_msg.msg_controllen = cmsg_buf.len();
    let received = unsafe { libc::recvmsg(fd, &mut header_msg, 0) };
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
                let payload_len = (*cmsg).cmsg_len.saturating_sub(libc::CMSG_LEN(0) as usize);
                let count = payload_len / std::mem::size_of::<libc::c_int>();
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
        let argv = net_config_argv(&op);
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
fn net_config_argv(op: &NetConfigOp) -> Vec<String> {
    match op {
        NetConfigOp::LinkSetUp { ifname } => {
            vec!["link".into(), "set".into(), ifname.clone(), "up".into()]
        }
        NetConfigOp::LinkSetDown { ifname } => {
            vec!["link".into(), "set".into(), ifname.clone(), "down".into()]
        }
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
        NetConfigOp::FlushRoutes { table } => {
            let table = table
                .map(|value| value.to_string())
                .unwrap_or_else(|| "main".to_string());
            vec!["route".into(), "flush".into(), "table".into(), table]
        }
    }
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
    let mut argv: Vec<String> = vec!["route".into(), action.into(), target.into()];
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
    benign
        && matches!(
            op,
            NetConfigOp::AddrDel { .. }
                | NetConfigOp::RouteDel { .. }
                | NetConfigOp::LinkSetDown { .. }
                | NetConfigOp::FlushRoutes { .. }
        )
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
}
