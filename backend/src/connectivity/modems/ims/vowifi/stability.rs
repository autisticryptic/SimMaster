#![allow(dead_code)]

use std::panic::{catch_unwind, AssertUnwindSafe};

use chrono::NaiveDate;
use serde::Serialize;

use super::{
    dataplane, epdg,
    executor::{DryRunRuntimeExecutor, RuntimeExecutor, RuntimeExecutorReport},
    ike, ims,
    live::{live_stage_implemented, live_transport_implemented},
    profiles::{self, CarrierProfile},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VowifiAuditCheck {
    pub check_id: &'static str,
    pub status: &'static str,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VowifiProfileAuditEntry {
    pub profile_id: &'static str,
    pub plmn: &'static str,
    pub country_iso2: &'static str,
    pub brand: &'static str,
    pub offline_plan_ready: bool,
    pub dry_run_ready: bool,
    pub live_test_ready: bool,
    pub blockers: Vec<&'static str>,
    pub checks: Vec<VowifiAuditCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VowifiLongRunGate {
    pub gate_id: &'static str,
    pub status: &'static str,
    pub target: &'static str,
    pub evidence: &'static str,
    pub blocker: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VowifiLiveStageReadiness {
    pub stage_id: &'static str,
    pub component: &'static str,
    pub status: &'static str,
    pub offline_ready: bool,
    pub dry_run_ready: bool,
    pub live_network_required: bool,
    pub device_state_change_required: bool,
    pub live_network_authorized: bool,
    pub device_state_changes_authorized: bool,
    pub implementation_ready: bool,
    pub evidence: &'static str,
    pub next_step: &'static str,
    pub blockers: Vec<&'static str>,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VowifiSoakScenarioPlan {
    pub scenario_id: &'static str,
    pub status: &'static str,
    pub duration_hours: u16,
    pub sample_interval_seconds: u16,
    pub live_network_required: bool,
    pub device_state_change_required: bool,
    pub sms_test_required: bool,
    pub metrics: &'static [&'static str],
    pub pass_criteria: &'static [&'static str],
    pub evidence_source: &'static str,
    pub blockers: Vec<&'static str>,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VowifiReadinessAuditReport {
    pub stage: &'static str,
    pub clean_room_policy: &'static str,
    pub live_network_allowed: bool,
    pub device_state_changes_allowed: bool,
    pub profile_count: usize,
    pub profiles_ready: usize,
    pub dry_run_profiles_ready: usize,
    pub live_profiles_ready: usize,
    pub long_run_gate_count: usize,
    pub long_run_ready_count: usize,
    pub live_stage_count: usize,
    pub live_stage_ready_count: usize,
    pub soak_scenario_count: usize,
    pub soak_scenario_ready_count: usize,
    pub profile_audits: Vec<VowifiProfileAuditEntry>,
    pub long_run_gates: Vec<VowifiLongRunGate>,
    pub live_stage_readiness: Vec<VowifiLiveStageReadiness>,
    pub soak_scenarios: Vec<VowifiSoakScenarioPlan>,
    pub blockers: Vec<&'static str>,
    pub sensitive_values_policy: &'static str,
}

pub fn build_readiness_audit_report(
    executor: &RuntimeExecutorReport,
) -> VowifiReadinessAuditReport {
    let live_network_allowed = executor.live_network_allowed;
    let device_state_changes_allowed = executor.device_state_changes_allowed;
    let profile_audits = profiles::published_database_profiles()
        .into_iter()
        .map(|profile| audit_profile(profile, live_network_allowed, device_state_changes_allowed))
        .collect::<Vec<_>>();

    let profile_count = profile_audits.len();
    let profiles_ready = profile_audits
        .iter()
        .filter(|entry| entry.offline_plan_ready)
        .count();
    let dry_run_profiles_ready = profile_audits
        .iter()
        .filter(|entry| entry.dry_run_ready)
        .count();
    let live_profiles_ready = profile_audits
        .iter()
        .filter(|entry| entry.live_test_ready)
        .count();
    let long_run_gates = long_run_gates(
        profile_count,
        profiles_ready,
        dry_run_profiles_ready,
        live_network_allowed,
        device_state_changes_allowed,
    );
    let long_run_ready_count = long_run_gates
        .iter()
        .filter(|gate| gate.status == "pass")
        .count();
    let live_stage_readiness = live_stage_readiness(
        executor,
        profile_count,
        profiles_ready,
        dry_run_profiles_ready,
    );
    let live_stage_count = live_stage_readiness.len();
    let live_stage_ready_count = live_stage_readiness
        .iter()
        .filter(|stage| stage.status == "ready")
        .count();
    let soak_scenarios = soak_scenarios(
        executor,
        profile_count,
        profiles_ready,
        dry_run_profiles_ready,
    );
    let soak_scenario_count = soak_scenarios.len();
    let soak_scenario_ready_count = soak_scenarios
        .iter()
        .filter(|scenario| scenario.status == "ready")
        .count();
    let mut blockers = Vec::new();
    if profiles_ready != profile_count {
        blockers.push("profile_offline_plan_audit_failed");
    }
    if dry_run_profiles_ready != profile_count {
        blockers.push("profile_dry_run_audit_failed");
    }
    if !live_network_allowed {
        blockers.push("live_network_executor_disabled");
    }
    if !device_state_changes_allowed {
        blockers.push("device_state_change_executor_disabled");
    }
    if !long_run_gates.iter().all(|gate| gate.status == "pass") {
        blockers.push("long_run_soak_not_completed");
    }

    VowifiReadinessAuditReport {
        stage: "m10_multi_carrier_stability",
        clean_room_policy: "public_profile_metadata_and_simadmin_owned_runtime_checks_only",
        live_network_allowed,
        device_state_changes_allowed,
        profile_count,
        profiles_ready,
        dry_run_profiles_ready,
        live_profiles_ready,
        long_run_gate_count: long_run_gates.len(),
        long_run_ready_count,
        live_stage_count,
        live_stage_ready_count,
        soak_scenario_count,
        soak_scenario_ready_count,
        profile_audits,
        long_run_gates,
        live_stage_readiness,
        soak_scenarios,
        blockers,
        sensitive_values_policy: "no_identity_or_key_material_serialized",
    }
}

fn audit_profile(
    profile: &'static CarrierProfile,
    live_network_allowed: bool,
    device_state_changes_allowed: bool,
) -> VowifiProfileAuditEntry {
    let mut checks = Vec::new();

    let metadata_ok = metadata_shape_ok(profile);
    checks.push(check(
        "clean_room_profile_metadata",
        metadata_ok,
        format!(
            "profile_id={} plmn={} source_refs={} last_verified={}",
            profile.meta.profile_id,
            profile.meta.plmn,
            profile.meta.source_refs.len(),
            profile.meta.last_verified
        ),
    ));

    let epdg_ready = catch_unwind_bool(|| {
        let plan = epdg::build_connection_plan(profile, None);
        !plan.host.is_empty() && plan.port == 500 && !plan.route_policy.policy_id.is_empty()
    });
    checks.push(check(
        "epdg_plan_builds",
        epdg_ready,
        format!("host={} route=direct_or_policy", profile.epdg.host),
    ));

    let ike_ready = catch_unwind_bool(|| {
        let plan = ike::build_session_plan(profile);
        !plan.ike_proposals.is_empty()
            && !plan.child_sa.esp_proposals.is_empty()
            && plan.dpd_interval_seconds > 0
    });
    checks.push(check(
        "ike_child_sa_plan_builds",
        ike_ready,
        format!(
            "ike_proposals={} esp_proposals={} dpd={}s",
            profile.ikev2.ike_proposals.len(),
            profile.ikev2.esp_proposals.len(),
            profile.ikev2.dpd_interval_seconds
        ),
    ));

    let dataplane_ready = catch_unwind_bool(|| {
        let plan = dataplane::build_dataplane_plan(profile);
        !plan.esp_proposals.is_empty()
            && plan.anti_replay_window >= 32
            && plan.mtu.inner_mtu > 0
            && plan.smoltcp.tcp_enabled
    });
    checks.push(check(
        "userspace_esp_smoltcp_plan_builds",
        dataplane_ready,
        "anti_replay_mtu_and_inner_gateway_metadata_ready".to_string(),
    ));

    let ims_ready = catch_unwind_bool(|| {
        let plan = ims::build_register_plan(profile);
        !plan.domain.is_empty()
            && !plan.security_client_mechanisms.is_empty()
            && plan.transport == "tcp"
    });
    checks.push(check(
        "ims_register_sec_agree_plan_builds",
        ims_ready,
        format!(
            "domain={} mechanisms={}",
            profile.ims.domain,
            profile.ims.register.security_client_mechanisms.len()
        ),
    ));

    let dry_run_ready = catch_unwind_bool(|| {
        let report = DryRunRuntimeExecutor::new(profile).describe();
        report.ike_dry_run.is_some()
            && report.dataplane_dry_run.is_some()
            && report
                .ims_register_dry_run
                .as_ref()
                .map(|snapshot| snapshot.register_200_received)
                .unwrap_or(false)
            && report
                .sms_dry_run
                .as_ref()
                .map(|snapshot| snapshot.sms_ready && snapshot.pending_delivery_count == 0)
                .unwrap_or(false)
            && report
                .esim_restore_dry_run
                .as_ref()
                .map(|snapshot| snapshot.switch_phase == "sms_ready")
                .unwrap_or(false)
    });
    checks.push(check(
        "offline_runtime_dry_run_builds",
        dry_run_ready,
        "ike_esp_ims_sms_restore_snapshots_without_live_io".to_string(),
    ));

    let offline_plan_ready = metadata_ok && epdg_ready && ike_ready && dataplane_ready && ims_ready;
    let live_test_ready =
        offline_plan_ready && dry_run_ready && live_network_allowed && device_state_changes_allowed;
    let mut blockers = Vec::new();
    if !offline_plan_ready {
        blockers.push("offline_plan_incomplete");
    }
    if !dry_run_ready {
        blockers.push("offline_dry_run_incomplete");
    }
    if !live_network_allowed {
        blockers.push("live_network_executor_disabled");
    }
    if !device_state_changes_allowed {
        blockers.push("device_state_change_executor_disabled");
    }

    VowifiProfileAuditEntry {
        profile_id: profile.meta.profile_id,
        plmn: profile.meta.plmn,
        country_iso2: profile.meta.country_iso2,
        brand: profile.meta.brand,
        offline_plan_ready,
        dry_run_ready,
        live_test_ready,
        blockers,
        checks,
    }
}

fn metadata_shape_ok(profile: &CarrierProfile) -> bool {
    let expected_prefix = format!("{}{}", profile.meta.mcc, profile.meta.mnc);
    profile
        .meta
        .profile_id
        .starts_with(profile.meta.country_iso2)
        && profile.meta.profile_id.ends_with(profile.meta.plmn)
        && profile
            .meta
            .profile_id
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        && profile.meta.mcc.len() == 3
        && profile.meta.mnc_len as usize == profile.meta.mnc.len()
        && profile.meta.plmn == expected_prefix
        && !profile.meta.brand.trim().is_empty()
        && !profile.meta.operator_legal_name.trim().is_empty()
        && !profile.meta.aliases.is_empty()
        && !profile.meta.source_refs.is_empty()
        && NaiveDate::parse_from_str(profile.meta.last_verified, "%Y-%m-%d").is_ok()
}

fn long_run_gates(
    profile_count: usize,
    profiles_ready: usize,
    dry_run_profiles_ready: usize,
    live_network_allowed: bool,
    device_state_changes_allowed: bool,
) -> Vec<VowifiLongRunGate> {
    vec![
        VowifiLongRunGate {
            gate_id: "clean_room_profile_registry",
            status: if profile_count > 0 && profiles_ready == profile_count {
                "pass"
            } else {
                "fail"
            },
            target: "all_database_profiles_have_structured_public_metadata",
            evidence: "runtime_database_profile_validation",
            blocker: None,
        },
        VowifiLongRunGate {
            gate_id: "all_profiles_offline_plan_build",
            status: if profiles_ready == profile_count {
                "pass"
            } else {
                "fail"
            },
            target: "epdg_ike_child_sa_esp_ims_plans_build_for_each_profile",
            evidence: "read_only_plan_construction",
            blocker: (profiles_ready != profile_count).then_some("profile_plan_audit_failed"),
        },
        VowifiLongRunGate {
            gate_id: "all_profiles_dry_run_runtime",
            status: if dry_run_profiles_ready == profile_count {
                "pass"
            } else {
                "fail"
            },
            target: "offline_ike_esp_ims_sms_restore_snapshots_for_each_profile",
            evidence: "dry_run_executor_without_live_io",
            blocker: (dry_run_profiles_ready != profile_count).then_some("profile_dry_run_failed"),
        },
        VowifiLongRunGate {
            gate_id: "live_executor_authorization",
            status: if live_network_allowed {
                "pass"
            } else {
                "blocked"
            },
            target: "explicit_user_authorized_live_network_test_window",
            evidence: "runtime_executor_flags",
            blocker: (!live_network_allowed).then_some("live_network_executor_disabled"),
        },
        VowifiLongRunGate {
            gate_id: "device_state_change_authorization",
            status: if device_state_changes_allowed {
                "pass"
            } else {
                "blocked"
            },
            target: "explicit_user_authorized_device_state_change_window",
            evidence: "runtime_executor_flags",
            blocker: (!device_state_changes_allowed)
                .then_some("device_state_change_executor_disabled"),
        },
        VowifiLongRunGate {
            gate_id: "rekey_dpd_and_nat_t_soak",
            status: "planned",
            target: "rekey_dpd_nat_t_keepalive_and_retransmit_survive_24h",
            evidence: "requires_live_runtime_counters",
            blocker: Some("live_runtime_not_enabled"),
        },
        VowifiLongRunGate {
            gate_id: "network_path_recovery_soak",
            status: "planned",
            target: "recover_from_wifi_ip_change_and_udp_path_interruption",
            evidence: "requires_authorized_network_fault_window",
            blocker: Some("fault_injection_not_authorized"),
        },
        VowifiLongRunGate {
            gate_id: "sms_delivery_consistency_soak",
            status: "planned",
            target: "mo_mt_delivery_state_matches_database_fact_source_after_24h",
            evidence: "requires_authorized_sms_test_window",
            blocker: Some("sms_live_test_not_authorized"),
        },
        VowifiLongRunGate {
            gate_id: "task_memory_secret_leak_soak",
            status: "planned",
            target: "no_task_growth_no_heap_drift_no_secret_serialization_after_72h",
            evidence: "requires_live_runtime_metrics_and_heap_sampling",
            blocker: Some("soak_metrics_not_collected"),
        },
    ]
}

fn live_stage_readiness(
    executor: &RuntimeExecutorReport,
    profile_count: usize,
    profiles_ready: usize,
    dry_run_profiles_ready: usize,
) -> Vec<VowifiLiveStageReadiness> {
    let offline_profiles_ready = profile_count > 0 && profiles_ready == profile_count;
    let dry_run_profiles_ready = profile_count > 0 && dry_run_profiles_ready == profile_count;
    let live_network_authorized = executor.live_gate.live_network_authorized;
    let device_state_changes_authorized = executor.live_gate.device_state_changes_authorized;

    vec![
        live_stage(
            "identity_readonly",
            "sim_identity",
            true,
            false,
            false,
            false,
            true,
            "modem_manager_identity_snapshot",
            "keep_readonly_identity_refresh_as_status_source",
            live_network_authorized,
            device_state_changes_authorized,
        ),
        live_stage(
            "profile_match",
            "carrier_profile_registry",
            offline_profiles_ready,
            false,
            false,
            false,
            true,
            "structured_profile_registry_audit",
            "continue_public_metadata_review_for_new_plmns",
            live_network_authorized,
            device_state_changes_authorized,
        ),
        live_stage(
            "qmi_sim_auth",
            "usim_aka_adapter",
            offline_profiles_ready,
            false,
            false,
            true,
            false,
            "logical_channel_policy_defined_no_live_apdu",
            "implement_authorized_qmi_uim_auth_adapter",
            live_network_authorized,
            device_state_changes_authorized,
        ),
        live_stage(
            "epdg_dns",
            "epdg_discovery",
            offline_profiles_ready,
            false,
            true,
            false,
            live_stage_implemented(super::executor::ExecutorStage::Epdg),
            "system_dns_epdg_resolver_wired_behind_live_network_gate",
            "authorize_live_network_window_to collect epdg dns observations",
            live_network_authorized,
            device_state_changes_authorized,
        ),
        live_stage(
            "udp_transport",
            "ike_datagram_transport",
            offline_profiles_ready,
            false,
            true,
            false,
            live_transport_implemented("udp_transport"),
            "udp_500_4500_socket_transport_wired_with_metadata_only_tests",
            "authorize_live_network_window_after ike handshake runner is connected",
            live_network_authorized,
            device_state_changes_authorized,
        ),
        live_stage(
            "ike_handshake",
            "ikev2_eap_aka",
            offline_profiles_ready,
            dry_run_profiles_ready,
            true,
            false,
            live_stage_implemented(super::executor::ExecutorStage::Ike),
            "live_ike_sa_init_runner_wired_without_serializing_handshake_material",
            "complete ike_auth_eap_aka_and_child_sa_install",
            live_network_authorized,
            device_state_changes_authorized,
        ),
        live_stage(
            "child_sa_esp",
            "userspace_esp_dataplane",
            offline_profiles_ready,
            dry_run_profiles_ready,
            true,
            false,
            false,
            "offline_child_sa_esp_smoltcp_snapshot",
            "attach_sa_keys_packet_io_and_inner_gateway",
            live_network_authorized,
            device_state_changes_authorized,
        ),
        live_stage(
            "ims_register",
            "ims_sip_register",
            offline_profiles_ready,
            dry_run_profiles_ready,
            true,
            false,
            false,
            "offline_register_200_and_sec_agree_snapshot",
            "wire_protected_tcp_sip_register_after_child_sa",
            live_network_authorized,
            device_state_changes_authorized,
        ),
        live_stage(
            "sms_over_ims",
            "sms_delivery_runtime",
            offline_profiles_ready,
            dry_run_profiles_ready,
            true,
            true,
            false,
            "offline_mo_mt_delivery_consistency_snapshot",
            "run_authorized_sms_window_and_compare_database_fact_source",
            live_network_authorized,
            device_state_changes_authorized,
        ),
        live_stage(
            "esim_restore",
            "profile_switch_restore",
            offline_profiles_ready,
            dry_run_profiles_ready,
            false,
            true,
            false,
            "offline_switch_restore_retry_snapshot",
            "gate_profile_switch_restore_behind_explicit_user_action",
            live_network_authorized,
            device_state_changes_authorized,
        ),
        live_stage(
            "long_soak",
            "stability_counters",
            offline_profiles_ready,
            false,
            true,
            true,
            false,
            "soak_counter_schema_planned_no_live_collection",
            "collect_24h_72h_rekey_sms_memory_secret_metrics",
            live_network_authorized,
            device_state_changes_authorized,
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
fn live_stage(
    stage_id: &'static str,
    component: &'static str,
    offline_ready: bool,
    dry_run_ready: bool,
    live_network_required: bool,
    device_state_change_required: bool,
    implementation_ready: bool,
    evidence: &'static str,
    next_step: &'static str,
    live_network_authorized: bool,
    device_state_changes_authorized: bool,
) -> VowifiLiveStageReadiness {
    let mut blockers = Vec::new();
    if !offline_ready {
        blockers.push("offline_plan_incomplete");
    }
    if live_network_required && !live_network_authorized {
        blockers.push("live_network_authorization_missing");
    }
    if device_state_change_required && !device_state_changes_authorized {
        blockers.push("device_state_change_authorization_missing");
    }
    if !implementation_ready {
        blockers.push("live_stage_not_implemented");
    }

    let status = if blockers.is_empty() {
        "ready"
    } else if implementation_ready {
        "blocked"
    } else {
        "planned"
    };

    VowifiLiveStageReadiness {
        stage_id,
        component,
        status,
        offline_ready,
        dry_run_ready,
        live_network_required,
        device_state_change_required,
        live_network_authorized,
        device_state_changes_authorized,
        implementation_ready,
        evidence,
        next_step,
        blockers,
        sensitive_values_policy: "stage_metadata_only_no_identity_or_secret_values",
    }
}

fn soak_scenarios(
    executor: &RuntimeExecutorReport,
    profile_count: usize,
    profiles_ready: usize,
    dry_run_profiles_ready: usize,
) -> Vec<VowifiSoakScenarioPlan> {
    let offline_profiles_ready = profile_count > 0 && profiles_ready == profile_count;
    let dry_run_profiles_ready = profile_count > 0 && dry_run_profiles_ready == profile_count;
    let live_network_authorized = executor.live_gate.live_network_authorized;
    let device_state_changes_authorized = executor.live_gate.device_state_changes_authorized;
    let live_runtime_ready = executor.live_gate.implementation_ready
        && executor.live_gate.effective_live_network_allowed
        && executor.live_gate.effective_device_state_changes_allowed;

    vec![
        soak_scenario(
            "register_refresh_soak",
            24,
            60,
            true,
            false,
            false,
            &[
                "register_attempts",
                "register_200_count",
                "register_401_count",
                "sec_agree_failures",
                "service_route_changes",
            ],
            &[
                "register_success_rate_at_least_99_percent",
                "no_unhandled_401_or_sec_agree_loop",
                "sms_ready_restored_after_register_refresh",
            ],
            "runtime_register_counters",
            offline_profiles_ready,
            dry_run_profiles_ready,
            live_network_authorized,
            device_state_changes_authorized,
            live_runtime_ready,
        ),
        soak_scenario(
            "rekey_dpd_nat_t_soak",
            24,
            30,
            true,
            false,
            false,
            &[
                "ike_rekey_count",
                "child_sa_rekey_count",
                "dpd_requests",
                "dpd_timeouts",
                "nat_t_keepalives",
                "retransmit_attempts",
            ],
            &[
                "no_dead_peer_without_recovery",
                "no_sa_reuse_after_rekey",
                "child_sa_forwarding_recovers_after_rekey",
            ],
            "ike_and_dataplane_runtime_counters",
            offline_profiles_ready,
            dry_run_profiles_ready,
            live_network_authorized,
            device_state_changes_authorized,
            live_runtime_ready,
        ),
        soak_scenario(
            "network_path_recovery_soak",
            12,
            15,
            true,
            true,
            false,
            &[
                "outer_path_changes",
                "udp_socket_rebinds",
                "dns_refreshes",
                "ike_retransmit_attempts",
                "ims_re_register_attempts",
            ],
            &[
                "runtime_enters_degraded_during_fault",
                "runtime_recovers_without_stale_sa_reuse",
                "sms_ready_restored_after_path_recovery",
            ],
            "authorized_fault_window_events",
            offline_profiles_ready,
            dry_run_profiles_ready,
            live_network_authorized,
            device_state_changes_authorized,
            live_runtime_ready,
        ),
        soak_scenario(
            "sms_delivery_consistency_soak",
            24,
            30,
            true,
            true,
            true,
            &[
                "mo_submitted",
                "mo_rp_ack",
                "mt_parts_received",
                "mt_reassembly_complete",
                "api_pending_count",
                "db_delivery_state_changes",
            ],
            &[
                "acked_mo_sms_not_left_pending",
                "mt_long_sms_requires_all_parts_before_complete",
                "database_delivery_is_api_fact_source",
            ],
            "sms_delivery_database_and_runtime_events",
            offline_profiles_ready,
            dry_run_profiles_ready,
            live_network_authorized,
            device_state_changes_authorized,
            live_runtime_ready,
        ),
        soak_scenario(
            "esim_restore_race_soak",
            12,
            10,
            false,
            true,
            false,
            &[
                "switch_phase",
                "switch_token",
                "phase_ms",
                "identity_ready",
                "sim_auth_ready",
                "retry_count",
                "degraded_reason",
            ],
            &[
                "old_runtime_teardown_before_profile_enable",
                "apdu_sessions_cleared_before_identity_refresh",
                "context_cancellation_retries_to_sms_ready",
            ],
            "profile_switch_restore_events",
            offline_profiles_ready,
            dry_run_profiles_ready,
            live_network_authorized,
            device_state_changes_authorized,
            live_runtime_ready,
        ),
        soak_scenario(
            "task_memory_secret_leak_soak",
            72,
            60,
            true,
            false,
            false,
            &[
                "task_count",
                "heap_bytes",
                "runtime_event_count",
                "redaction_failures",
                "secret_zeroize_drop_count",
            ],
            &[
                "no_monotonic_task_growth",
                "heap_growth_within_budget_after_steady_state",
                "no_secret_or_identity_fields_in_public_json",
            ],
            "runtime_metrics_and_public_api_serialization_scan",
            offline_profiles_ready,
            dry_run_profiles_ready,
            live_network_authorized,
            device_state_changes_authorized,
            live_runtime_ready,
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
fn soak_scenario(
    scenario_id: &'static str,
    duration_hours: u16,
    sample_interval_seconds: u16,
    live_network_required: bool,
    device_state_change_required: bool,
    sms_test_required: bool,
    metrics: &'static [&'static str],
    pass_criteria: &'static [&'static str],
    evidence_source: &'static str,
    offline_ready: bool,
    dry_run_ready: bool,
    live_network_authorized: bool,
    device_state_changes_authorized: bool,
    live_runtime_ready: bool,
) -> VowifiSoakScenarioPlan {
    let mut blockers = Vec::new();
    if !offline_ready {
        blockers.push("offline_plan_incomplete");
    }
    if !dry_run_ready {
        blockers.push("offline_dry_run_incomplete");
    }
    if live_network_required && !live_network_authorized {
        blockers.push("live_network_authorization_missing");
    }
    if device_state_change_required && !device_state_changes_authorized {
        blockers.push("device_state_change_authorization_missing");
    }
    if sms_test_required {
        blockers.push("sms_live_test_window_not_authorized");
    }
    if !live_runtime_ready {
        blockers.push("live_runtime_executor_not_enabled");
    }

    VowifiSoakScenarioPlan {
        scenario_id,
        status: if blockers.is_empty() {
            "ready"
        } else {
            "planned"
        },
        duration_hours,
        sample_interval_seconds,
        live_network_required,
        device_state_change_required,
        sms_test_required,
        metrics,
        pass_criteria,
        evidence_source,
        blockers,
        sensitive_values_policy: "counters_and_state_names_only_no_payload_or_secret_values",
    }
}

fn check(check_id: &'static str, passed: bool, detail: String) -> VowifiAuditCheck {
    VowifiAuditCheck {
        check_id,
        status: if passed { "pass" } else { "fail" },
        detail,
    }
}

fn catch_unwind_bool(check: impl FnOnce() -> bool) -> bool {
    catch_unwind(AssertUnwindSafe(check)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::ims::vowifi::executor::NoopRuntimeExecutor;

    #[test]
    fn audit_covers_all_profiles_without_live_permissions() {
        let report = build_readiness_audit_report(&NoopRuntimeExecutor.describe());

        assert_eq!(report.stage, "m10_multi_carrier_stability");
        assert_eq!(report.profile_count, profiles::BUILTIN_PROFILES.len());
        assert_eq!(report.profiles_ready, report.profile_count);
        assert_eq!(report.dry_run_profiles_ready, report.profile_count);
        assert_eq!(report.live_profiles_ready, 0);
        assert_eq!(report.live_stage_ready_count, 2);
        assert_eq!(report.soak_scenario_ready_count, 0);
        assert!(report.blockers.contains(&"live_network_executor_disabled"));
        assert!(report
            .long_run_gates
            .iter()
            .any(|gate| gate.gate_id == "all_profiles_dry_run_runtime" && gate.status == "pass"));
        assert!(report
            .live_stage_readiness
            .iter()
            .any(|stage| { stage.stage_id == "identity_readonly" && stage.status == "ready" }));
        assert!(report.live_stage_readiness.iter().any(|stage| {
            stage.stage_id == "sms_over_ims"
                && stage
                    .blockers
                    .contains(&"device_state_change_authorization_missing")
        }));
        assert!(report.soak_scenarios.iter().any(|scenario| {
            scenario.scenario_id == "sms_delivery_consistency_soak"
                && scenario
                    .blockers
                    .contains(&"sms_live_test_window_not_authorized")
        }));
    }

    #[test]
    fn audit_serializes_no_private_identifiers_or_key_material() {
        let report = build_readiness_audit_report(&NoopRuntimeExecutor.describe());
        let json = serde_json::to_string(&report).expect("serialize audit report");
        let lower = json.to_ascii_lowercase();

        for forbidden_key in [
            "imsi",
            "iccid",
            "imei",
            "eid",
            "msisdn",
            "phone_number",
            "authorization",
            "password",
            "token",
            "key_material",
            "shared_secret",
            "skeyseed",
            "ck",
            "ik",
        ] {
            assert!(
                !lower.contains(&format!("\"{forbidden_key}\"")),
                "audit report must not contain a {forbidden_key} field"
            );
        }
    }
}
