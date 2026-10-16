use super::*;
use std::{collections::VecDeque, sync::{atomic::{AtomicBool, AtomicUsize, Ordering}, Mutex}};
use tokio::sync::Notify;
use crate::{
    connectivity::core::access_network::ServingAccessSnapshot,
    hardware::{cellular::{
        backends::{config::NativeDeviceConfig, io::NativeIo, native::{NativeFleet, NativeSnapshot}, protocol::{CommandRequest, Tool}},
        observations::{ModemObservationProvider, ObservationError},
        native_ims::{NativeImsRegistration, NativeImsService},
    }, devices::transport::TransportFuture},
};

const SLOT: &str = "--uim-get-card-status";
const SLOT_REPLY: &str = "Primary GW: slot '1', application '1'";
const ICCID: &str = "+QCCID: 8900000000000000001\r\nOK";
const OTHER_ICCID: &str = "+QCCID: 8900000000000000002\r\nOK";
const STATUS: &str = "+QIMSCFG:\"ims_status\",0,1,2,0,1,2\r\nOK";
const CONTROL: &str = "/dev/synthetic-qmi";
const AT: &str = "/dev/synthetic-at";

struct ScriptedIo {
    steps: Mutex<VecDeque<(&'static str, Result<&'static str, NativeError>)>>,
    requests: Mutex<Vec<CommandRequest>>,
    owners: Mutex<Vec<String>>,
    fail_owner: AtomicUsize,
    invalidated: AtomicBool,
    delay_status: AtomicBool,
    entered: Notify,
    release: Notify,
}

impl ScriptedIo {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new([
                (SLOT, Ok(SLOT_REPLY)), (ec20::ICCID_COMMAND, Ok(ICCID)),
                (ec20::MODEL_COMMAND, Ok("EC20\r\nOK")),
                (ec20::REVISION_COMMAND, Ok("EC20CEFAGR08A03M4G\r\nOK")),
                (ec20::STATUS_COMMAND, Ok(STATUS)), (ec20::ICCID_COMMAND, Ok(ICCID)),
                (SLOT, Ok(SLOT_REPLY)),
            ].into()),
            requests: Default::default(), owners: Default::default(),
            fail_owner: AtomicUsize::new(0), invalidated: AtomicBool::new(false),
            delay_status: AtomicBool::new(false), entered: Notify::new(), release: Notify::new(),
        })
    }

    fn replace(&self, index: usize, reply: Result<&'static str, NativeError>) {
        self.steps.lock().unwrap()[index].1 = reply;
    }

    fn count(&self) -> usize { self.requests.lock().unwrap().len() }
}

impl NativeIo for ScriptedIo {
    fn invalidate(&self) { self.invalidated.store(true, Ordering::Release); }

    fn execute<'a>(&'a self, request: &'a CommandRequest) -> TransportFuture<'a, Result<String, NativeError>> {
        Box::pin(async move {
            assert!(!self.invalidated.load(Ordering::Acquire), "commands after invalidation");
            self.requests.lock().unwrap().push(request.clone());
            let (expected, reply) = self.steps.lock().unwrap().pop_front().expect("unexpected native IO/retry");
            if expected == SLOT {
                assert_eq!(request.tool, Tool::Qmi);
                assert_eq!(request.device, CONTROL);
                assert_eq!(request.arguments, ["-d", CONTROL, "--device-open-proxy", SLOT]);
                assert_eq!(request.timeout_seconds, 20);
            } else {
                assert_eq!(request.tool, Tool::At);
                assert_eq!(request.device, AT);
                assert_eq!(request.arguments, [expected]);
                assert_eq!(request.timeout_seconds, AT_BUDGET_SECONDS);
            }
            if expected == ec20::STATUS_COMMAND && self.delay_status.load(Ordering::Acquire) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            reply.map(str::to_string)
        })
    }

    fn verify_owner<'a>(&'a self, device: &'a str) -> TransportFuture<'a, Result<(), NativeError>> {
        Box::pin(async move {
            let index = {
                let mut owners = self.owners.lock().unwrap();
                owners.push(device.to_string());
                owners.len()
            };
            assert_eq!(device, if index % 2 == 1 { CONTROL } else { AT });
            if self.fail_owner.load(Ordering::Acquire) == index || self.invalidated.load(Ordering::Acquire) {
                Err(NativeError::OwnerConflict("synthetic-private-port-generation".into()))
            } else {
                Ok(())
            }
        })
    }

    fn save_receipt(&self, _: &str, _: &[u8], _: bool) -> Result<(), NativeError> {
        panic!("read-only probe must not save receipts")
    }
    fn clear_receipt(&self, _: &str) -> Result<(), NativeError> {
        panic!("read-only probe must not clear receipts")
    }
}

fn spec() -> NativeDeviceConfig {
    NativeDeviceConfig {
        hardware_key: "synthetic-native-ims".into(), sysfs_anchor: "/sys/synthetic-native-ims".into(),
        protocol: NativeProtocol::Qmi, control_device: CONTROL.into(), at_device: Some(AT.into()),
        sms_reception_enabled: false, uim_slot: 1, ims: None, data: None,
    }
}

fn fixture_with(spec: NativeDeviceConfig, io: Arc<ScriptedIo>) -> (Arc<NativeFleet>, Arc<NativeDevice>, ModemBinding) {
    let device = NativeDevice::new(spec, io);
    let mut snapshot = NativeSnapshot::default();
    snapshot.identity.iccid = "8900000000000000001".into();
    // A cached model string must never qualify the live firmware query.
    snapshot.model = "untrusted_cached_model".into();
    let binding = device.binding(&snapshot, true);
    let fleet = NativeFleet::new(vec![device.clone()]).unwrap();
    (fleet, device, binding)
}

fn fixture(io: Arc<ScriptedIo>) -> (Arc<NativeFleet>, Arc<NativeDevice>, ModemBinding) {
    fixture_with(spec(), io)
}

#[tokio::test]
async fn native_ims_real_fleet_performs_only_the_exact_owned_read_sequence() {
    let io = ScriptedIo::new();
    let (fleet, device, binding) = fixture(io.clone());
    assert!(fleet.supports_native_ims_probe(&binding));
    assert_eq!(io.count(), 0);
    assert!(io.owners.lock().unwrap().is_empty());
    let sample = fleet.probe_native_ims(&binding).await.unwrap();
    assert_eq!(sample.registration, NativeImsRegistration::Registered);
    assert_eq!(sample.sms, NativeImsService::UnknownOrUnreported);
    assert_eq!(sample.voice, NativeImsService::Limited);
    assert_eq!(sample.ut, NativeImsService::Available);
    assert_eq!(io.count(), 7);
    assert!(io.steps.lock().unwrap().is_empty());
    assert_eq!(*io.owners.lock().unwrap(), [CONTROL, AT, CONTROL, AT]);
    assert!(!io.invalidated.load(Ordering::Acquire));
    assert!(device.operation.try_lock().is_ok());
    assert!(device.sim_channel_status().owner.is_none());
    assert!(!device.sim_channel_status().reconciliation_required);
    assert!(device.active_interfaces.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_ims_static_support_is_not_model_or_firmware_qualification() {
    let io = ScriptedIo::new();
    io.replace(2, Ok("not-EC20\r\nOK"));
    let (fleet, _, mut binding) = fixture(io.clone());
    binding.model = "EC20".into();
    binding.manufacturer = "Quectel".into();
    assert!(fleet.supports_native_ims_probe(&binding));
    assert_eq!(io.count(), 0);
    assert_eq!(fleet.probe_native_ims(&binding).await, Err(NativeImsProbeError::FirmwareNotQualified));
    assert_eq!(io.count(), 4);
}

#[tokio::test]
async fn native_ims_wrong_or_ambiguous_firmware_never_sends_status() {
    for (index, reply) in [
        (2, "EC25\r\nOK"), (2, "EC20\r\nEC20\r\nOK"),
        (3, "EC20CEFAGR08A04M4G\r\nOK"), (3, "OK"),
    ] {
        let io = ScriptedIo::new();
        io.replace(index, Ok(reply));
        let (fleet, _, binding) = fixture(io.clone());
        assert_eq!(fleet.probe_native_ims(&binding).await, Err(NativeImsProbeError::FirmwareNotQualified));
        assert_eq!(io.count(), 4);
    }
}

#[tokio::test]
async fn native_ims_missing_at_or_unproved_protocol_is_unsupported_without_io() {
    for (protocol, at_device) in [
        (NativeProtocol::Qmi, None), (NativeProtocol::Qmi, Some(String::new())),
        (NativeProtocol::Mbim, Some(AT.into())), (NativeProtocol::At, Some(AT.into())),
    ] {
        let io = ScriptedIo::new();
        let mut config = spec();
        config.protocol = protocol;
        config.at_device = at_device;
        let (fleet, _, binding) = fixture_with(config, io.clone());
        assert!(!fleet.supports_native_ims_probe(&binding));
        assert_eq!(fleet.probe_native_ims(&binding).await, Err(NativeImsProbeError::Unsupported));
        assert_eq!(io.count(), 0);
        assert!(io.owners.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_ims_wrong_binding_selector_slot_port_or_missing_identity_is_rejected_before_io() {
    for change in 0..13 {
        let io = ScriptedIo::new();
        let (fleet, _, mut binding) = fixture(io.clone());
        match change {
            0 => binding.hardware_key = "different".into(),
            1 => binding.line_id = "different".into(),
            2 => binding.modem_path = "native:unowned".into(),
            3 => binding.modem_id = "native:unowned".into(),
            4 => binding.uim_slot = 2,
            5 => binding.primary_port = "/dev/other".into(),
            6 => binding.qmi_device = None,
            7 => binding.slot_conflict = true,
            8 => binding.slot_stable = false,
            9 => binding.present = false,
            10 => binding.sim_iccid.clear(),
            11 => binding.sim_iccid = "private-not-an-iccid".into(),
            _ => binding.line_kind = "unknown".into(),
        }
        assert_eq!(fleet.probe_native_ims(&binding).await, Err(NativeImsProbeError::BindingChanged));
        assert_eq!(io.count(), 0);
        assert!(io.owners.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_ims_mm_reader_and_other_fleet_never_fall_back_to_global_controller() {
    let io = ScriptedIo::new();
    let (fleet, _, binding) = fixture(io.clone());
    let empty = NativeFleet::new(vec![]).unwrap();
    assert!(!empty.supports_native_ims_probe(&binding));
    assert_eq!(empty.probe_native_ims(&binding).await, Err(NativeImsProbeError::BindingChanged));
    for reader in [false, true] {
        let mut binding = binding.clone();
        if reader { binding.line_kind = "reader".into(); }
        else { binding.modem_path = "/org/freedesktop/ModemManager1/Modem/0".into(); }
        assert!(!fleet.supports_native_ims_probe(&binding));
        assert_eq!(fleet.probe_native_ims(&binding).await, Err(NativeImsProbeError::Unsupported));
    }
    assert_eq!(io.count(), 0);
}

#[tokio::test]
async fn native_ims_fresh_primary_slot_and_iccid_pre_post_races_discard_the_sample() {
    for (index, reply, count) in [
        (0, "Primary GW: slot '2', application '1'", 1),
        (0, "Primary GW: unknown", 1),
        (0, "Primary GW: slot '1", 1),
        (0, "Primary GW: slot '1', application '1'\nPrimary GW: slot '2', application '1'", 1),
        (1, OTHER_ICCID, 2), (1, "OK", 2),
        (5, OTHER_ICCID, 6), (5, "OK", 6),
        (6, "Primary GW: slot '2', application '1'", 7),
    ] {
        let io = ScriptedIo::new();
        io.replace(index, Ok(reply));
        let (fleet, _, binding) = fixture(io.clone());
        assert_eq!(fleet.probe_native_ims(&binding).await, Err(NativeImsProbeError::BindingChanged));
        assert_eq!(io.count(), count);
    }
}

#[tokio::test]
async fn native_ims_owner_or_port_generation_changes_are_checked_on_both_sides() {
    for fail_at in 1..=4 {
        let io = ScriptedIo::new();
        io.fail_owner.store(fail_at, Ordering::Release);
        let (fleet, _, binding) = fixture(io.clone());
        let error = fleet.probe_native_ims(&binding).await.unwrap_err();
        assert_eq!(error, NativeImsProbeError::BindingChanged);
        assert_eq!(error.to_string(), "native_ims_binding_changed");
        assert_eq!(io.count(), if fail_at <= 2 { 0 } else { 7 });
    }
    let io = ScriptedIo::new();
    io.replace(4, Err(NativeError::OwnerConflict("private-physical-generation".into())));
    let (fleet, _, binding) = fixture(io.clone());
    assert_eq!(fleet.probe_native_ims(&binding).await, Err(NativeImsProbeError::BindingChanged));
    assert_eq!(io.count(), 5);
}

#[tokio::test]
async fn native_ims_transport_errors_and_malformed_status_have_no_retry() {
    for index in 0..7 {
        let io = ScriptedIo::new();
        io.replace(index, Err(NativeError::CommandFailed("synthetic-private-error")));
        let (fleet, _, binding) = fixture(io.clone());
        assert_eq!(fleet.probe_native_ims(&binding).await, Err(NativeImsProbeError::Unavailable));
        assert_eq!(io.count(), index + 1);
    }
    for reply in ["OK", "ERROR", "+QIMSCFG:\"ims_status\",0,0,2,2,2\r\nOK"] {
        let io = ScriptedIo::new();
        io.replace(4, Ok(reply));
        let (fleet, _, binding) = fixture(io.clone());
        assert_eq!(fleet.probe_native_ims(&binding).await, Err(NativeImsProbeError::MalformedResponse));
        assert_eq!(io.count(), 5);
    }
}

#[tokio::test]
async fn native_ims_operation_contention_is_busy_without_queuing_or_io() {
    let io = ScriptedIo::new();
    let (fleet, device, binding) = fixture(io.clone());
    let _guard = device.operation.lock().await;
    assert_eq!(fleet.probe_native_ims(&binding).await, Err(NativeImsProbeError::Busy));
    assert_eq!(io.count(), 0);
}

#[tokio::test]
async fn native_ims_maintenance_and_unresolved_sim_ownership_fail_closed() {
    for maintenance in [true, false] {
        let io = ScriptedIo::new();
        let (fleet, device, binding) = fixture(io.clone());
        if maintenance { device.mark_maintenance_required(); }
        else { device.sim_ledger.lock().unwrap().reconciliation_required = true; }
        assert!(fleet.probe_native_ims(&binding).await.is_err());
        assert_eq!(io.count(), 0);
    }
}

#[tokio::test]
async fn native_ims_caller_cancellation_keeps_operation_until_io_and_postchecks_finish() {
    let io = ScriptedIo::new();
    io.delay_status.store(true, Ordering::Release);
    let (fleet, device, binding) = fixture(io.clone());
    let caller_fleet = fleet.clone();
    let caller_binding = binding.clone();
    let caller = tokio::spawn(async move { caller_fleet.probe_native_ims(&caller_binding).await });
    tokio::time::timeout(Duration::from_secs(2), io.entered.notified()).await.unwrap();
    caller.abort();
    let _ = caller.await;
    assert!(device.operation.try_lock().is_err());
    assert_eq!(fleet.probe_native_ims(&binding).await, Err(NativeImsProbeError::Busy));
    assert_eq!(io.count(), 5);
    io.release.notify_one();
    let guard = tokio::time::timeout(Duration::from_secs(2), device.operation.lock()).await.unwrap();
    assert_eq!(io.count(), 7);
    assert_eq!(io.owners.lock().unwrap().len(), 4);
    drop(guard);
}

#[tokio::test]
async fn native_ims_invalidation_while_status_is_inflight_never_returns_or_continues_sample() {
    let io = ScriptedIo::new();
    io.delay_status.store(true, Ordering::Release);
    let (fleet, device, binding) = fixture(io.clone());
    let caller = tokio::spawn(async move { fleet.probe_native_ims(&binding).await });
    tokio::time::timeout(Duration::from_secs(2), io.entered.notified()).await.unwrap();
    device.mark_maintenance_required();
    assert!(device.operation.try_lock().is_err());
    io.release.notify_one();
    assert_eq!(caller.await.unwrap(), Err(NativeImsProbeError::Unavailable));
    assert_eq!(io.count(), 5);
    assert!(device.operation.try_lock().is_ok());
}

#[tokio::test]
async fn native_ims_expired_transaction_starts_no_new_io() {
    let io = ScriptedIo::new();
    let (_, device, _) = fixture(io.clone());
    let _guard = device.operation.lock().await;
    assert_eq!(device.native_ims_transaction("8900000000000000001", Instant::now()).await,
        Err(NativeImsProbeError::Unavailable));
    assert_eq!(io.count(), 0);
}

struct DefaultProvider;
impl ModemObservationProvider for DefaultProvider {
    fn name(&self) -> &'static str { "synthetic-default" }
    fn discover(&self) -> TransportFuture<'_, Result<Vec<ModemBinding>, ObservationError>> {
        panic!("not discovery")
    }
    fn serving_access<'a>(&'a self, _: &'a ModemBinding) -> TransportFuture<'a, Result<ServingAccessSnapshot, ObservationError>> {
        panic!("not serving access")
    }
}

#[tokio::test]
async fn native_ims_default_provider_is_inert_and_unsupported() {
    let binding = ModemBinding::default();
    let provider: Arc<dyn ModemObservationProvider> = Arc::new(DefaultProvider);
    assert!(!provider.supports_native_ims_probe(&binding));
    assert_eq!(provider.probe_native_ims(&binding).await, Err(NativeImsProbeError::Unsupported));
}
