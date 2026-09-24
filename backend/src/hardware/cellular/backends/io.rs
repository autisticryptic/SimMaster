//! The only process/serial boundary used by native controllers.
//!
//! No shell, no mmcli, no service start/stop, and no automatic manager
//! takeover. Hardware access is possible only after an explicit native claim.

use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::io::AsyncReadExt;
use zbus::{fdo::DBusProxy, Connection};

use super::{
    config::{NativeBearerConfig, NativeDeviceConfig, NativeProtocol},
    protocol::{single_line, CommandRequest, Tool},
    NativeError,
};
use crate::hardware::devices::transport::TransportFuture;

pub trait NativeIo: Send + Sync {
    /// Fence an uncertain reset/write outcome. Existing receipts remain owned.
    fn invalidate(&self) {}

    fn execute<'a>(
        &'a self,
        request: &'a CommandRequest,
    ) -> TransportFuture<'a, Result<String, NativeError>>;

    fn verify_bearer<'a>(
        &'a self,
        _endpoint: &'a NativeBearerConfig,
    ) -> TransportFuture<'a, Result<(), NativeError>> {
        Box::pin(async {
            Err(NativeError::Unsupported(
                "native_interface_ownership_verification_unavailable",
            ))
        })
    }
    fn verify_owner<'a>(
        &'a self,
        _device: &'a str,
    ) -> TransportFuture<'a, Result<(), NativeError>> {
        Box::pin(async {
            Err(NativeError::Unsupported(
                "native_owner_verification_unavailable",
            ))
        })
    }
    fn save_receipt(&self, _key: &str, _bytes: &[u8], _create: bool) -> Result<(), NativeError> {
        Err(NativeError::Unsupported(
            "native_session_ledger_unavailable",
        ))
    }
    fn clear_receipt(&self, _key: &str) -> Result<(), NativeError> {
        Err(NativeError::Unsupported(
            "native_session_ledger_unavailable",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PortIdentity {
    pub(super) device: String,
    pub(super) canonical: std::path::PathBuf,
    pub(super) sysfs: std::path::PathBuf,
    pub(super) rdev: u64,
    pub(super) inode: u64,
}

#[cfg(unix)]
pub(super) fn identity(device: &str, anchor: &std::path::Path) -> Result<PortIdentity, NativeError> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let canonical = std::fs::canonicalize(device)
        .map_err(|_| NativeError::Unavailable("native_control_port_absent".into()))?;
    let metadata = std::fs::metadata(&canonical)
        .map_err(|_| NativeError::Unavailable("native_control_port_absent".into()))?;
    if !metadata.file_type().is_char_device() {
        return Err(NativeError::OwnerConflict(
            "native_control_port_is_not_character_device".into(),
        ));
    }
    let sysfs = std::fs::canonicalize(format!(
        "/sys/dev/char/{}:{}",
        libc::major(metadata.rdev()),
        libc::minor(metadata.rdev())
    ))
    .map_err(|_| NativeError::OwnerConflict("native_control_sysfs_unresolved".into()))?;
    if !sysfs.starts_with(anchor) {
        return Err(NativeError::OwnerConflict(
            "native_control_port_wrong_physical_device".into(),
        ));
    }
    Ok(PortIdentity {
        device: device.into(),
        canonical,
        sysfs,
        rdev: metadata.rdev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(unix))]
pub(super) fn identity(_device: &str, _anchor: &std::path::Path) -> Result<PortIdentity, NativeError> {
    Err(NativeError::Unsupported("native_backend_requires_linux"))
}

pub struct SystemNativeIo {
    connection: Arc<Connection>,
    #[cfg(unix)]
    spec: NativeDeviceConfig,
    #[cfg(unix)]
    owner_instance: super::recovery::OwnerInstance,
    invalidated: std::sync::atomic::AtomicBool,
    #[cfg(unix)]
    anchor: std::path::PathBuf,
    #[cfg(unix)]
    identities: Vec<PortIdentity>,
    // Drop the proxy-open leases before releasing the physical locks.
    #[cfg(unix)]
    qmi_proxy_leases: Vec<super::qmi_proxy::QmiProxyLease>,
    #[cfg(unix)]
    _locks: Vec<std::fs::File>,
}

impl SystemNativeIo {
    /// The conservative initial handover policy requires MM to be stopped by
    /// the operator. It never stops it itself and never calls a method that
    /// would D-Bus-activate MM. Coexisting device owners need a separate,
    /// explicit inhibition/port-isolation policy, not a fallback here.
    pub(super) async fn verify_manager_absent(connection: &Connection) -> Result<(), NativeError> {
        let bus = DBusProxy::new(connection)
            .await
            .map_err(|_| NativeError::OwnerConflict("native_owner_check_unavailable".into()))?;
        if bus
            .name_has_owner(
                "org.freedesktop.ModemManager1"
                    .try_into()
                    .expect("valid service name"),
            )
            .await
            .map_err(|_| NativeError::OwnerConflict("native_owner_check_unavailable".into()))?
        {
            return Err(NativeError::OwnerConflict(
                "native_handover_required_modemmanager_running".into(),
            ));
        }
        Ok(())
    }

    pub async fn claim(
        spec: &NativeDeviceConfig,
        connection: Arc<Connection>,
    ) -> Result<Arc<Self>, NativeError> {
        Self::verify_manager_absent(&connection).await?;
        #[cfg(unix)]
        {
            use std::os::unix::{
                fs::{OpenOptionsExt, PermissionsExt},
                io::AsRawFd,
            };
            let anchor = std::fs::canonicalize(&spec.sysfs_anchor)
                .map_err(|_| NativeError::OwnerConflict("native_sysfs_anchor_absent".into()))?;
            let ports = std::iter::once(spec.control_device.as_str())
                .chain(spec.at_device.as_deref())
                .chain(spec.ims.iter().map(|b| b.control_device.as_str()))
                .chain(spec.data.iter().map(|b| b.control_device.as_str()))
                .collect::<HashSet<_>>();
            let identities = ports
                .into_iter()
                .map(|p| identity(p, &anchor))
                .collect::<Result<Vec<_>, _>>()?;
            let directory = std::path::Path::new("/run/simadmin/native-control");
            std::fs::create_dir_all(directory).map_err(|_| {
                NativeError::OwnerConflict("native_lock_directory_unavailable".into())
            })?;
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| NativeError::OwnerConflict("native_lock_permissions_failed".into()))?;
            let mut keys = identities
                .iter()
                .map(|p| format!("port-{}", p.rdev))
                .collect::<HashSet<_>>();
            keys.insert(format!(
                "physical-{:x}",
                md5::compute(anchor.as_os_str().as_encoded_bytes())
            ));
            let mut locks = Vec::new();
            // Nonblocking claims cannot deadlock with another partially
            // acquired set; unwinding drops every already-held flock.
            for key in keys {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .open(directory.join(key))
                    .map_err(|_| {
                        NativeError::OwnerConflict("native_device_lock_unavailable".into())
                    })?;
                if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                    return Err(NativeError::OwnerConflict(
                        "native_device_already_owned".into(),
                    ));
                }
                locks.push(file);
            }
            // Opening a new physical channel can invalidate old firmware CIDs.
            // Crash receipts must be reconciled before even a helper bootstrap.
            verify_receipts_clear(directory, &spec.line_id())?;
            super::recovery::ensure_persistent_clear()?;
            super::recovery::ensure_directory(std::path::Path::new(super::recovery::RECEIPT_DIRECTORY))?;
            let owner_instance = super::recovery::OwnerInstance::current()?;
            let mut qmi_proxy_leases = Vec::new();
            if spec.protocol == NativeProtocol::Qmi {
                let ports = std::iter::once(spec.control_device.clone())
                    .chain(spec.ims.iter().map(|e| e.control_device.clone()))
                    .chain(spec.data.iter().map(|e| e.control_device.clone()))
                    .collect::<std::collections::BTreeSet<_>>();
                for port in ports {
                    qmi_proxy_leases.push(open_qmi_proxy_lease(&port).await?);
                }
            }
            Self::verify_manager_absent(&connection).await?;
            for previous in &identities {
                if identity(&previous.device, &anchor)? != *previous {
                    return Err(NativeError::OwnerConflict(
                        "native_device_changed_during_claim".into(),
                    ));
                }
            }
            Ok(Arc::new(Self {
                connection,
                spec: spec.clone(),
                owner_instance,
                invalidated: std::sync::atomic::AtomicBool::new(false),
                anchor,
                identities,
                qmi_proxy_leases,
                _locks: locks,
            }))
        }
        #[cfg(not(unix))]
        {
            let _ = spec;
            Err(NativeError::Unsupported("native_backend_requires_linux"))
        }
    }

    async fn verify(&self, device: &str) -> Result<(), NativeError> {
        if self.invalidated.load(std::sync::atomic::Ordering::Acquire) {
            return Err(NativeError::OwnerConflict(
                "native_maintenance_reconciliation_required".into(),
            ));
        }
        Self::verify_manager_absent(&self.connection).await?;
        #[cfg(unix)]
        {
            for lease in &self.qmi_proxy_leases {
                lease.verify_alive()?;
            }
            let previous = self
                .identities
                .iter()
                .find(|i| i.device == device)
                .ok_or_else(|| NativeError::OwnerConflict("native_port_not_owned".into()))?;
            if identity(device, &self.anchor)? != *previous {
                return Err(NativeError::OwnerConflict(
                    "native_device_generation_changed_restart_required".into(),
                ));
            }
            Ok(())
        }
        #[cfg(not(unix))]
        Err(NativeError::Unsupported("native_backend_requires_linux"))
    }
}

fn verify_receipts_clear(directory: &std::path::Path, _line: &str) -> Result<(), NativeError> {
    // Unknown/legacy records cannot be attributed safely after a hardware-key
    // or slot edit. All lines and DJI/partial receipts block normal startup.
    if !super::recovery::pending_paths(directory)?.is_empty() {
        return Err(NativeError::OwnerConflict(
            "native_sessions_require_reconciliation_before_native_start".into(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
async fn open_qmi_proxy_lease(
    device: &str,
) -> Result<super::qmi_proxy::QmiProxyLease, NativeError> {
    use super::qmi_proxy::QmiProxyLease;
    use crate::connectivity::modems::ims::vowifi::qmi_uim::QmiUimError;
    let open = |path: String| tokio::task::spawn_blocking(move || QmiProxyLease::open(&path));
    let mut result = open(device.to_string())
        .await
        .map_err(|_| NativeError::CommandFailed("native_qmi_proxy_lease_worker_failed"))?;
    if matches!(&result, Err(QmiUimError::Io(error))
        if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused))
    {
        // libqmi starts qmi-proxy on demand. This CTL query neither allocates a
        // WDS client nor changes RF, SIM, data-format, APNs or autoconnect.
        run_process(&CommandRequest::query(
            NativeProtocol::Qmi,
            device,
            "--get-service-version-info",
        ))
        .await?;
        result = open(device.to_string())
            .await
            .map_err(|_| NativeError::CommandFailed("native_qmi_proxy_lease_worker_failed"))?;
    }
    result.map_err(|_| NativeError::OwnerConflict("native_qmi_proxy_open_lease_failed".into()))
}

impl NativeIo for SystemNativeIo {
    fn invalidate(&self) {
        self.invalidated
            .store(true, std::sync::atomic::Ordering::Release);
    }
    fn verify_owner<'a>(&'a self, device: &'a str) -> TransportFuture<'a, Result<(), NativeError>> {
        Box::pin(self.verify(device))
    }
    fn save_receipt(&self, key: &str, bytes: &[u8], create: bool) -> Result<(), NativeError> {
        #[cfg(unix)] {
            let record = super::recovery::Receipt::new(key, &self.spec, &self.anchor,
                self.owner_instance.clone(), self.identities.clone(), bytes)?;
            super::recovery::save_owned(std::path::Path::new(super::recovery::RECEIPT_DIRECTORY), key, &record, create)
        }
        #[cfg(not(unix))] { let _ = (key, bytes, create); Err(NativeError::Unsupported("native_backend_requires_linux")) }
    }
    fn clear_receipt(&self, key: &str) -> Result<(), NativeError> {
        #[cfg(unix)] {
            super::recovery::clear_owned(std::path::Path::new(super::recovery::RECEIPT_DIRECTORY), key, &self.owner_instance)
        }
        #[cfg(not(unix))] { let _ = key; Err(NativeError::Unsupported("native_backend_requires_linux")) }
    }
    fn verify_bearer<'a>(
        &'a self,
        endpoint: &'a NativeBearerConfig,
    ) -> TransportFuture<'a, Result<(), NativeError>> {
        Box::pin(async move {
            self.verify(&endpoint.control_device).await?;
            #[cfg(unix)]
            {
                let sysfs =
                    std::fs::canonicalize(format!("/sys/class/net/{}/device", endpoint.interface))
                        .map_err(|_| {
                            NativeError::OwnerConflict("native_bearer_interface_absent".into())
                        })?;
                if !sysfs.starts_with(&self.anchor) {
                    return Err(NativeError::OwnerConflict(
                        "native_bearer_interface_wrong_device".into(),
                    ));
                }
                let result = tokio::time::timeout(
                    Duration::from_secs(5),
                    tokio::process::Command::new("ip")
                        .args(["-j", "address", "show", "dev", &endpoint.interface])
                        .kill_on_drop(true)
                        .output(),
                )
                .await
                .map_err(|_| {
                    NativeError::OwnerConflict("native_interface_inspection_timeout".into())
                })?
                .map_err(|_| {
                    NativeError::OwnerConflict("native_interface_inspection_failed".into())
                })?;
                if !result.status.success() {
                    return Err(NativeError::OwnerConflict(
                        "native_interface_inspection_failed".into(),
                    ));
                }
                let value: serde_json::Value =
                    serde_json::from_slice(&result.stdout).map_err(|_| {
                        NativeError::OwnerConflict("native_interface_inspection_invalid".into())
                    })?;
                let interfaces = value.as_array().filter(|a| a.len() == 1).ok_or_else(|| {
                    NativeError::OwnerConflict("native_interface_inspection_ambiguous".into())
                })?;
                let addresses = interfaces[0]
                    .get("addr_info")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| {
                        NativeError::OwnerConflict("native_interface_addresses_unknown".into())
                    })?;
                if addresses
                    .iter()
                    .any(|a| a.get("scope").and_then(|s| s.as_str()) != Some("link"))
                {
                    return Err(NativeError::OwnerConflict(
                        "native_interface_already_configured_by_another_owner".into(),
                    ));
                }
                Ok(())
            }
            #[cfg(not(unix))]
            Err(NativeError::Unsupported("native_backend_requires_linux"))
        })
    }

    fn execute<'a>(
        &'a self,
        request: &'a CommandRequest,
    ) -> TransportFuture<'a, Result<String, NativeError>> {
        Box::pin(async move {
            self.verify(&request.device).await?;
            if request.arguments.is_empty() || request.arguments.len() > 32 {
                return Err(NativeError::Protocol("native_command_shape_invalid".into()));
            }
            for argument in &request.arguments {
                single_line(argument)?;
            }
            let result = if matches!(
                request.tool,
                Tool::At | Tool::AtPoll | Tool::AtDirectPoll | Tool::AtDirectBind | Tool::AtDirectCommit | Tool::AtUssd | Tool::AtSms
            ) {
                if request.arguments.len() != if matches!(request.tool, Tool::AtSms | Tool::AtDirectCommit) { 2 } else { 1 } {
                    return Err(NativeError::Protocol(
                        "native_at_transaction_invalid".into(),
                    ));
                }
                if request.tool == Tool::AtPoll && request.arguments[0] != "poll-urcs" {
                    return Err(NativeError::Protocol("native_at_poll_invalid".into()));
                }
                if request.tool == Tool::AtDirectPoll && request.arguments[0] != "direct-pdus" {
                    return Err(NativeError::Protocol("native_at_poll_invalid".into()));
                }
                let device = request.device.clone();
                let command = request.arguments[0].clone();
                let arguments = request.arguments.clone();
                let tool = request.tool;
                let timeout = Duration::from_secs(request.timeout_seconds.clamp(1, 120));
                // The serial implementation has its own deadline. Do not drop
                // a blocking transaction and release the owner gate early.
                tokio::task::spawn_blocking(move || match tool {
                    Tool::AtPoll => {
                        let events = crate::hardware::cellular::at_session::poll_events(&device)?;
                        serde_json::to_string(&events)
                            .map_err(|_| "AT event encoding failed".into())
                    }
                    Tool::AtDirectPoll => {
                        serde_json::to_string(&crate::hardware::cellular::at_session::direct_pdus(&device)?)
                            .map_err(|_| "AT direct queue encoding failed".into())
                    }
                    Tool::AtDirectBind => crate::hardware::cellular::at_session::bind_direct_sim(&device, &command),
                    Tool::AtDirectCommit => {
                        let token = arguments[0].parse::<u64>().map_err(|_| "AT direct token invalid".to_string())?;
                        let ack = match arguments[1].as_str() { "0"=>Some(false), "1"=>Some(true), "unknown"=>None, _=>return Err("AT direct ack invalid".into()) };
                        crate::hardware::cellular::at_session::complete_direct(&device, token, ack)
                    }
                    Tool::AtUssd => {
                        crate::hardware::cellular::at_session::execute_ussd(&device, &command)
                    }
                    Tool::AtSms => {
                        let length = arguments[0]
                            .parse::<usize>()
                            .map_err(|_| "invalid SMS length".to_string())?;
                        crate::hardware::cellular::at_session::send_sms_pdu(
                            &device,
                            &arguments[1],
                            length,
                        )
                    }
                    _ => crate::hardware::cellular::at_session::execute_command_with_timeout(
                        &device, &command, timeout,
                    ),
                })
                .await
                .map_err(|_| NativeError::CommandFailed("native_at_worker_failed"))?
                .map_err(|_| NativeError::CommandFailed("native_at_command_failed"))
            } else if request.tool == Tool::QmiControl {
                if request.arguments.len() != 2 {
                    return Err(NativeError::Protocol("native_qmi_control_invalid".into()));
                }
                let (service, message_id) = match request.arguments[0].as_str() {
                    "band-capabilities" => (2, 0x45),
                    "get-preferences" => (3, 0x34),
                    "set-preferences" => (3, 0x33),
                    _ => {
                        return Err(NativeError::Protocol(
                            "native_qmi_control_not_allowed".into(),
                        ))
                    }
                };
                let fields: Vec<(u8, Vec<u8>)> = serde_json::from_str(&request.arguments[1])
                    .map_err(|_| NativeError::Protocol("native_qmi_control_invalid".into()))?;
                super::management::validate_fields(message_id, &fields)?;
                let device = request.device.clone();
                let response = tokio::task::spawn_blocking(move || {
                    crate::connectivity::modems::ims::vowifi::qmi_uim::native_management_exchange(
                        &device, service, message_id, fields,
                    )
                })
                .await
                .map_err(|_| NativeError::CommandFailed("native_qmi_control_worker_failed"))?
                .map_err(|_| NativeError::CommandFailed("native_qmi_control_failed"))?;
                serde_json::to_string(&response).map_err(|_| {
                    NativeError::Protocol("native_qmi_response_encoding_failed".into())
                })
            } else {
                run_process(request).await
            }?;
            self.verify(&request.device).await?;
            Ok(result)
        })
    }
}

pub(crate) async fn run_process(request: &CommandRequest) -> Result<String, NativeError> {
    let program = match request.tool {
        Tool::Qmi => "qmicli",
        Tool::Mbim => "mbimcli",
        Tool::At | Tool::AtPoll | Tool::AtDirectPoll | Tool::AtDirectBind | Tool::AtDirectCommit | Tool::AtSms | Tool::AtUssd | Tool::QmiControl => {
            return Err(NativeError::Protocol("native_at_not_a_process".into()))
        }
    };
    let mut child = tokio::process::Command::new(program)
        .args(&request.arguments)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| NativeError::CommandFailed("native_protocol_helper_unavailable"))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let result = tokio::time::timeout(
        Duration::from_secs(request.timeout_seconds.clamp(1, 120)),
        async {
            // Different pipe types require separate futures, not a generic closure.
            let stdout = async {
                let mut b = Vec::new();
                stdout.take(1_048_577).read_to_end(&mut b).await.map(|_| b)
            };
            let stderr = async {
                let mut b = Vec::new();
                stderr.take(1_048_577).read_to_end(&mut b).await.map(|_| b)
            };
            let (stdout, stderr, status) = tokio::join!(stdout, stderr, child.wait());
            let stdout =
                stdout.map_err(|_| NativeError::CommandFailed("native_helper_read_failed"))?;
            let stderr =
                stderr.map_err(|_| NativeError::CommandFailed("native_helper_read_failed"))?;
            if stdout.len() > 1_048_576 || stderr.len() > 1_048_576 {
                return Err(NativeError::CommandFailed("native_helper_output_limit"));
            }
            let success = status
                .map_err(|_| NativeError::CommandFailed("native_helper_wait_failed"))?
                .success();
            verify_command_completion(success, &stderr)?;
            String::from_utf8(stdout)
                .map_err(|_| NativeError::Protocol("native_helper_output_encoding".into()))
        },
    )
    .await;
    match result {
        Ok(result) => result,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(NativeError::CommandFailed(
                "native_protocol_command_timeout",
            ))
        }
    }
}

fn verify_command_completion(success: bool, stderr: &[u8]) -> Result<(), NativeError> {
    // qmicli can exit 0 even when its asynchronous CTL Release Client fails.
    // Such a warning must not authorize deleting an ownership receipt.
    if String::from_utf8_lossy(stderr).contains("couldn't release client") {
        return Err(NativeError::CommandFailed(
            "native_qmi_client_release_unconfirmed",
        ));
    }
    if success {
        Ok(())
    } else {
        Err(classify_failed_command(stderr))
    }
}

/// A nonzero helper exit is not proof that a mutating request was rejected.
/// Only an explicit protocol response may turn an uncertain allocation into
/// a known failure. Do not expose stderr (it may contain APN credentials).
fn classify_failed_command(stderr: &[u8]) -> NativeError {
    let text = String::from_utf8_lossy(stderr);
    for prefix in ["QMI protocol error (", "MBIM protocol error ("] {
        if let Some(code) = text
            .split_once(prefix)
            .and_then(|(_, tail)| tail.split_once(')'))
            .and_then(|(code, _)| code.parse::<u16>().ok())
        {
            return NativeError::ProtocolRejected(code);
        }
    }
    NativeError::CommandFailed("native_command_outcome_unconfirmed")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_failure_is_not_silently_treated_as_a_protocol_rejection() {
        assert_eq!(
            classify_failed_command(b"QMI protocol error (14): 'CallFailed'"),
            NativeError::ProtocolRejected(14)
        );
        for message in [
            b"operation timed out".as_slice(),
            b"helper crashed",
            b"QMI protocol error (unknown)",
        ] {
            assert_eq!(
                classify_failed_command(message),
                NativeError::CommandFailed("native_command_outcome_unconfirmed")
            );
        }
    }

    #[test]
    fn successful_process_exit_does_not_hide_an_unconfirmed_client_release() {
        assert_eq!(
            verify_command_completion(
                true,
                b"error: couldn't release client: QMI protocol error (7): 'InvalidClientId'"
            ),
            Err(NativeError::CommandFailed(
                "native_qmi_client_release_unconfirmed"
            ))
        );
        assert!(verify_command_completion(true, b"").is_ok());
    }

    #[test]
    fn native_reopen_requires_reconciling_both_complete_and_partial_receipts() {
        let directory = std::env::temp_dir().join(format!(
            "simadmin-native-receipt-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        verify_receipts_clear(&directory, "fixture").unwrap();
        for role in ["ims", "data", "maintenance", "sim"] {
            for extension in ["json", "tmp", "123.tmp"] {
                let path = directory.join(format!("session-fixture-{role}.{extension}"));
                std::fs::write(&path, "{}").unwrap();
                assert!(verify_receipts_clear(&directory, "fixture").is_err());
                assert!(verify_receipts_clear(&directory, "another-line").is_err());
                std::fs::remove_file(path).unwrap();
            }
        }
        std::fs::remove_dir(directory).unwrap();
    }
}
