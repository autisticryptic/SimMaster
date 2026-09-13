//! Explicit backend selection and native device controllers.
//!
//! ModemManager stays the default. Native activation is startup-only,
//! experimental, and requires a deliberate maintenance-window opt-in.

use std::{error::Error, fmt};

pub mod bearer;
pub mod config;
pub mod io;
pub mod management;
pub mod messages;
pub mod native;
pub mod protocol;
pub mod qmi_proxy;
pub mod sim;

static ACTIVE_NATIVE: std::sync::OnceLock<std::sync::Arc<native::NativeFleet>> =
    std::sync::OnceLock::new();
static SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PENDING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn is_shutting_down() -> bool {
    SHUTTING_DOWN.load(std::sync::atomic::Ordering::Acquire)
}
pub fn begin_shutdown() {
    SHUTTING_DOWN.store(true, std::sync::atomic::Ordering::Release);
}

pub(super) struct PendingSetup;
impl PendingSetup {
    pub fn new() -> Self {
        PENDING.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self
    }
}
impl Drop for PendingSetup {
    fn drop(&mut self) {
        PENDING.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

pub async fn drain_native_operations() {
    begin_shutdown();
    let drain = async {
        while PENDING.load(std::sync::atomic::Ordering::Acquire) != 0 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        if let Some(fleet) = active_native() {
            for device in fleet.all() {
                let guard = device.operation.lock().await;
                drop(guard);
            }
        }
    };
    if tokio::time::timeout(std::time::Duration::from_secs(120), drain)
        .await
        .is_err()
    {
        tracing::warn!("Native operation drain timed out; ownership receipts must be reconciled before reactivation");
    }
}

pub fn active_native() -> Option<&'static std::sync::Arc<native::NativeFleet>> {
    ACTIVE_NATIVE.get()
}

/// Do not start MM over a live native owner or an unresolved native session.
/// This check never removes a receipt or stops another process.
pub fn ensure_mm_handover_clear() -> Result<(), String> {
    let entries = match std::fs::read_dir("/run/simadmin/native-control") {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("native_owner_handover_state_unreadable".into()),
    };
    for entry in entries {
        let entry = entry.map_err(|_| "native_owner_handover_state_unreadable")?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("session-") && (name.ends_with(".json") || name.ends_with(".tmp")) {
            return Err("native_sessions_require_reconciliation_before_mm_start".into());
        }
        #[cfg(unix)]
        if name.starts_with("physical-") {
            use std::os::unix::{fs::OpenOptionsExt, io::AsRawFd};
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(entry.path())
                .map_err(|_| "native_owner_handover_state_unreadable")?;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err("native_device_owner_still_running".into());
            }
        }
    }
    Ok(())
}

pub fn native_device(selector: &str) -> Result<std::sync::Arc<native::NativeDevice>, NativeError> {
    ACTIVE_NATIVE
        .get()
        .ok_or_else(|| NativeError::Unavailable("native_backend_not_selected".into()))?
        .device(selector)
}

pub async fn initialize(
    config: &config::BackendConfig,
    connection: std::sync::Arc<zbus::Connection>,
) -> Result<
    (
        std::sync::Arc<dyn super::observations::ModemObservationProvider>,
        std::sync::Arc<dyn super::radio::ModemRadioControl>,
    ),
    String,
> {
    config.validate()?;
    if config.mode == config::BackendMode::Modemmanager {
        return Ok((
            std::sync::Arc::new(super::mm_observations::ModemManagerObservations::new(
                connection.clone(),
            )),
            std::sync::Arc::new(super::mm_radio::ModemManagerRadio::new(connection)),
        ));
    }
    let mut devices = Vec::new();
    for spec in &config.devices {
        let io = io::SystemNativeIo::claim(spec, connection.clone())
            .await
            .map_err(|e| e.to_string())?;
        devices.push(native::NativeDevice::new(spec.clone(), io));
    }
    let fleet = native::NativeFleet::new(devices).map_err(|e| e.to_string())?;
    ACTIVE_NATIVE
        .set(fleet.clone())
        .map_err(|_| "native_backend_already_initialized")?;
    Ok((fleet.clone(), fleet))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeError {
    Unsupported(&'static str),
    Unavailable(String),
    OwnerConflict(String),
    Protocol(String),
    ProtocolRejected(u16),
    CommandFailed(&'static str),
}

impl fmt::Display for NativeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProtocolRejected(code) => write!(f, "native_protocol_rejected:{code}"),
            Self::Unsupported(reason) | Self::CommandFailed(reason) => f.write_str(reason),
            Self::Unavailable(reason) | Self::OwnerConflict(reason) | Self::Protocol(reason) => {
                f.write_str(reason)
            }
        }
    }
}

impl Error for NativeError {}

pub fn is_native_selector(selector: &str) -> bool {
    selector.starts_with("native:")
}
