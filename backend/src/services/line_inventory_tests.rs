//! These fixtures deliberately have no config, bus, transport, or runtime.
use super::*;
use crate::hardware::devices::transport::TransportFuture;
use std::sync::atomic::AtomicUsize;

struct PassiveObservations {
    calls: AtomicUsize,
    result: Result<Vec<PassiveModemInventory>, ObservationError>,
}

impl ModemObservationProvider for PassiveObservations {
    fn name(&self) -> &'static str { "passive-fixture" }

    fn discover(&self) -> TransportFuture<'_, Result<Vec<ModemBinding>, ObservationError>> {
        panic!("operational discovery could open a SIM channel")
    }

    fn discover_passive(&self) -> TransportFuture<'_, Result<Vec<PassiveModemInventory>, ObservationError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { self.result.clone() })
    }

    fn serving_access<'a>(&'a self, _: &'a ModemBinding) -> TransportFuture<'a, Result<crate::connectivity::core::access_network::ServingAccessSnapshot, ObservationError>> {
        panic!("display inventory must not sample serving state")
    }
}

fn card() -> PassiveModemInventory {
    PassiveModemInventory {
        line_id: "line-passive-slot".into(),
        manufacturer: "fixture".into(),
        model: "no SIM modem".into(),
        slot_source: "physdev".into(),
        slot_stable: true,
        uim_slot: 1,
        present: true,
        sim_missing: Some(true),
        observation_source: "modemmanager_cache",
    }
}

#[tokio::test]
async fn failed_gate_shows_no_sim_hardware_without_admitting_any_runtime() {
    let observations = Arc::new(PassiveObservations {
        calls: AtomicUsize::new(0), result: Ok(vec![card()]),
    });
    let registry = LineRuntimeRegistry::new(DeviceKind::Unknown, observations.clone());
    registry.defer_ims_startup_recovery().await;
    assert_eq!(registry.ims_startup_gate.ensure_ready(|| async { false }).await,
        Err("ims_startup_recovery_pending"));
    let (reason, inventory) = registry.passive_inventory_if_blocked().await.unwrap().unwrap();
    assert_eq!(reason, "ims_startup_recovery_pending");
    assert_eq!(inventory, vec![card()]);
    assert!(inventory[0].present);
    assert_eq!(inventory[0].sim_missing, Some(true));
    assert_eq!(observations.calls.load(Ordering::SeqCst), 1);
    assert!(registry.get(&inventory[0].line_id).await.is_none());
    assert!(registry.all().await.is_empty());
    assert!(registry.statuses().await.is_empty());
    assert_eq!(registry.ims_startup_gate.blocked_reason().await, Some(reason));
    // No synthetic namespace/worker/UE/transport is ever constructed: there is
    // no LineRuntime or ModemBinding in this fixture or display result.
}

#[tokio::test]
async fn passive_failure_is_not_empty_inventory_or_recovery_success() {
    let registry = LineRuntimeRegistry::new(DeviceKind::Unknown, Arc::new(PassiveObservations {
        calls: AtomicUsize::new(0),
        result: Err(ObservationError::Transient("fixture_bus_unavailable".into())),
    }));
    registry.defer_ims_startup_recovery().await;
    assert_eq!(registry.passive_inventory_if_blocked().await.unwrap(),
        Err(ObservationError::Transient("fixture_bus_unavailable".into())));
    assert!(registry.get("line-passive-slot").await.is_none());
    assert!(registry.ims_startup_gate.blocked_reason().await.is_some());
}

struct ReadyObservations(AtomicUsize);

impl ModemObservationProvider for ReadyObservations {
    fn name(&self) -> &'static str { "ready-fixture" }
    fn discover(&self) -> TransportFuture<'_, Result<Vec<ModemBinding>, ObservationError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Vec::new()) })
    }
    fn serving_access<'a>(&'a self, _: &'a ModemBinding) -> TransportFuture<'a, Result<crate::connectivity::core::access_network::ServingAccessSnapshot, ObservationError>> {
        panic!("empty inventory cannot sample serving state")
    }
}

#[tokio::test]
async fn ready_gate_retains_ordinary_discovery_and_never_calls_passive_provider() {
    let observations = Arc::new(ReadyObservations(AtomicUsize::new(0)));
    let registry = LineRuntimeRegistry::new(DeviceKind::Unknown, observations.clone());
    // ReadyObservations has only the default-unsupported passive method.
    assert!(registry.passive_inventory_if_blocked().await.is_none());
    assert_eq!(registry.refresh().await.unwrap(), 0);
    assert_eq!(observations.0.load(Ordering::SeqCst), 1);
    registry.defer_ims_startup_recovery().await;
    assert_eq!(registry.passive_inventory_if_blocked().await.unwrap(),
        Err(ObservationError::Unavailable("passive_inventory_unsupported".into())));
    assert_eq!(observations.0.load(Ordering::SeqCst), 1);
    assert_eq!(registry.ims_startup_gate.ensure_ready(|| async { true }).await, Ok(true));
    assert!(registry.passive_inventory_if_blocked().await.is_none());
    assert_eq!(registry.refresh().await.unwrap(), 0);
    assert_eq!(observations.0.load(Ordering::SeqCst), 2);
}

#[test]
fn blocked_api_has_no_operational_data_profile_or_fake_disabled_runtime() {
    use crate::api::models::{ApiResponse, LineInventoryResponse};
    let response = LineInventoryResponse::<serde_json::Value>::blocked(
        "ims_startup_recovery_pending", vec![card()],
    );
    let json = serde_json::to_value(response).unwrap();
    assert_eq!(json["data"], serde_json::json!([]));
    assert_eq!(json["display_only_lines"][0]["present"], true);
    assert_eq!(json["display_only_lines"][0]["sim_missing"], true);
    assert_eq!(json["blocked_reason"], "ims_startup_recovery_pending");
    let text = json.to_string();
    for forbidden in ["profile", "runtime", "enabled", "modem_path", "qmi_device"] {
        assert!(!text.contains(forbidden), "{forbidden}");
    }
    let ordinary = ApiResponse::success_with_message("Success", vec![1]);
    let wrapped: LineInventoryResponse<i32> = ordinary.into();
    assert_eq!(serde_json::to_value(wrapped).unwrap(),
        serde_json::json!({"status": "ok", "message": "Success", "data": [1]}));
}
