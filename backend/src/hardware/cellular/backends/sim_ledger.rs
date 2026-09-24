//! Native SIM/APDU ownership receipts. Never store AIDs, APDUs, IMSI or AKA.
//! Capacity is unknown unless a future device-specific query proves it; one
//! exclusive physical transaction is not a claim that the UICC has one channel.
use super::{native::NativeDevice, NativeError};
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Identity,
    Authentication,
    Epdg,
    Probe,
    QmiUim,
    Esim,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelOwner {
    pub purpose: Purpose,
    pub slot: u8,
    pub client_id: Option<u8>,
    pub channel_id: Option<u32>,
    pub state: &'static str,
    pub external_channels_unknown: bool,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Ledger {
    pub capacity: Option<u32>,
    pub owner: Option<ChannelOwner>,
    pub reconciliation_required: bool,
    pub confirmed_opens: u64,
    pub confirmed_closes: u64,
    pub rejected_opens: u64,
    #[serde(skip)]
    serial: u64,
}

pub struct ChannelLease {
    device: Arc<NativeDevice>,
    serial: u64,
    key: String,
    finished: bool,
}

impl ChannelLease {
    pub fn begin(
        device: Arc<NativeDevice>,
        purpose: Purpose,
        external: bool,
    ) -> Result<Self, NativeError> {
        device.ensure_available()?;
        let key = format!("session-{}-sim", device.spec.line_id());
        let mut ledger = device.sim_ledger.lock().unwrap_or_else(|p| p.into_inner());
        if ledger.owner.is_some() || ledger.reconciliation_required {
            return Err(NativeError::OwnerConflict(
                "native_sim_channels_pending_reconciliation".into(),
            ));
        }
        ledger.serial = ledger.serial.wrapping_add(1);
        ledger.owner = Some(ChannelOwner {
            purpose,
            slot: device.spec.uim_slot,
            client_id: None,
            channel_id: None,
            state: if external {
                "external_operation"
            } else {
                "open_pending"
            },
            external_channels_unknown: external,
        });
        let serial = ledger.serial;
        let bytes = serde_json::to_vec(&*ledger).expect("serializable SIM ledger");
        if let Err(error) = device.io.save_receipt(&key, &bytes, true) {
            ledger.reconciliation_required = true;
            return Err(error);
        }
        drop(ledger);
        Ok(Self {
            device,
            serial,
            key,
            finished: false,
        })
    }

    pub fn opened(&mut self, client_id: Option<u8>, channel_id: u32) -> Result<(), NativeError> {
        if channel_id == 0 {
            return Err(NativeError::Protocol(
                "native_sim_channel_id_invalid".into(),
            ));
        }
        let mut ledger = self
            .device
            .sim_ledger
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if ledger.serial != self.serial {
            return Err(NativeError::OwnerConflict(
                "native_sim_lease_replaced".into(),
            ));
        }
        let owner = ledger
            .owner
            .as_mut()
            .ok_or_else(|| NativeError::OwnerConflict("native_sim_lease_missing".into()))?;
        owner.client_id = client_id;
        owner.channel_id = Some(channel_id);
        owner.state = "open";
        ledger.confirmed_opens = ledger.confirmed_opens.saturating_add(1);
        self.device.io.save_receipt(
            &self.key,
            &serde_json::to_vec(&*ledger).expect("SIM ledger"),
            false,
        )
    }

    fn update(&mut self, action: impl FnOnce(&mut Ledger) -> Result<(), NativeError>) -> Result<(), NativeError> {
        let mut ledger = self.device.sim_ledger.lock().unwrap_or_else(|p| p.into_inner());
        if ledger.serial != self.serial { return Err(NativeError::OwnerConflict("native_sim_lease_replaced".into())); }
        action(&mut ledger)?;
        self.device.io.save_receipt(&self.key, &serde_json::to_vec(&*ledger).expect("SIM ledger"), false)
    }

    /// QMI CTL allocation is a resource operation too, before logical-channel open.
    pub fn client_allocated(&mut self, client: u8) -> Result<(), NativeError> {
        if client == 0 { return Err(NativeError::Protocol("native_sim_client_invalid".into())); }
        self.update(|ledger| {
            let owner = ledger.owner.as_mut().ok_or_else(|| NativeError::OwnerConflict("native_sim_lease_missing".into()))?;
            owner.client_id = Some(client);
            owner.state = "client_open";
            Ok(())
        })
    }
    pub fn channel_open_pending(&mut self) -> Result<(), NativeError> {
        self.update(|ledger| {
            let owner = ledger.owner.as_mut().ok_or_else(|| NativeError::OwnerConflict("native_sim_lease_missing".into()))?;
            owner.state = "open_pending";
            Ok(())
        })
    }
    pub fn channel_open_rejected(&mut self) -> Result<(), NativeError> {
        self.update(|ledger| {
            let owner = ledger.owner.as_mut().ok_or_else(|| NativeError::OwnerConflict("native_sim_lease_missing".into()))?;
            owner.state = "client_open";
            ledger.rejected_opens = ledger.rejected_opens.saturating_add(1);
            Ok(())
        })
    }
    pub fn channel_closed_retaining_client(&mut self) -> Result<(), NativeError> {
        self.update(|ledger| {
            let owner = ledger.owner.as_mut().ok_or_else(|| NativeError::OwnerConflict("native_sim_lease_missing".into()))?;
            if owner.client_id.is_none() || owner.channel_id.is_none() { return Err(NativeError::OwnerConflict("native_sim_channel_scope_invalid".into())); }
            owner.channel_id = None;
            owner.state = "client_open";
            ledger.confirmed_closes = ledger.confirmed_closes.saturating_add(1);
            Ok(())
        })
    }
    pub fn client_released(&mut self) -> Result<(), NativeError> {
        self.update(|ledger| {
            let owner = ledger.owner.as_mut().ok_or_else(|| NativeError::OwnerConflict("native_sim_lease_missing".into()))?;
            if owner.channel_id.is_some() || owner.state != "client_open" { return Err(NativeError::OwnerConflict("native_sim_channel_release_unconfirmed".into())); }
            owner.client_id = None;
            owner.state = "closed";
            Ok(())
        })
    }

    /// A logical-channel close alone does not release its QMI client.
    pub fn closed(&mut self) -> Result<(), NativeError> {
        if self.device.sim_ledger.lock().unwrap_or_else(|p| p.into_inner()).owner.as_ref().is_some_and(|owner| owner.client_id.is_some()) {
            return Err(NativeError::OwnerConflict("native_sim_client_release_pending".into()));
        }
        self.finish(false)
    }
    /// Only an explicit protocol rejection proves allocation never happened.
    pub fn rejected(&mut self) -> Result<(), NativeError> {
        if self.device.sim_ledger.lock().unwrap_or_else(|p| p.into_inner()).owner.as_ref().is_some_and(|owner| owner.client_id.is_some() || owner.channel_id.is_some()) {
            return Err(NativeError::OwnerConflict("native_sim_allocated_resources_still_owned".into()));
        }
        self.finish(true)
    }
    /// An external helper exposes no channel IDs; process completion releases
    /// its exclusive scope, not a fabricated count of hardware channel closes.
    pub fn external_completed(&mut self) -> Result<(), NativeError> {
        self.finish(false)
    }

    fn finish(&mut self, rejected: bool) -> Result<(), NativeError> {
        let mut ledger = self
            .device
            .sim_ledger
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if ledger.serial != self.serial {
            return Err(NativeError::OwnerConflict(
                "native_sim_lease_replaced".into(),
            ));
        }
        self.device.io.clear_receipt(&self.key)?;
        if rejected {
            ledger.rejected_opens = ledger.rejected_opens.saturating_add(1);
        } else if ledger
            .owner
            .as_ref()
            .is_some_and(|o| o.channel_id.is_some())
        {
            ledger.confirmed_closes = ledger.confirmed_closes.saturating_add(1);
        }
        ledger.owner = None;
        ledger.reconciliation_required = false;
        self.finished = true;
        Ok(())
    }
}
impl Drop for ChannelLease {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let mut ledger = self
            .device
            .sim_ledger
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if ledger.serial == self.serial {
            ledger.reconciliation_required = true;
            if let Some(owner) = ledger.owner.as_mut() {
                owner.state = "outcome_unconfirmed";
            }
            // Even if this update fails the original open-pending receipt is
            // deliberately left on disk. Never issue a blind CCHC from Drop.
            let _ = self.device.io.save_receipt(
                &self.key,
                &serde_json::to_vec(&*ledger).expect("SIM ledger"),
                false,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{
        cellular::backends::{
            config::{NativeDeviceConfig, NativeProtocol},
            io::NativeIo,
            protocol::CommandRequest,
        },
        devices::transport::TransportFuture,
    };
    struct MemoryIo {
        receipts: std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
    }
    impl NativeIo for MemoryIo {
        fn execute<'a>(
            &'a self,
            _: &'a CommandRequest,
        ) -> TransportFuture<'a, Result<String, NativeError>> {
            Box::pin(async { panic!("ledger must not execute hardware commands") })
        }
        fn save_receipt(&self, key: &str, bytes: &[u8], create: bool) -> Result<(), NativeError> {
            let mut values = self.receipts.lock().unwrap();
            if create && values.contains_key(key) {
                return Err(NativeError::OwnerConflict("exists".into()));
            }
            values.insert(key.into(), bytes.to_vec());
            Ok(())
        }
        fn clear_receipt(&self, key: &str) -> Result<(), NativeError> {
            self.receipts
                .lock()
                .unwrap()
                .remove(key)
                .expect("existing receipt");
            Ok(())
        }
    }
    fn fixture() -> (Arc<NativeDevice>, Arc<MemoryIo>) {
        let io = Arc::new(MemoryIo {
            receipts: Default::default(),
        });
        let device = NativeDevice::new(
            NativeDeviceConfig {
                hardware_key: "ledger-fixture".into(),
                sysfs_anchor: "/sys/devices/fixture".into(),
                protocol: NativeProtocol::At,
                control_device: "/dev/fixture".into(),
                at_device: None,
                sms_reception_enabled: false,
                uim_slot: 1,
                ims: None,
                data: None,
            },
            io.clone(),
        );
        (device, io)
    }
    #[test]
    fn confirmed_open_and_close_release_only_the_current_receipt() {
        let (d, io) = fixture();
        let mut c = ChannelLease::begin(d.clone(), Purpose::Identity, false).unwrap();
        c.opened(None, 3).unwrap();
        let snapshot = d.sim_ledger.lock().unwrap().clone();
        assert_eq!(snapshot.owner.unwrap().channel_id, Some(3));
        assert_eq!(snapshot.capacity, None);
        c.closed().unwrap();
        drop(c);
        assert!(io.receipts.lock().unwrap().is_empty());
        let s = d.sim_ledger.lock().unwrap();
        assert_eq!(s.confirmed_opens, 1);
        assert_eq!(s.confirmed_closes, 1);
    }
    #[test]
    fn unknown_open_or_close_survives_drop_and_blocks_new_allocations() {
        for opened in [false, true] {
            let (d, io) = fixture();
            let mut c = ChannelLease::begin(d.clone(), Purpose::Authentication, false).unwrap();
            if opened {
                c.opened(Some(7), 2).unwrap();
            }
            drop(c);
            assert_eq!(io.receipts.lock().unwrap().len(), 1);
            assert!(ChannelLease::begin(d, Purpose::Identity, false).is_err());
        }
    }
    #[test]
    fn qmi_client_survives_logical_channel_close_until_ctl_release_is_confirmed() {
        let (d, io) = fixture();
        let mut c = ChannelLease::begin(d.clone(), Purpose::QmiUim, false).unwrap();
        c.client_allocated(7).unwrap();
        c.channel_open_pending().unwrap();
        c.opened(Some(7), 2).unwrap();
        assert!(c.closed().is_err());
        c.channel_closed_retaining_client().unwrap();
        assert!(c.closed().is_err());
        assert_eq!(io.receipts.lock().unwrap().len(), 1);
        c.client_released().unwrap();
        c.closed().unwrap();
        assert!(io.receipts.lock().unwrap().is_empty());
        assert_eq!(d.sim_channel_status().confirmed_closes, 1);
    }
    #[test]
    fn qmi_client_only_and_failed_release_are_retained() {
        let (d, io) = fixture();
        let mut c = ChannelLease::begin(d.clone(), Purpose::QmiUim, false).unwrap();
        c.client_allocated(7).unwrap();
        c.channel_open_pending().unwrap();
        c.channel_open_rejected().unwrap();
        assert!(c.rejected().is_err());
        drop(c);
        assert_eq!(io.receipts.lock().unwrap().len(), 1);
        assert!(d.sim_channel_status().reconciliation_required);
    }

    #[test]
    fn explicit_rejection_clears_pending_not_an_unrelated_channel() {
        let (d, io) = fixture();
        let mut c = ChannelLease::begin(d.clone(), Purpose::Probe, false).unwrap();
        assert!(ChannelLease::begin(d.clone(), Purpose::Identity, false).is_err());
        c.rejected().unwrap();
        drop(c);
        assert!(io.receipts.lock().unwrap().is_empty());
        assert_eq!(d.sim_ledger.lock().unwrap().rejected_opens, 1);
    }
    #[test]
    fn external_scope_is_visible_without_claiming_a_channel_capacity() {
        let (d, _) = fixture();
        let mut c = ChannelLease::begin(d.clone(), Purpose::Esim, true).unwrap();
        assert!(
            d.sim_ledger
                .lock()
                .unwrap()
                .owner
                .as_ref()
                .unwrap()
                .external_channels_unknown
        );
        c.external_completed().unwrap();
        let s = d.sim_ledger.lock().unwrap();
        assert_eq!(s.confirmed_opens, 0);
        assert_eq!(s.confirmed_closes, 0);
    }
}
