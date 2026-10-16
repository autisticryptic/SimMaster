use super::*;
use std::{path::PathBuf, sync::atomic::{AtomicBool, AtomicUsize, Ordering}};
use tokio::sync::Notify;
use crate::{
    connectivity::core::{access_network::ServingAccessSnapshot, ims_access::ImsAccessDecision},
    hardware::{cellular::{backends::{config::{NativeDeviceConfig, NativeProtocol}, io::NativeIo,
        native::{NativeDevice, NativeFleet, NativeSnapshot}, protocol::{CommandRequest, Tool}, NativeError},
        native_ims::{NativeImsRegistration, NativeImsService}, observations::{ModemObservationProvider, ObservationError}},
        devices::{DeviceKind, transport::TransportFuture}},
    platform::db::Database,
};

/// The real NativeFleet -> NativeDevice -> NativeIo boundary is used by both
/// service and real-router HTTP tests. Only physical IO is synthetic.
pub(crate) struct TestIo {
    pub(crate) calls: AtomicUsize,
    pub(crate) requests: Mutex<Vec<CommandRequest>>,
    pub(crate) delay: AtomicBool,
    pub(crate) entered: Notify,
    pub(crate) release: Notify,
    pub(crate) fail: AtomicBool,
}
impl NativeIo for TestIo {
    fn execute<'a>(&'a self, request: &'a CommandRequest) -> TransportFuture<'a, Result<String, NativeError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.requests.lock().unwrap().push(request.clone());
            let command = request.arguments.last().unwrap().as_str();
            let text = match (request.tool, command) {
                (Tool::Qmi, "--uim-get-card-status") => "Primary GW: slot '1', application '1'",
                (Tool::At, "AT+QCCID") => "+QCCID: 8900000000000000001\r\nOK",
                (Tool::At, "AT+CGMM") => "EC20\r\nOK",
                (Tool::At, "AT+CGMR") => "EC20CEFAGR08A03M4G\r\nOK",
                (Tool::At, "AT+QIMSCFG=\"ims_status\"") => {
                    if self.delay.load(Ordering::Acquire) {
                        self.entered.notify_one();
                        self.release.notified().await;
                    }
                    if self.fail.load(Ordering::Acquire) { return Err(NativeError::CommandFailed("synthetic failure")); }
                    "+QIMSCFG: \"ims_status\",0,0,2,0,1,2\r\nOK"
                }
                _ => panic!("unexpected hardware operation in read-only fixture: {request:?}"),
            };
            Ok(text.to_owned())
        })
    }
    fn verify_owner<'a>(&'a self, _: &'a str) -> TransportFuture<'a, Result<(), NativeError>> { Box::pin(async { Ok(()) }) }
    fn save_receipt(&self, _: &str, _: &[u8], _: bool) -> Result<(), NativeError> { panic!("probe must not write receipt") }
    fn clear_receipt(&self, _: &str) -> Result<(), NativeError> { panic!("probe must not clear receipt") }
}

pub(crate) fn native_fixture() -> (Arc<NativeFleet>, Arc<TestIo>, ModemBinding) {
    static SEQ: AtomicUsize = AtomicUsize::new(1);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let io = Arc::new(TestIo { calls: AtomicUsize::new(0), requests: Mutex::new(Vec::new()),
        delay: AtomicBool::new(false), entered: Notify::new(), release: Notify::new(), fail: AtomicBool::new(false) });
    let device = NativeDevice::new(NativeDeviceConfig {
        hardware_key: format!("native-observation-{seq}"), sysfs_anchor: "/sys/devices/synthetic-native".into(),
        protocol: NativeProtocol::Qmi, control_device: "/dev/synthetic-native-qmi".into(),
        at_device: Some("/dev/synthetic-native-at".into()), sms_reception_enabled: false,
        uim_slot: 1, ims: None, data: None,
    }, io.clone());
    let mut snapshot = NativeSnapshot::default();
    snapshot.identity.iccid = "8900000000000000001".into();
    let binding = device.binding(&snapshot, true);
    (NativeFleet::new(vec![device]).unwrap(), io, binding)
}

struct Fixture {
    registry: Arc<LineRuntimeRegistry>, config: Arc<ConfigManager>, line: Arc<LineRuntime>, io: Arc<TestIo>,
    device: Arc<NativeDevice>,
    paths: PathBuf,
}
impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.paths); } }
impl Fixture {
    async fn new() -> Self {
        let (provider, io, binding) = native_fixture();
        let device = provider.all().into_iter().next().unwrap();
        let paths = std::env::temp_dir().join(format!("simadmin-native-ims-{}-{}", std::process::id(), binding.line_id));
        std::fs::create_dir_all(&paths).unwrap();
        let database = Arc::new(Database::new(paths.join("test.db")).unwrap());
        let config = Arc::new(ConfigManager::try_new(paths.join("test.yaml"), database.clone()).unwrap());
        config.reconcile_line_profiles(&[binding.line_id.clone()]).unwrap();
        let registry = Arc::new(LineRuntimeRegistry::with_config_for_device(config.clone(), database, DeviceKind::Unknown, provider));
        let line = registry.insert_control_test_line(binding).await;
        Self { registry, config, line, io, device, paths }
    }
    async fn probe(&self) -> Result<NativeImsObservationStatus, NativeImsProbeError> {
        probe(self.registry.clone(), self.config.clone(), ShutdownSignal::never(), &self.line.binding().line_id).await
    }
    async fn status(&self) -> NativeImsObservationStatus {
        status(&self.registry, &self.config, &ShutdownSignal::never(), &self.line.binding().line_id).await.unwrap()
    }
    fn clear_cooldown(&self) { self.line.native_ims_observation.state.lock().unwrap().next_probe = None; }
}

#[tokio::test]
async fn native_observation_get_is_zero_io_and_post_uses_real_owned_fleet() {
    let fixture = Fixture::new().await;
    let before = serde_json::to_value(fixture.config.get_line_profile(&fixture.line.binding().line_id)).unwrap();
    for _ in 0..3 {
        let status = fixture.status().await;
        assert_eq!(status.state, ObservationState::NotProbed);
        assert!(status.adapter_available && status.sample.is_none());
    }
    assert_eq!(fixture.io.calls.load(Ordering::Acquire), 0);
    let sample = fixture.probe().await.unwrap();
    assert_eq!(sample.state, ObservationState::Observed);
    assert_eq!(sample.sample.unwrap().registration, NativeImsRegistration::Registered);
    assert_eq!(sample.sample.unwrap().sms, NativeImsService::UnknownOrUnreported);
    assert_eq!(sample.sample.unwrap().ut, NativeImsService::Available);
    assert!(!sample.stale);
    assert_eq!(fixture.io.calls.load(Ordering::Acquire), 7);
    for _ in 0..3 { assert_eq!(fixture.status().await.observed_at, sample.observed_at); }
    assert_eq!(fixture.io.calls.load(Ordering::Acquire), 7);
    assert!(!fixture.line.cellular_ims.status().await.registered);
    assert!(!fixture.line.vowifi.snapshot().await.live_readiness.ims_registered);
    assert_eq!(serde_json::to_value(fixture.config.get_line_profile(&fixture.line.binding().line_id)).unwrap(), before);
    let json = serde_json::to_string(&fixture.status().await).unwrap();
    for secret in ["8900000000000000001", "/dev/", "QIMSCFG", "hardware_key", "scope", "worker_generation"] {
        assert!(!json.contains(secret), "private observation context must not serialize");
    }
}

#[tokio::test]
async fn native_observation_host_intents_activity_and_admission_fail_without_probe() {
    let fixture = Fixture::new().await;
    let line_id = fixture.line.binding().line_id;
    fixture.config.set_line_cellular_ims_connection_enabled(&line_id, true).unwrap();
    assert_eq!(fixture.probe().await.unwrap_err(), NativeImsProbeError::Busy);
    fixture.config.set_line_cellular_ims_connection_enabled(&line_id, false).unwrap();
    fixture.config.set_line_vowifi_connection_enabled(&line_id, true).unwrap();
    assert_eq!(fixture.probe().await.unwrap_err(), NativeImsProbeError::Busy);
    fixture.config.set_line_vowifi_connection_enabled(&line_id, false).unwrap();
    let activity = fixture.line.ims_registration.host_activity().await;
    assert_eq!(fixture.probe().await.unwrap_err(), NativeImsProbeError::Busy);
    drop(activity);
    fixture.clear_cooldown();
    fixture.line.ims_registration.publish(ImsAccessDecision::cellular_only("test-enabled")).await;
    assert_eq!(fixture.probe().await.unwrap_err(), NativeImsProbeError::Busy);
    assert_eq!(fixture.io.calls.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn native_observation_expiry_and_failure_never_refresh_old_sample_age() {
    let fixture = Fixture::new().await;
    let accepted = fixture.probe().await.unwrap();
    {
        let mut state = fixture.line.native_ims_observation.state.lock().unwrap();
        state.sample.as_mut().unwrap().at = Instant::now() - OBSERVATION_TTL - Duration::from_secs(1);
        state.next_probe = None;
    }
    let stale = fixture.status().await;
    assert_eq!(stale.state, ObservationState::Stale);
    assert!(stale.stale && stale.age_seconds.unwrap() >= 31);
    fixture.io.fail.store(true, Ordering::Release);
    assert_eq!(fixture.probe().await.unwrap_err(), NativeImsProbeError::Unavailable);
    let failed = fixture.status().await;
    assert_eq!(failed.observed_at, accepted.observed_at);
    assert!(failed.sample.is_some() && failed.stale);
    assert_eq!(failed.last_error, Some("native_ims_unavailable"));
}

#[tokio::test]
async fn native_observation_same_sim_reappearance_host_epoch_and_worker_change_invalidate_cache() {
    let fixture = Fixture::new().await;
    fixture.probe().await.unwrap();
    let original = fixture.line.binding();
    let mut absent = original.clone(); absent.present = false;
    fixture.line.replace_native_observation_test_binding(absent);
    fixture.line.replace_native_observation_test_binding(original.clone());
    assert!(fixture.status().await.sample.is_none(), "same-SIM reappearance must not resurrect old evidence");
    fixture.clear_cooldown(); fixture.probe().await.unwrap();
    let activity = fixture.line.ims_registration.host_activity().await; drop(activity);
    assert!(fixture.status().await.sample.is_none());
    fixture.clear_cooldown(); fixture.probe().await.unwrap();
    fixture.line.ue_worker.advance_test_generation().await;
    assert!(fixture.status().await.sample.is_none());
    fixture.clear_cooldown(); fixture.probe().await.unwrap();
    fixture.line.vowifi.reset_runtime("test reset").await;
    assert!(fixture.status().await.sample.is_none());
}

#[tokio::test]
async fn native_observation_late_result_cannot_cross_binding_or_enable_changes() {
    for change_binding in [false, true] {
        let fixture = Fixture::new().await;
        fixture.io.delay.store(true, Ordering::Release);
        let registry = fixture.registry.clone(); let config = fixture.config.clone(); let id = fixture.line.binding().line_id;
        let caller = tokio::spawn(async move { probe(registry, config, ShutdownSignal::never(), &id).await });
        tokio::time::timeout(Duration::from_secs(2), fixture.io.entered.notified()).await.unwrap();
        if change_binding {
            let mut other = fixture.line.binding(); other.sim_iccid = "8900000000000000002".into();
            fixture.line.replace_native_observation_test_binding(other);
        } else {
            fixture.config.set_line_cellular_ims_connection_enabled(&fixture.line.binding().line_id, true).unwrap();
        }
        fixture.io.release.notify_one();
        assert_eq!(caller.await.unwrap().unwrap_err(), NativeImsProbeError::BindingChanged);
        assert!(fixture.status().await.sample.is_none());
    }
}

#[tokio::test]
async fn native_observation_cancelled_request_holds_exclusion_and_never_queues_another_probe() {
    let fixture = Fixture::new().await;
    fixture.io.delay.store(true, Ordering::Release);
    let registry = fixture.registry.clone(); let config = fixture.config.clone(); let id = fixture.line.binding().line_id;
    let caller = tokio::spawn(async move { probe(registry, config, ShutdownSignal::never(), &id).await });
    tokio::time::timeout(Duration::from_secs(2), fixture.io.entered.notified()).await.unwrap();
    caller.abort(); let _ = caller.await;
    assert!(fixture.line.ims_registration.try_native_observation().is_err());
    assert!(fixture.line.cellular_ims_connect_lock.try_lock().is_err());
    assert_eq!(fixture.probe().await.unwrap_err(), NativeImsProbeError::Busy);
    assert_eq!(fixture.io.calls.load(Ordering::Acquire), 5);
    fixture.io.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !fixture.line.native_ims_observation.state.lock().unwrap().in_flight { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    assert_eq!(fixture.io.calls.load(Ordering::Acquire), 7);
    assert!(fixture.line.ims_registration.try_native_observation().is_ok());
    assert_eq!(fixture.status().await.state, ObservationState::Observed);
}

#[tokio::test]
async fn native_observation_maintenance_invalidates_cache_without_query_or_owner_reset() {
    let fixture = Fixture::new().await;
    fixture.probe().await.unwrap();
    fixture.device.mark_maintenance_required();
    let status = fixture.status().await;
    assert!(status.adapter_available, "topology support is independent of controller admission");
    assert!(status.sample.is_none() && status.stale);
    assert_eq!(status.last_error, Some("native_ims_unavailable"));
    assert_eq!(fixture.io.calls.load(Ordering::Acquire), 7);
    assert_eq!(fixture.probe().await.unwrap_err(), NativeImsProbeError::Unavailable);
}

#[tokio::test]
async fn native_observation_shutdown_or_missing_line_cannot_probe() {
    let fixture = Fixture::new().await;
    let (controller, shutdown) = crate::platform::shutdown::channel(); controller.trigger();
    assert_eq!(probe(fixture.registry.clone(), fixture.config.clone(), shutdown, &fixture.line.binding().line_id).await.unwrap_err(), NativeImsProbeError::Unavailable);
    assert_eq!(probe(fixture.registry.clone(), fixture.config.clone(), ShutdownSignal::never(), "missing-line").await.unwrap_err(), NativeImsProbeError::BindingChanged);
    assert!(status(&fixture.registry, &fixture.config, &ShutdownSignal::never(), "missing-line").await.is_none());
    assert_eq!(fixture.io.calls.load(Ordering::Acquire), 0);
}

struct Unsupported;
impl ModemObservationProvider for Unsupported {
    fn name(&self) -> &'static str { "unsupported-fixture" }
    fn discover(&self) -> TransportFuture<'_, Result<Vec<ModemBinding>, ObservationError>> { panic!("no implicit discovery") }
    fn serving_access<'a>(&'a self, _: &'a ModemBinding) -> TransportFuture<'a, Result<ServingAccessSnapshot, ObservationError>> { panic!("no implicit serving query") }
}
#[tokio::test]
async fn native_observation_unknown_provider_keeps_cached_get_inert() {
    let registry = Arc::new(LineRuntimeRegistry::new(DeviceKind::Unknown, Arc::new(Unsupported)));
    let fixture = Fixture::new().await;
    let binding = fixture.line.binding();
    registry.insert_control_test_line(binding.clone()).await;
    let status = status(&registry, &fixture.config, &ShutdownSignal::never(), &binding.line_id).await.unwrap();
    assert!(!status.adapter_available && status.sample.is_none());
    assert_eq!(probe(registry, fixture.config.clone(), ShutdownSignal::never(), &binding.line_id).await.unwrap_err(), NativeImsProbeError::Unsupported);
}
