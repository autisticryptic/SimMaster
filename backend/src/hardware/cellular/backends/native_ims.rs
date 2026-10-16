//! Explicit modem-native IMS probe on an already owned native controller.
//! No MM fallback, bearer allocation, SIM application access or configuration.

use std::{sync::Arc, time::Duration};
use tokio::time::Instant;

use super::{config::NativeProtocol, native::NativeDevice, NativeError};
use crate::hardware::{
    cellular::{bindings::ModemBinding, native_ims::{NativeImsProbeError, NativeImsSample}},
    devices::quectel::native_ims as ec20,
};

const PROBE_BUDGET: Duration = Duration::from_secs(90);
const OWNER_BUDGET: Duration = Duration::from_secs(3);
const AT_BUDGET_SECONDS: u64 = 5;

fn safe_error(error: NativeError) -> NativeImsProbeError {
    match error {
        NativeError::OwnerConflict(_) => NativeImsProbeError::BindingChanged,
        // Unsupported ownership verification is NOT permission to proceed.
        _ => NativeImsProbeError::Unavailable,
    }
}

impl NativeDevice {
    /// Static topology only; model strings and current registration are ignored.
    pub(super) fn supports_native_ims_binding(&self, binding: &ModemBinding) -> bool {
        self.spec.protocol == NativeProtocol::Qmi
            && self.spec.at_device.as_deref().is_some_and(|p| !p.trim().is_empty())
            && self.spec.uim_slot != 0
            && !self.spec.hardware_key.is_empty()
            && !self.spec.sysfs_anchor.is_empty()
            && binding.hardware_key == self.spec.hardware_key
            && binding.line_id == self.spec.line_id()
            && binding.modem_path == self.spec.selector()
            && binding.modem_id == self.spec.selector()
            && binding.primary_port == self.spec.control_device
            && binding.qmi_device.as_deref() == Some(self.spec.control_device.as_str())
            && binding.uim_slot == self.spec.uim_slot
            && matches!(binding.line_kind.as_str(), "" | "baseband")
            && binding.slot_stable
            && !binding.slot_conflict
    }

    pub(super) async fn probe_native_ims_binding(
        self: &Arc<Self>,
        binding: &ModemBinding,
    ) -> Result<NativeImsSample, NativeImsProbeError> {
        if self.spec.protocol != NativeProtocol::Qmi
            || self.spec.at_device.as_deref().is_none_or(|p| p.trim().is_empty())
        {
            return Err(NativeImsProbeError::Unsupported);
        }
        if !self.supports_native_ims_binding(binding) || !binding.present {
            return Err(NativeImsProbeError::BindingChanged);
        }
        let expected = ec20::normalized_iccid(&binding.sim_iccid)
            .ok_or(NativeImsProbeError::BindingChanged)?;
        self.ensure_sim_available().map_err(safe_error)?;
        // No unbounded queue behind the AT pump, maintenance or another probe.
        let guard = self.operation.clone().try_lock_owned()
            .map_err(|_| NativeImsProbeError::Busy)?;
        let this = self.clone();
        let deadline = Instant::now() + PROBE_BUDGET;
        // Dropping/aborting the caller does NOT cancel the transaction. The
        // detached task still owns the device gate until NativeIo's bounded
        // serial/process work has actually finished. Do not timeout its join:
        // returning early could release the caller's host-IMS exclusion while
        // serial work is still draining. The deadline stops further dispatch.
        let task = tokio::spawn(async move {
            let _guard = guard;
            this.native_ims_transaction(&expected, deadline).await
        });
        task.await.map_err(|_| NativeImsProbeError::Unavailable)?
    }

    fn native_ims_checkpoint(&self, deadline: Instant) -> Result<(), NativeImsProbeError> {
        self.ensure_sim_available().map_err(safe_error)?;
        if Instant::now() >= deadline {
            return Err(NativeImsProbeError::Unavailable);
        }
        Ok(())
    }

    async fn native_ims_owner(&self, deadline: Instant) -> Result<(), NativeImsProbeError> {
        // Verify BOTH configured endpoints, including physical/port generation
        // and manager absence; do not discover or open another endpoint.
        for port in [Some(self.spec.control_device.as_str()), self.spec.at_device.as_deref()] {
            self.native_ims_checkpoint(deadline)?;
            let port = port.ok_or(NativeImsProbeError::Unsupported)?;
            tokio::time::timeout_at(deadline.min(Instant::now() + OWNER_BUDGET), self.io.verify_owner(port))
                .await.map_err(|_| NativeImsProbeError::Unavailable)?
                .map_err(safe_error)?;
            self.native_ims_checkpoint(deadline)?;
        }
        Ok(())
    }

    async fn native_ims_slot(&self, deadline: Instant) -> Result<(), NativeImsProbeError> {
        self.native_ims_checkpoint(deadline)?;
        // Existing QMI primary-GW proof, never a slot selector/write. qmicli
        // may use its normal transient UIM query client; no WDS/PDN is created.
        self.verify_primary_slot_locked().await.map_err(safe_error)?;
        self.native_ims_checkpoint(deadline)
    }

    async fn native_ims_read(&self, command: &str, deadline: Instant) -> Result<String, NativeImsProbeError> {
        self.native_ims_checkpoint(deadline)?;
        let mut request = self.at_request(command).map_err(safe_error)?;
        request.timeout_seconds = AT_BUDGET_SECONDS;
        // Do not timeout/drop execute: its spawn_blocking serial transaction
        // must finish before the physical gate can be released.
        let output = self.io.execute(&request).await.map_err(safe_error)?;
        self.native_ims_checkpoint(deadline)?;
        Ok(output)
    }

    async fn native_ims_transaction(&self, expected: &str, deadline: Instant) -> Result<NativeImsSample, NativeImsProbeError> {
        self.native_ims_owner(deadline).await?;
        self.native_ims_slot(deadline).await?;
        let before = self.native_ims_read(ec20::ICCID_COMMAND, deadline).await?;
        if ec20::parse_iccid(&before)? != expected {
            return Err(NativeImsProbeError::BindingChanged);
        }
        let model = self.native_ims_read(ec20::MODEL_COMMAND, deadline).await?;
        let revision = self.native_ims_read(ec20::REVISION_COMMAND, deadline).await?;
        ec20::qualify(&model, &revision)?;
        let output = self.native_ims_read(ec20::STATUS_COMMAND, deadline).await?;
        let sample = ec20::parse_status(&output)?;
        let after = self.native_ims_read(ec20::ICCID_COMMAND, deadline).await?;
        if ec20::parse_iccid(&after)? != expected {
            return Err(NativeImsProbeError::BindingChanged);
        }
        self.native_ims_slot(deadline).await?;
        self.native_ims_owner(deadline).await?;
        Ok(sample)
    }
}

#[cfg(test)]
#[path = "native_ims_tests.rs"]
mod tests;
