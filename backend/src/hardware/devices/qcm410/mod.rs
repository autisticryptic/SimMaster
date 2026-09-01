//! Qualcomm 410 (MSM8916-class) device driver.
//!
//! Everything specific to the Qualcomm 410 pocket-WiFi: exposing spare
//! `DATA*_CNTL` rpmsg channels as QMI endpoints, keeping them out of
//! ModemManager's hands (udev `ID_MM_PORT_IGNORE`), and running a retained WDS
//! session for user data so the IMS bearer never shares a slot with it.

use std::sync::Arc;

use super::{
    transport::{CellularDataTransport, ImsBearerTransport, TransportFuture},
    DeviceCapabilities, DeviceDriver, DeviceKind,
};

pub mod baseband_faults;
pub mod ims_bearer;
pub mod netdev;
pub mod resources;
pub mod secondary_qmi;
pub mod secondary_qmi_data;
pub mod secondary_qmi_init;

/// QCM410 platform driver exposed through the device registry.
pub struct Driver;

impl DeviceDriver for Driver {
    fn kind(&self) -> DeviceKind {
        DeviceKind::Qcm410
    }

    fn is_present(&self) -> bool {
        // The 410 exposes its modem DSP as 4080000.remoteproc. Do not classify
        // the neighbouring a204000.remoteproc (WCNSS Wi-Fi/BT) as a baseband.
        std::path::Path::new("/sys/devices/platform/soc@0/4080000.remoteproc").exists()
            || std::fs::read_dir("/sys/class/remoteproc")
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .any(|entry| {
                    std::fs::read_to_string(entry.path().join("name"))
                        .map(|name| name.trim() == "4080000.remoteproc")
                        .unwrap_or(false)
                })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            // The pocket-WiFi has no local mic/speaker/camera/display/codec;
            // voice and video terminate on the external trunk gateway.
            gateway_mode: true,
            local_audio_capable: false,
            local_video_capable: false,
        }
    }

    fn ims_bearer_transport(&self) -> Option<Arc<dyn ImsBearerTransport>> {
        Some(Arc::new(ims_bearer::Qcm410ImsBearer))
    }

    fn cellular_data_transport(&self) -> Arc<dyn CellularDataTransport> {
        Arc::new(secondary_qmi_data::SecondaryDataRuntime::default())
    }

    fn baseband_fault_policy(&self) -> &'static dyn super::baseband_faults::BasebandFaultPolicy {
        &baseband_faults::Qcm410BasebandFaults
    }

    fn initialize_native_bearers(
        &self,
        write_udev_rule: bool,
        dry_run: bool,
    ) -> TransportFuture<'_, anyhow::Result<()>> {
        Box::pin(async move { secondary_qmi_init::run(write_udev_rule, dry_run).await })
    }

    fn install_update_resources(&self, staging_dir: &str, restart_now: bool) -> String {
        resources::install(staging_dir, restart_now)
    }
}
