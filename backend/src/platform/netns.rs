//! Per-UE Linux network namespace management.
//!
//! This is the foundation for the multi-UE isolation architecture. Each SIM
//! line is modelled as a UE Context and owns its own network namespace, so two
//! UEs may receive identical IP addresses, gateways and P-CSCF addresses
//! without ever sharing routing, neighbour, XFRM or netfilter state.
//!
//! The layer is deliberately additive: every system operation is a small,
//! idempotent `ip` command, and the rest of the application keeps working with
//! the namespace disabled. Data-plane migration (moving the VoLTE bearer
//! netdev into the namespace, running the VoWiFi TUN and the per-UE proxy
//! inside it) is done in later phases behind the same configuration switch.

use std::{fmt, net::Ipv4Addr};

#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use tokio::process::Command;

/// Default namespace name prefix (`sa-ue` + 12 hex chars of the line id hash).
pub const DEFAULT_NAMESPACE_PREFIX: &str = "sa-ue";
/// Default host-side veth prefix.
pub const DEFAULT_HOST_VETH_PREFIX: &str = "savh";
/// Default UE-side veth prefix.
pub const DEFAULT_UE_VETH_PREFIX: &str = "save";
/// Default veth MTU for the UE egress pair.
pub const DEFAULT_VETH_MTU: u32 = 1500;

/// Stable, validated network namespace name for one UE.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NetnsName(String);

impl NetnsName {
    /// Build the stable namespace name for a line. The same line always maps to
    /// the same name across restarts, so reconnect and teardown can reclaim the
    /// same device. Twelve hex characters (48 bits) keep collisions negligible
    /// for realistic line counts while staying well under NAME_MAX.
    pub fn for_line(prefix: &str, line_id: &str) -> Self {
        let prefix = if prefix.trim().is_empty() {
            DEFAULT_NAMESPACE_PREFIX
        } else {
            prefix.trim()
        };
        let digest = md5::compute(line_id.as_bytes());
        let hex = format!("{digest:x}");
        let suffix = &hex[..hex.len().min(12)];
        Self(format!("{prefix}{suffix}"))
    }

    /// Build a stable host-side veth link name for this namespace.
    pub fn host_veth_name(&self, prefix: &str) -> String {
        let base = if prefix.trim().is_empty() {
            DEFAULT_HOST_VETH_PREFIX
        } else {
            prefix.trim()
        };
        let suffix = self.suffix();
        format!("{base}{suffix}")
    }

    /// Build a stable UE-side veth link name for this namespace.
    pub fn ue_veth_name(&self, prefix: &str) -> String {
        let base = if prefix.trim().is_empty() {
            DEFAULT_UE_VETH_PREFIX
        } else {
            prefix.trim()
        };
        let suffix = self.suffix();
        format!("{base}{suffix}")
    }

    /// The short hex suffix used for link names (IFNAMSIZ-safe: 8 chars).
    fn suffix(&self) -> &str {
        self.0
            .get(self.0.len().saturating_sub(8)..)
            .unwrap_or(&self.0)
    }

    /// Expose the 8-hex-char namespace hash suffix. Stable per line and used
    /// to derive deterministic per-UE link names and veth address octets.
    pub fn suffix_hex(&self) -> &str {
        self.suffix()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Adopt a namespace name that already exists on this host.
    ///
    /// [`Self::for_line`] can only name a namespace for a line we currently
    /// know about. Startup reclaim has the opposite problem: a namespace left
    /// by a previous process may hold a netdev while its line has not been
    /// discovered yet, so the name has to come from `/run/netns` instead of
    /// from a line id. The name is validated because it comes from the
    /// filesystem rather than from a hash.
    pub fn adopt(name: &str) -> Result<Self, NetnsError> {
        let name = name.trim();
        validate_namespace_name(name)?;
        Ok(Self(name.to_string()))
    }
}

impl fmt::Display for NetnsName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for NetnsName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetnsErrorKind {
    /// The current platform does not support network namespaces.
    Unsupported,
    /// The namespace/link name is not usable.
    InvalidName,
    /// The requested resource does not exist.
    NotFound,
    /// The requested resource already exists.
    AlreadyExists,
    /// The command could not be spawned.
    SpawnFailed,
    /// The command ran but returned a non-zero exit status.
    CommandFailed,
}

#[derive(Debug, Clone)]
pub struct NetnsError {
    pub kind: NetnsErrorKind,
    pub detail: String,
}

impl NetnsError {
    pub fn unsupported(detail: impl Into<String>) -> Self {
        Self {
            kind: NetnsErrorKind::Unsupported,
            detail: detail.into(),
        }
    }

    pub fn invalid_name(detail: impl Into<String>) -> Self {
        Self {
            kind: NetnsErrorKind::InvalidName,
            detail: detail.into(),
        }
    }

    fn command(program: &str, args: &[String], status: Option<i32>, stderr: &str) -> Self {
        Self {
            kind: NetnsErrorKind::CommandFailed,
            detail: format!(
                "{program}:{}:{}:{}",
                status.unwrap_or(-1),
                args.join(" "),
                stderr.trim()
            ),
        }
    }
}

impl fmt::Display for NetnsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "netns_{:?}:{}", self.kind, self.detail)
    }
}

impl std::error::Error for NetnsError {}

/// Validate a namespace name adopted from the filesystem.
///
/// Namespace names are directory entries under `/run/netns`, so unlike
/// [`NetnsName::for_line`] output they are not trusted by construction. `.` and
/// `..` are rejected along with any path separator, and the character set is the
/// same one `for_line` can produce plus `.` for prefixes that contain one.
fn validate_namespace_name(name: &str) -> Result<(), NetnsError> {
    if name.is_empty()
        || name.len() > 64
        || name == "."
        || name == ".."
        || !name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' || byte == b'.'
        })
    {
        return Err(NetnsError::invalid_name(format!(
            "namespace name {name:?} must be 1..64 chars of [A-Za-z0-9_.-] and not . or .."
        )));
    }
    Ok(())
}

fn validate_link_name(name: &str) -> Result<(), NetnsError> {
    if name.is_empty()
        || name.len() >= 16
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(NetnsError::invalid_name(format!(
            "link name {name:?} must be 1..15 chars of [A-Za-z0-9_-]"
        )));
    }
    Ok(())
}

/// Pure argv construction used by every namespace-capable command. Kept as a
/// function so the exact command lines are unit-testable without a Linux host.
pub fn run_in_argv(namespace: Option<&NetnsName>, program: &str, args: &[&str]) -> Vec<String> {
    let mut argv = Vec::new();
    if let Some(namespace) = namespace {
        argv.extend_from_slice(&["ip".to_string(), "netns".to_string(), "exec".to_string()]);
        argv.push(namespace.to_string());
    }
    argv.push(program.to_string());
    argv.extend(args.iter().map(|arg| (*arg).to_string()));
    argv
}

/// True when the namespace exists on this host (Linux only; false elsewhere).
pub fn exists(namespace: &NetnsName) -> bool {
    #[cfg(target_os = "linux")]
    {
        let name = namespace.as_str();
        Path::new("/run/netns").join(name).exists()
            || Path::new("/var/run/netns").join(name).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = namespace;
        false
    }
}

/// Filesystem ref of a named network namespace. `ip netns add` bind-mounts
/// `/proc/self/ns/net` here; opening this path yields an fd that `setns(2)`
/// can enter.
#[cfg(target_os = "linux")]
pub fn namespace_path(namespace: &NetnsName) -> PathBuf {
    Path::new("/var/run/netns").join(namespace.as_str())
}

/// Returns a `pre_exec` closure for `tokio::process::Command` that enters the
/// UE network namespace in the forked child *before* the worker image execs.
/// This is the core of Option B: the worker process starts inside the UE
/// namespace, so every socket it creates (SIP, RTP, IKE/ESP, DNS) belongs to
/// that UE's stack and identical addresses can never collide with another UE.
///
/// The closure is intentionally minimal (open/setns/close only) because it
/// runs in the fork child where allocation and locking are unsafe.
#[cfg(target_os = "linux")]
pub fn setns_pre_exec(
    namespace: &NetnsName,
) -> Box<dyn FnMut() -> std::io::Result<()> + Send + Sync> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = namespace_path(namespace);
    let c_path = CString::new(path.as_os_str().as_bytes())
        .expect("network namespace path cannot contain NUL")
        .into_bytes_with_nul();
    Box::new(move || {
        // SAFETY: the C string is NUL-terminated and points to a borrowed
        // buffer that stays alive for the whole pre_exec call.
        let fd = unsafe { libc::open(c_path.as_ptr().cast(), libc::O_RDONLY | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: fd refers to an open namespace ref; CLONE_NEWNET only
        // changes the (currently single-threaded) child's network namespace.
        let result = unsafe { libc::setns(fd, libc::CLONE_NEWNET) };
        // SAFETY: close is called exactly once on the fd we opened.
        unsafe { libc::close(fd) };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    })
}

/// Non-Linux placeholder; the closure always fails so workers are never
/// spawned outside Linux.
#[cfg(not(target_os = "linux"))]
pub fn setns_pre_exec(
    _namespace: &NetnsName,
) -> Box<dyn FnMut() -> std::io::Result<()> + Send + Sync> {
    Box::new(|| {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "network namespaces require Linux",
        ))
    })
}

/// Enter the caller's network namespace directly. Only meaningful for a
/// dedicated worker/process; do not call from the main process where other
/// line sockets are already open.
#[cfg(target_os = "linux")]
pub fn enter(namespace: &NetnsName) -> std::io::Result<()> {
    let mut enter = setns_pre_exec(namespace);
    enter()
}

/// Non-Linux placeholder.
#[cfg(not(target_os = "linux"))]
pub fn enter(namespace: &NetnsName) -> std::io::Result<()> {
    let mut enter = setns_pre_exec(namespace);
    enter()
}

/// Create the namespace and bring its loopback up. Idempotent.
#[cfg(target_os = "linux")]
pub async fn ensure(namespace: &NetnsName) -> Result<(), NetnsError> {
    if exists(namespace) {
        return set_loopback_up(namespace).await;
    }
    run_ip_host(&["netns", "add", namespace.as_str()], true).await?;
    set_loopback_up(namespace).await
}

/// Create the namespace and bring its loopback up. Idempotent.
#[cfg(not(target_os = "linux"))]
pub async fn ensure(namespace: &NetnsName) -> Result<(), NetnsError> {
    let _ = namespace;
    Err(NetnsError::unsupported(
        "network namespaces are only supported on Linux",
    ))
}

/// Delete the namespace. Missing namespaces are not an error.
#[cfg(target_os = "linux")]
pub async fn remove(namespace: &NetnsName) -> Result<(), NetnsError> {
    run_ip_host(&["netns", "del", namespace.as_str()], true).await?;
    Ok(())
}

/// Delete the namespace. Missing namespaces are not an error.
#[cfg(not(target_os = "linux"))]
pub async fn remove(namespace: &NetnsName) -> Result<(), NetnsError> {
    let _ = namespace;
    Err(NetnsError::unsupported(
        "network namespaces are only supported on Linux",
    ))
}

/// Bring `lo` up inside the namespace.
#[cfg(target_os = "linux")]
pub async fn set_loopback_up(namespace: &NetnsName) -> Result<(), NetnsError> {
    run_ip_in(namespace, &["link", "set", "lo", "up"], true).await?;
    Ok(())
}

/// Bring `lo` up inside the namespace.
#[cfg(not(target_os = "linux"))]
pub async fn set_loopback_up(namespace: &NetnsName) -> Result<(), NetnsError> {
    let _ = namespace;
    Err(NetnsError::unsupported(
        "network namespaces are only supported on Linux",
    ))
}

/// Run `ip <args>` inside the namespace.
#[cfg(target_os = "linux")]
pub async fn ip_in(namespace: &NetnsName, args: &[&str]) -> Result<String, NetnsError> {
    run_ip_in(namespace, args, false).await
}

/// Run `ip <args>` inside the namespace.
#[cfg(not(target_os = "linux"))]
pub async fn ip_in(namespace: &NetnsName, args: &[&str]) -> Result<String, NetnsError> {
    let _ = (namespace, args);
    Err(NetnsError::unsupported(
        "network namespaces are only supported on Linux",
    ))
}

/// Move a network device into the namespace. The interface disappears from the
/// host namespace afterwards, so callers must only move interfaces that belong
/// exclusively to this UE's data plane.
#[cfg(target_os = "linux")]
pub async fn move_iface_in(namespace: &NetnsName, interface: &str) -> Result<(), NetnsError> {
    validate_link_name(interface)?;
    run_ip_host(
        &["link", "set", interface, "netns", namespace.as_str()],
        false,
    )
    .await
    .map(|_| ())
}

/// Move a network device into the namespace. Unsupported off Linux.
#[cfg(not(target_os = "linux"))]
pub async fn move_iface_in(namespace: &NetnsName, interface: &str) -> Result<(), NetnsError> {
    let _ = (namespace, interface);
    Err(NetnsError::unsupported(
        "network namespaces are only supported on Linux",
    ))
}

/// Move a network device back into the init (host) namespace. Best effort for
/// teardown: a missing device is treated as already moved.
#[cfg(target_os = "linux")]
pub async fn move_iface_out(namespace: &NetnsName, interface: &str) -> Result<(), NetnsError> {
    validate_link_name(interface)?;
    run_ip_in(namespace, &["link", "set", interface, "netns", "1"], true)
        .await
        .map(|_| ())
}

/// Move a network device back into the init namespace. Unsupported off Linux.
#[cfg(not(target_os = "linux"))]
pub async fn move_iface_out(namespace: &NetnsName, interface: &str) -> Result<(), NetnsError> {
    let _ = (namespace, interface);
    Err(NetnsError::unsupported(
        "network namespaces are only supported on Linux",
    ))
}

/// Every namespace currently present on this host, in a stable order.
///
/// Both conventional mount points are read because `exists` accepts either.
/// Unparseable entries are skipped rather than reported: this feeds a
/// best-effort startup sweep, and a stray file in `/run/netns` must not stop it.
#[cfg(target_os = "linux")]
pub fn list_namespaces() -> Vec<NetnsName> {
    let mut names = Vec::new();
    for dir in ["/run/netns", "/var/run/netns"] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(namespace) = NetnsName::adopt(&name) else {
                continue;
            };
            if !names.contains(&namespace) {
                names.push(namespace);
            }
        }
    }
    names.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    names
}

/// Every namespace currently present on this host. Never any off Linux.
#[cfg(not(target_os = "linux"))]
pub fn list_namespaces() -> Vec<NetnsName> {
    Vec::new()
}

/// Extract link names from `ip -o link show` output.
///
/// Pure so the parsing is testable without a Linux host or a namespace. Handles
/// the `@peer` suffix `ip` appends to veth and other paired links, which is part
/// of the display form and not part of the device name.
pub fn parse_link_names(output: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in output.lines() {
        // `6: wwan1: <POINTOPOINT,...> mtu 1500 ...`
        let Some((_index, rest)) = line.split_once(':') else {
            continue;
        };
        let Some((name, _rest)) = rest.trim_start().split_once(':') else {
            continue;
        };
        // veth and other paired links render as `save0c9d2870@if19`.
        let name = name.split('@').next().unwrap_or(name).trim();
        if name.is_empty() {
            continue;
        }
        names.push(name.to_string());
    }
    names
}

/// List the link names inside a namespace.
#[cfg(target_os = "linux")]
pub async fn links_in(namespace: &NetnsName) -> Result<Vec<String>, NetnsError> {
    let output = run_ip_in(namespace, &["-o", "link", "show"], false).await?;
    Ok(parse_link_names(&output))
}

/// List the link names inside a namespace. Unsupported off Linux.
#[cfg(not(target_os = "linux"))]
pub async fn links_in(namespace: &NetnsName) -> Result<Vec<String>, NetnsError> {
    let _ = namespace;
    Err(NetnsError::unsupported(
        "network namespaces are only supported on Linux",
    ))
}

/// True when a link inside the namespace is backed by a real device.
///
/// `/sys/class/net` is network-namespace scoped, and a netdev's `device`
/// symlink still resolves to its global bus path from inside the namespace.
/// That makes the presence of `device` an exact, platform-independent split
/// between hardware netdevs (the modem's bam-dmux interfaces, USB, WiFi) and
/// the software links we create ourselves: veth, the VoWiFi tun and `lo` have
/// no `device` at all. Verified on the reference 410 — from inside the UE
/// namespace `wwan1/device` resolved to `4080000.remoteproc:bam-dmux` while
/// `save*`, `sa_vwf*` and `lo` had none.
///
/// Deliberately does not key off a name prefix or a baseband token. A name
/// pattern would either miss hardware on a platform whose netdevs are not
/// called `wwan*`, or claim a software link that happens to match.
#[cfg(target_os = "linux")]
async fn link_is_hardware(namespace: &NetnsName, interface: &str) -> bool {
    if validate_link_name(interface).is_err() {
        return false;
    }
    // `ls` on the symlink is a plain existence check that needs no shell, so
    // nothing here interpolates into a command line an interpreter would parse.
    let path = format!("/sys/class/net/{interface}/device");
    matches!(
        run_command(
            "ip",
            &["netns", "exec", namespace.as_str(), "ls", path.as_str()],
            false,
        )
        .await,
        Ok(_)
    )
}

/// Return hardware netdevs stranded inside a namespace to the host namespace.
///
/// A data session legitimately keeps its netdev inside the UE namespace while it
/// is active, so this is only correct where no session can exist — see
/// [`reclaim_all_stranded_hardware_links`]. Software links are left alone: the
/// veth peer and the VoWiFi tun belong to the namespace and are rebuilt by their
/// own reconcilers.
///
/// Best effort per interface. One link that refuses to move must not hide the
/// others, so failures are collected and reported rather than returned early.
#[cfg(target_os = "linux")]
pub async fn reclaim_stranded_hardware_links(
    namespace: &NetnsName,
) -> (Vec<String>, Vec<(String, NetnsError)>) {
    let mut reclaimed = Vec::new();
    let mut failed = Vec::new();
    let links = match links_in(namespace).await {
        Ok(links) => links,
        Err(error) => {
            failed.push((namespace.as_str().to_string(), error));
            return (reclaimed, failed);
        }
    };
    for link in links {
        if link == "lo" {
            continue;
        }
        if !link_is_hardware(namespace, &link).await {
            continue;
        }
        match move_iface_out(namespace, &link).await {
            Ok(()) => reclaimed.push(link),
            Err(error) => failed.push((link, error)),
        }
    }
    (reclaimed, failed)
}

/// Return hardware netdevs stranded inside a namespace. Nothing off Linux.
#[cfg(not(target_os = "linux"))]
pub async fn reclaim_stranded_hardware_links(
    namespace: &NetnsName,
) -> (Vec<String>, Vec<(String, NetnsError)>) {
    let _ = namespace;
    (Vec::new(), Vec::new())
}

/// One-shot startup sweep that returns every stranded hardware netdev to the
/// host namespace.
///
/// # Why this is needed
///
/// An active data session legitimately holds its netdev *inside* the UE
/// namespace: `move_data_session_into_worker` puts it there and `deactivate`
/// takes it back out. When the process dies without running `deactivate` — the
/// `graceful shutdown exceeded 8s; forcing process exit` path — the netdev stays
/// behind. Namespace names are deterministic per line, so the next process
/// re-attaches to the very namespace still holding it, and `ensure` only brings
/// `lo` up. Meanwhile `qmi_netdev::candidates_for_baseband` enumerates the
/// *host's* `/sys/class/net`, so the interface is invisible: the resolver finds
/// no candidate that answers, falls back to `Assumed`, and the session comes up
/// unverified — which makes SIP fail silently. Observed on the reference 410,
/// where `wwan1` sat in `sa-ue286e0c9d2870` with `inet 10.92.5.194/30` while the
/// host's ifindex list had a gap exactly at 6.
///
/// # Why it must only run at startup
///
/// This is why the sweep does not live in `ensure`. `ensure` is reached from
/// `reconcile_ue_context` on *every* reconcile, and pulling a netdev out from
/// under a live session would break the working data path this is meant to
/// protect. Call this once, before any line is discovered and therefore before
/// any session can hold a netdev; at that point anything found inside a
/// namespace is by definition a leftover. It is not enough to do this in the
/// `secondary-qmi-init` unit either: that runs once per boot, whereas the leak
/// happens on any `systemctl restart simadmin` within the same boot.
#[cfg(target_os = "linux")]
pub async fn reclaim_all_stranded_hardware_links() {
    for namespace in list_namespaces() {
        let (reclaimed, failed) = reclaim_stranded_hardware_links(&namespace).await;
        for interface in &reclaimed {
            // Deliberately info: this only fires after an unclean shutdown, and
            // it explains why an interface reappeared on the host.
            tracing::info!(
                namespace = %namespace,
                interface = %interface,
                "Reclaimed a data netdev stranded in a UE namespace by a previous run"
            );
        }
        for (interface, error) in &failed {
            tracing::warn!(
                namespace = %namespace,
                interface = %interface,
                error = %error,
                "Could not reclaim a netdev stranded in a UE namespace; \
                 data resolution may fall back to an unverified interface"
            );
        }
    }
}

/// One-shot startup sweep for stranded netdevs. Nothing to do off Linux.
#[cfg(not(target_os = "linux"))]
pub async fn reclaim_all_stranded_hardware_links() {}

/// Create a veth pair that gives the UE namespace an Internet egress through
/// the host (used by VoWiFi and the per-UE proxy in later phases).
///
/// Topology (per UE, addresses inside a /30):
///
/// ```text
/// host namespace                 UE namespace
///   savh<hex>  <------------>    save<hex>
///   host_addr/30                 ue_addr/30
///                                default via host_addr
/// ```
#[cfg(target_os = "linux")]
pub async fn ensure_veth_egress(
    namespace: &NetnsName,
    host_if: &str,
    ue_if: &str,
    host_addr: Ipv4Addr,
    ue_addr: Ipv4Addr,
    mtu: u32,
) -> Result<(), NetnsError> {
    validate_link_name(host_if)?;
    validate_link_name(ue_if)?;
    let host_cidr = format!("{host_addr}/30");
    let ue_cidr = format!("{ue_addr}/30");

    run_ip_host(
        &[
            "link",
            "add",
            host_if,
            "type",
            "veth",
            "peer",
            "name",
            ue_if,
            "mtu",
            &mtu.to_string(),
        ],
        true,
    )
    .await?;
    run_ip_host(&["link", "set", ue_if, "netns", namespace.as_str()], true).await?;
    run_ip_host(&["address", "replace", &host_cidr, "dev", host_if], true).await?;
    run_ip_host(
        &["link", "set", host_if, "up", "mtu", &mtu.to_string()],
        true,
    )
    .await?;

    run_ip_in(
        namespace,
        &["address", "replace", &ue_cidr, "dev", ue_if],
        true,
    )
    .await?;
    run_ip_in(
        namespace,
        &["link", "set", ue_if, "up", "mtu", &mtu.to_string()],
        true,
    )
    .await?;
    run_ip_in(
        namespace,
        &[
            "route",
            "replace",
            "default",
            "via",
            &host_addr.to_string(),
            "dev",
            ue_if,
        ],
        true,
    )
    .await?;
    Ok(())
}

/// Create a veth pair and configure only the host side, moving the UE peer
/// into the namespace. The UE side (address/link/default route) is applied by
/// the UE worker through the control channel so the worker owns its stack.
///
/// Idempotent: re-running after a crash re-creates a missing peer and leaves
/// existing links/addresses untouched.
#[cfg(target_os = "linux")]
pub async fn ensure_veth_pair_host_side(
    namespace: &NetnsName,
    host_if: &str,
    ue_if: &str,
    host_addr: Ipv4Addr,
    mtu: u32,
) -> Result<(), NetnsError> {
    validate_link_name(host_if)?;
    validate_link_name(ue_if)?;
    let host_cidr = format!("{host_addr}/30");

    run_ip_host(
        &[
            "link",
            "add",
            host_if,
            "type",
            "veth",
            "peer",
            "name",
            ue_if,
            "mtu",
            &mtu.to_string(),
        ],
        true,
    )
    .await?;
    run_ip_host(&["link", "set", ue_if, "netns", namespace.as_str()], true).await?;
    run_ip_host(&["address", "replace", &host_cidr, "dev", host_if], true).await?;
    run_ip_host(
        &["link", "set", host_if, "up", "mtu", &mtu.to_string()],
        true,
    )
    .await?;
    Ok(())
}

/// Create the veth pair and configure only the host side. Unsupported off
/// Linux.
#[cfg(not(target_os = "linux"))]
pub async fn ensure_veth_pair_host_side(
    namespace: &NetnsName,
    host_if: &str,
    ue_if: &str,
    host_addr: Ipv4Addr,
    mtu: u32,
) -> Result<(), NetnsError> {
    let _ = (namespace, host_if, ue_if, host_addr, mtu);
    Err(NetnsError::unsupported(
        "network namespaces are only supported on Linux",
    ))
}

/// Create a veth pair for UE egress. Unsupported off Linux.
#[cfg(not(target_os = "linux"))]
pub async fn ensure_veth_egress(
    namespace: &NetnsName,
    host_if: &str,
    ue_if: &str,
    host_addr: Ipv4Addr,
    ue_addr: Ipv4Addr,
    mtu: u32,
) -> Result<(), NetnsError> {
    let _ = (namespace, host_if, ue_if, host_addr, ue_addr, mtu);
    Err(NetnsError::unsupported(
        "network namespaces are only supported on Linux",
    ))
}

/// Remove the host side of a UE egress veth pair. The peer in the UE namespace
/// disappears with it.
#[cfg(target_os = "linux")]
pub async fn teardown_veth(host_if: &str) -> Result<(), NetnsError> {
    run_ip_host(&["link", "del", host_if], true).await?;
    Ok(())
}

/// Remove the host side of a UE egress veth pair. Unsupported off Linux.
#[cfg(not(target_os = "linux"))]
pub async fn teardown_veth(host_if: &str) -> Result<(), NetnsError> {
    let _ = host_if;
    Err(NetnsError::unsupported(
        "network namespaces are only supported on Linux",
    ))
}

/// Best-effort host-side SNAT for a UE veth subnet. Worker-created sockets
/// inside the UE namespace egress through the host side of the veth pair;
/// MASQUERADE makes that traffic routable when the host's primary interface
/// uses a different subnet. Idempotent: the rule is only appended when the
/// check finds it missing. `iptables` is preferred for compatibility with
/// embedded images (including the QCM410 image); systems that only expose
/// nftables use the dedicated `simadmin_nat` table as a fallback.
#[cfg(target_os = "linux")]
pub async fn ensure_host_veth_nat(host_addr: Ipv4Addr) -> Result<(), NetnsError> {
    let cidr = format!("{host_addr}/30");
    // UE-side packets must be routed by the host (not dropped); IPv4
    // forwarding is normally disabled on embedded hosts.
    match Command::new("sysctl")
        .args(["-w", "net.ipv4.ip_forward=1"])
        .status()
        .await
    {
        Ok(status) if status.success() => {}
        Ok(_) => tracing::warn!("Failed to enable IPv4 forwarding for UE veth egress"),
        Err(error) => {
            tracing::warn!(error = %error, "Failed to spawn sysctl for UE veth forwarding")
        }
    }
    let iptables_error = match ensure_host_veth_nat_iptables(&cidr).await {
        Ok(()) => return Ok(()),
        Err(error) => Some(error),
    };

    match ensure_host_veth_nat_nft(&cidr).await {
        Ok(()) => Ok(()),
        Err(nft_error) => Err(combine_nat_errors(iptables_error, nft_error)),
    }
}

/// Remove the host-side SNAT rule installed by [`ensure_host_veth_nat`].
/// Missing rules are treated as success so teardown is safe after a partial
/// setup or a previous process crash.
#[cfg(target_os = "linux")]
pub async fn remove_host_veth_nat(host_addr: Ipv4Addr) -> Result<(), NetnsError> {
    let cidr = format!("{host_addr}/30");
    // Inspect both backends so a rule created before a host switched its
    // firewall frontend is not stranded. An absent binary is benign when the
    // other backend completed cleanup; real command failures are preserved.
    let iptables_result = remove_host_veth_nat_iptables(&cidr).await;
    let nft_result = remove_host_veth_nat_nft(&cidr).await;
    match (iptables_result, nft_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) if is_missing_backend_error(&error) => Ok(()),
        (Ok(()), Err(error)) if is_missing_backend_error(&error) => Ok(()),
        (Err(iptables_error), Err(nft_error))
            if is_missing_backend_error(&iptables_error)
                && is_missing_backend_error(&nft_error) =>
        {
            Ok(())
        }
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(iptables_error), Err(nft_error)) => {
            Err(combine_nat_errors(Some(iptables_error), nft_error))
        }
    }
}

#[cfg(target_os = "linux")]
const NFT_NAT_TABLE: &str = "simadmin_nat";

#[cfg(target_os = "linux")]
const NFT_NAT_CHAIN: &str = "postrouting";

#[cfg(target_os = "linux")]
async fn ensure_host_veth_nat_iptables(cidr: &str) -> Result<(), NetnsError> {
    let mut args = vec![
        "-t".to_string(),
        "nat".to_string(),
        "-C".to_string(),
        "POSTROUTING".to_string(),
        "-s".to_string(),
        cidr.to_string(),
        "-j".to_string(),
        "MASQUERADE".to_string(),
    ];
    let check = Command::new("iptables")
        .args(&args)
        .status()
        .await
        .map_err(|error| NetnsError {
            kind: NetnsErrorKind::SpawnFailed,
            detail: format!("iptables:{error}"),
        })?;
    if check.success() {
        return Ok(());
    }
    args[2] = "-A".to_string();
    let output = Command::new("iptables")
        .args(&args)
        .output()
        .await
        .map_err(|error| NetnsError {
            kind: NetnsErrorKind::SpawnFailed,
            detail: format!("iptables:{error}"),
        })?;
    if output.status.success() {
        return Ok(());
    }
    Err(NetnsError::command(
        "iptables",
        &args,
        output.status.code(),
        &String::from_utf8_lossy(&output.stderr),
    ))
}

#[cfg(target_os = "linux")]
async fn remove_host_veth_nat_iptables(cidr: &str) -> Result<(), NetnsError> {
    let args = vec![
        "-t".to_string(),
        "nat".to_string(),
        "-D".to_string(),
        "POSTROUTING".to_string(),
        "-s".to_string(),
        cidr.to_string(),
        "-j".to_string(),
        "MASQUERADE".to_string(),
    ];
    let output = Command::new("iptables")
        .args(&args)
        .output()
        .await
        .map_err(|error| NetnsError {
            kind: NetnsErrorKind::SpawnFailed,
            detail: format!("iptables:{error}"),
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    if stderr.contains("bad rule")
        || stderr.contains("does a matching rule exist")
        || stderr.contains("no chain/target/match")
    {
        return Ok(());
    }
    Err(NetnsError::command(
        "iptables",
        &args,
        output.status.code(),
        &String::from_utf8_lossy(&output.stderr),
    ))
}

#[cfg(target_os = "linux")]
async fn ensure_host_veth_nat_nft(cidr: &str) -> Result<(), NetnsError> {
    // Native nftables does not require a system-wide ruleset or a shell. The
    // private table/chain lets multiple UE rules coexist and avoids touching
    // NetworkManager/firewalld-owned chains.
    run_nft_command(&["add", "table", "ip", NFT_NAT_TABLE], true).await?;
    run_nft_command(
        &[
            "add",
            "chain",
            "ip",
            NFT_NAT_TABLE,
            NFT_NAT_CHAIN,
            "{",
            "type",
            "nat",
            "hook",
            "postrouting",
            "priority",
            "100",
            ";",
            "policy",
            "accept",
            ";",
            "}",
        ],
        true,
    )
    .await?;

    let listing =
        run_nft_output(&["-a", "list", "chain", "ip", NFT_NAT_TABLE, NFT_NAT_CHAIN]).await?;
    if nft_rule_handle(&listing, cidr).is_some() {
        return Ok(());
    }
    run_nft_command(
        &[
            "add",
            "rule",
            "ip",
            NFT_NAT_TABLE,
            NFT_NAT_CHAIN,
            "ip",
            "saddr",
            cidr,
            "counter",
            "masquerade",
        ],
        false,
    )
    .await
}

#[cfg(target_os = "linux")]
async fn remove_host_veth_nat_nft(cidr: &str) -> Result<(), NetnsError> {
    let listing =
        match run_nft_output(&["-a", "list", "chain", "ip", NFT_NAT_TABLE, NFT_NAT_CHAIN]).await {
            Ok(listing) => listing,
            Err(error) if is_missing_nft_object_error(&error) => return Ok(()),
            Err(error) => return Err(error),
        };
    let Some(handle) = nft_rule_handle(&listing, cidr) else {
        return Ok(());
    };
    run_nft_command(
        &[
            "delete",
            "rule",
            "ip",
            NFT_NAT_TABLE,
            NFT_NAT_CHAIN,
            "handle",
            &handle.to_string(),
        ],
        false,
    )
    .await
}

#[cfg(target_os = "linux")]
async fn run_nft_output(args: &[&str]) -> Result<String, NetnsError> {
    let output = Command::new("nft")
        .args(args)
        .output()
        .await
        .map_err(|error| NetnsError {
            kind: NetnsErrorKind::SpawnFailed,
            detail: format!("nft:{error}"),
        })?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(NetnsError::command(
        "nft",
        &args
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>(),
        output.status.code(),
        &String::from_utf8_lossy(&output.stderr),
    ))
}

#[cfg(target_os = "linux")]
async fn run_nft_command(args: &[&str], allow_existing: bool) -> Result<(), NetnsError> {
    let output = Command::new("nft")
        .args(args)
        .output()
        .await
        .map_err(|error| NetnsError {
            kind: NetnsErrorKind::SpawnFailed,
            detail: format!("nft:{error}"),
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    if allow_existing && (stderr.contains("file exists") || stderr.contains("already exists")) {
        return Ok(());
    }
    Err(NetnsError::command(
        "nft",
        &args
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>(),
        output.status.code(),
        &String::from_utf8_lossy(&output.stderr),
    ))
}

/// Parse the handle of the rule we own from `nft -a list chain` output. The
/// expression is intentionally strict so another UE's subnet cannot be
/// removed by accident. This helper is pure and therefore testable on every
/// build target.
#[cfg(any(target_os = "linux", test))]
fn nft_rule_handle(output: &str, cidr: &str) -> Option<u64> {
    output.lines().find_map(|line| {
        let line = line.trim();
        if !line.contains("ip saddr ") || !line.contains(cidr) || !line.contains("masquerade") {
            return None;
        }
        let marker = line.rsplit_once("# handle ")?.1.trim();
        marker.parse().ok()
    })
}

#[cfg(target_os = "linux")]
fn is_missing_backend_error(error: &NetnsError) -> bool {
    let detail = error.detail.to_ascii_lowercase();
    error.kind == NetnsErrorKind::SpawnFailed
        && (detail.contains("no such file") || detail.contains("not found"))
}

#[cfg(target_os = "linux")]
fn is_missing_nft_object_error(error: &NetnsError) -> bool {
    error.kind == NetnsErrorKind::CommandFailed
        && (error.detail.to_ascii_lowercase().contains("no such file")
            || error
                .detail
                .to_ascii_lowercase()
                .contains("could not process rule")
            || error.detail.to_ascii_lowercase().contains("does not exist"))
}

#[cfg(target_os = "linux")]
fn combine_nat_errors(iptables_error: Option<NetnsError>, nft_error: NetnsError) -> NetnsError {
    let detail = match iptables_error {
        Some(error) => format!("iptables: {}; nft: {}", error.detail, nft_error.detail),
        None => format!("nft: {}", nft_error.detail),
    };
    NetnsError {
        kind: if nft_error.kind == NetnsErrorKind::SpawnFailed {
            NetnsErrorKind::SpawnFailed
        } else {
            NetnsErrorKind::CommandFailed
        },
        detail,
    }
}

/// Remove the host-side SNAT rule for a UE veth subnet. Unsupported off
/// Linux.
#[cfg(not(target_os = "linux"))]
pub async fn ensure_host_veth_nat(_host_addr: Ipv4Addr) -> Result<(), NetnsError> {
    Err(NetnsError::unsupported(
        "network namespaces are only supported on Linux",
    ))
}

#[cfg(not(target_os = "linux"))]
pub async fn remove_host_veth_nat(_host_addr: Ipv4Addr) -> Result<(), NetnsError> {
    Err(NetnsError::unsupported(
        "network namespaces are only supported on Linux",
    ))
}

#[cfg(target_os = "linux")]
async fn run_ip_host(args: &[&str], allow_existing: bool) -> Result<String, NetnsError> {
    run_command("ip", args, allow_existing).await
}

#[cfg(target_os = "linux")]
async fn run_ip_in(
    namespace: &NetnsName,
    args: &[&str],
    allow_existing: bool,
) -> Result<String, NetnsError> {
    let argv = run_in_argv(Some(namespace), "ip", args);
    let program = argv.first().map(String::as_str).unwrap_or("ip").to_string();
    let rest = argv.iter().skip(1).map(String::as_str).collect::<Vec<_>>();
    run_command(&program, &rest, allow_existing).await
}

#[cfg(target_os = "linux")]
async fn run_command(
    program: &str,
    args: &[&str],
    allow_existing: bool,
) -> Result<String, NetnsError> {
    let output = Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|error| NetnsError {
            kind: NetnsErrorKind::SpawnFailed,
            detail: format!("{program}:{error}"),
        })?;
    let argv = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    if allow_existing
        && (stderr.contains("file exists")
            || stderr.contains("already exists")
            || stderr.contains("no such file")
            || stderr.contains("cannot find device")
            || stderr.contains("not exist"))
    {
        return Ok(String::new());
    }
    Err(NetnsError::command(
        program,
        &argv,
        output.status.code(),
        &String::from_utf8_lossy(&output.stderr),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_stable_and_distinct_per_line() {
        let a = NetnsName::for_line("sa-ue", "line-11111111111111111111111111111111");
        let b = NetnsName::for_line("sa-ue", "line-22222222222222222222222222222222");
        let a_again = NetnsName::for_line("sa-ue", "line-11111111111111111111111111111111");
        assert_ne!(a, b);
        assert_eq!(a, a_again);
        assert!(a.as_str().starts_with("sa-ue"));
        assert!(a.as_str().len() <= 24);
    }

    #[test]
    fn default_prefix_is_used_when_config_is_empty() {
        let name = NetnsName::for_line("  ", "line-abc");
        assert!(name.as_str().starts_with(DEFAULT_NAMESPACE_PREFIX));
    }

    #[test]
    fn run_in_argv_prefixes_the_namespace() {
        let namespace = NetnsName::for_line("sa-ue", "line-abc");
        let argv = run_in_argv(Some(&namespace), "ip", &["-json", "link"]);
        assert_eq!(
            argv,
            vec![
                "ip".to_string(),
                "netns".to_string(),
                "exec".to_string(),
                namespace.to_string(),
                "ip".to_string(),
                "-json".to_string(),
                "link".to_string(),
            ]
        );
        assert_eq!(
            run_in_argv(None, "ip", &["link"]),
            vec!["ip".to_string(), "link".to_string()]
        );
    }

    #[test]
    fn veth_names_fit_ifnamesiz() {
        let namespace = NetnsName::for_line("sa-ue", "line-abc");
        let host = namespace.host_veth_name("savh");
        let ue = namespace.ue_veth_name("save");
        assert!(host.len() < 16 && ue.len() < 16);
        assert!(host.starts_with("savh"));
        assert!(ue.starts_with("save"));
    }

    #[test]
    fn invalid_link_names_are_rejected() {
        assert!(validate_link_name("").is_err());
        assert!(validate_link_name("this-name-is-way-too-long").is_err());
        assert!(validate_link_name("bad name").is_err());
        assert!(validate_link_name("wwan0").is_ok());
    }

    #[test]
    fn adopted_namespace_names_reject_path_traversal() {
        // These become `ip netns exec` arguments, so a separator or a relative
        // entry must never survive validation.
        assert!(NetnsName::adopt(".").is_err());
        assert!(NetnsName::adopt("..").is_err());
        assert!(NetnsName::adopt("").is_err());
        assert!(NetnsName::adopt("../../etc/passwd").is_err());
        assert!(NetnsName::adopt("sa-ue 286e0c9d2870").is_err());
        assert!(NetnsName::adopt("sa-ue/286e").is_err());
        // A real name from the reference device, and one that a configured
        // prefix containing a dot could produce.
        assert_eq!(
            NetnsName::adopt("sa-ue286e0c9d2870").unwrap().as_str(),
            "sa-ue286e0c9d2870"
        );
        assert!(NetnsName::adopt("sa.ue286e0c9d2870").is_ok());
        // The name comes from a directory entry, so surrounding whitespace is
        // trimmed rather than rejected.
        assert_eq!(
            NetnsName::adopt("  sa-ue286e0c9d2870\n").unwrap().as_str(),
            "sa-ue286e0c9d2870"
        );
    }

    #[test]
    fn parse_link_names_reads_real_namespace_listing() {
        // Verbatim `ip -o link show` output from inside sa-ue286e0c9d2870 on the
        // reference 410, where wwan1 was found stranded after a forced exit.
        let output = "1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN mode DEFAULT group default qlen 1000\\    link/loopback 00:00:00:00:00:00 brd 00:00:00:00:00:00\n\
             6: wwan1: <POINTOPOINT,NOARP,UP,LOWER_UP> mtu 1500 qdisc pfifo_fast state UNKNOWN mode DEFAULT group default qlen 1000\\    link/[519] \n\
             18: save0c9d2870@if19: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc noqueue state UP mode DEFAULT group default qlen 1000\\    link/ether 86:94:09:cc:f0:c7 brd ff:ff:ff:ff:ff:ff link-netnsid 0\n\
             20: sa_vwf0c931974d: <POINTOPOINT,MULTICAST,NOARP,UP,LOWER_UP> mtu 1600 qdisc pfifo_fast state UNKNOWN mode DEFAULT group default qlen 500\\    link/none \n";
        assert_eq!(
            parse_link_names(output),
            vec![
                "lo".to_string(),
                "wwan1".to_string(),
                // The `@if19` peer suffix is display form, not part of the name;
                // passing it to `ip link set` would fail.
                "save0c9d2870".to_string(),
                "sa_vwf0c931974d".to_string(),
            ]
        );
    }

    #[test]
    fn parse_link_names_ignores_malformed_lines() {
        assert!(parse_link_names("").is_empty());
        assert!(parse_link_names("garbage with no colon\n").is_empty());
        assert!(parse_link_names("7:\n").is_empty());
        assert_eq!(
            parse_link_names("7: wwan2: <UP>\n"),
            vec!["wwan2".to_string()]
        );
    }

    #[test]
    fn nft_rule_handle_matches_only_our_source_masquerade_rule() {
        let listing = r#"
            chain postrouting {
                    type nat hook postrouting priority srcnat; policy accept;
                    ip saddr 10.200.12.156/30 counter packets 0 bytes 0 masquerade # handle 17
                    ip saddr 10.200.12.160/30 counter packets 0 bytes 0 masquerade # handle 18
            }
        "#;
        assert_eq!(nft_rule_handle(listing, "10.200.12.156/30"), Some(17));
        assert_eq!(nft_rule_handle(listing, "10.200.12.160/30"), Some(18));
        assert_eq!(nft_rule_handle(listing, "10.200.12.164/30"), None);
    }

    #[test]
    fn nft_rule_handle_ignores_non_masquerade_or_unrelated_lines() {
        let listing = r#"
            ip saddr 10.200.12.156/30 counter packets 0 bytes 0 accept # handle 21
            ip daddr 10.200.12.156/30 counter packets 0 bytes 0 masquerade # handle 22
            ip saddr 10.200.12.156/30 counter packets 0 bytes 0 masquerade
        "#;
        assert_eq!(nft_rule_handle(listing, "10.200.12.156/30"), None);
    }

    #[tokio::test]
    #[cfg(not(target_os = "linux"))]
    async fn non_linux_operations_report_unsupported() {
        let namespace = NetnsName::for_line("sa-ue", "line-abc");
        assert!(matches!(
            ensure(&namespace).await,
            Err(NetnsError {
                kind: NetnsErrorKind::Unsupported,
                ..
            })
        ));
        assert!(!exists(&namespace));
    }
}
