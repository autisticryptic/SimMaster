#![allow(dead_code)]

use crate::infra::db::{Database, NewVowifiSoakRun, NewVowifiSoakSample};

use super::{
    executor::{ExecutorStageRequest, ExecutorStageResult},
    live::{LiveStageAdapter, LiveStageRunner},
};

pub fn persist_stage_observation(
    db: &Database,
    request: &ExecutorStageRequest,
    result: &ExecutorStageResult,
) -> rusqlite::Result<Option<String>> {
    let Some(observation) = result.soak_observation.as_ref() else {
        return Ok(None);
    };
    let run_id = format!("{}-{}", request.trace_id, observation.scenario_id);
    let failure_count = i64::from(result.status == "failed");

    let last_error = result.reason.as_deref().filter(|_| failure_count > 0);

    db.upsert_vowifi_soak_run(NewVowifiSoakRun {
        run_id: &run_id,
        scenario_id: observation.scenario_id,
        profile_id: request.profile_id.as_deref(),
        plmn: request.plmn.as_deref(),
        status: "planned",
        duration_seconds: 0,
        sample_count: 1,
        failure_count,
        last_error,
    })?;
    db.insert_vowifi_soak_sample(NewVowifiSoakSample {
        run_id: &run_id,
        sample_kind: observation.sample_kind,
        metric_name: observation.metric_name,
        metric_value: observation.metric_value,
        state: observation.state,
    })?;

    Ok(Some(run_id))
}

pub async fn run_stage_and_persist<A>(
    db: &Database,
    runner: &LiveStageRunner<A>,
    request: ExecutorStageRequest,
) -> rusqlite::Result<(ExecutorStageResult, Option<String>)>
where
    A: LiveStageAdapter,
{
    let result = runner.run(request.clone()).await;
    let run_id = persist_stage_observation(db, &request, &result)?;
    Ok((result, run_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        access::vowifi::executor::{
            readiness_key_for_stage, soak_observation_for_stage, ExecutorStage,
            ExecutorStageRequest, ExecutorStageResult, ExecutorStageStatus,
        },
        access::vowifi::{
            live::{LiveAdapterFuture, LiveStageObservation},
            profiles::{CarrierProfile, GB_EE_23433},
        },
        infra::db::Database,
    };
    use std::path::PathBuf;

    #[derive(Debug, Clone, Copy)]
    struct MockReadyAdapter;

    impl LiveStageAdapter for MockReadyAdapter {
        fn run_stage<'a>(
            &'a self,
            stage: ExecutorStage,
            _profile: &'static CarrierProfile,
        ) -> LiveAdapterFuture<'a> {
            Box::pin(async move {
                Ok(LiveStageObservation {
                    stage: stage.as_str(),
                    ready: true,
                    detail: "mock_ready",
                    sensitive_values_policy: "metadata_only",
                })
            })
        }
    }

    #[test]
    fn stage_observation_persists_counter_metadata_only() {
        let db = Database::new(PathBuf::from(":memory:")).expect("create db");
        let request = ExecutorStageRequest {
            stage: ExecutorStage::Ike,
            profile_id: Some("gb_ee_23433".to_string()),
            plmn: Some("23433".to_string()),
            trace_id: "trace-local".to_string(),
        };
        let result = ExecutorStageResult {
            stage: request.stage.as_str(),
            status: ExecutorStageStatus::Skipped.as_str(),
            readiness_key: readiness_key_for_stage(request.stage),
            reason: Some("live_runtime_executor_not_implemented".to_string()),
            soak_observation: Some(soak_observation_for_stage(request.stage)),
        };

        let run_id = persist_stage_observation(&db, &request, &result)
            .expect("persist observation")
            .expect("run id");

        assert_eq!(run_id, "trace-local-rekey_dpd_nat_t_soak");
        let runs = db.get_vowifi_soak_runs(10, 0).expect("read runs");
        assert_eq!(runs.total, 1);
        assert_eq!(runs.runs[0].scenario_id, "rekey_dpd_nat_t_soak");
        assert_eq!(runs.runs[0].samples[0].metric_name, "ike_stage_attempts");

        let json = serde_json::to_string(&runs).expect("serialize runs");
        for forbidden_key in [
            "imsi",
            "iccid",
            "imei",
            "eid",
            "msisdn",
            "phone_number",
            "key_material",
            "authorization",
            "password",
            "token",
        ] {
            assert!(!json
                .to_ascii_lowercase()
                .contains(&format!("\"{forbidden_key}\"")));
        }
    }

    #[tokio::test]
    async fn runner_result_persists_to_soak_tables() {
        let db = Database::new(PathBuf::from(":memory:")).expect("create db");
        let runner = LiveStageRunner::new(
            crate::access::vowifi::executor::LiveExecutorGateReport {
                live_network_authorized: true,
                device_state_changes_authorized: true,
                adb_path_configured: true,
                device_admin_url_configured: true,
                implementation_ready: true,
                effective_live_network_allowed: true,
                effective_device_state_changes_allowed: true,
                blockers: Vec::new(),
                sensitive_values_policy: "presence_flags_only_no_paths_or_urls_serialized",
            },
            &GB_EE_23433,
            MockReadyAdapter,
        );

        let (result, run_id) = run_stage_and_persist(
            &db,
            &runner,
            ExecutorStageRequest {
                stage: ExecutorStage::Epdg,
                profile_id: Some("gb_ee_23433".to_string()),
                plmn: Some("23433".to_string()),
                trace_id: "runner".to_string(),
            },
        )
        .await
        .expect("run and persist");

        assert_eq!(result.status, "completed");
        assert_eq!(run_id.as_deref(), Some("runner-network_path_recovery_soak"));
        let runs = db.get_vowifi_soak_runs(10, 0).expect("read runs");
        assert_eq!(runs.total, 1);
        assert_eq!(
            runs.runs[0].samples[0].metric_name,
            "epdg_resolution_attempts"
        );
    }
}
