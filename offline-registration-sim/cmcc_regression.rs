//! Controlled header-policy regression model, NOT a captured CMCC registrar.
use super::*;

#[tokio::test]
async fn offline_cmcc_historical_declaration_matrix() {
    let cases = [
        ("cmcc_46000_september_declaration", "460", "00", "cmcc_legacy", true),
        ("cmcc_46002_september_declaration", "460", "02", "cmcc_legacy", true),
        ("cmcc_proactive_flags_counterexample", "460", "02", "cmcc_regressed_primary", false),
        ("cmcc_network_demand_still_escalates", "460", "02", "cmcc_demands_security", true),
        ("cmcc_ordinary_403_still_stops", "460", "02", "cmcc_terminal_403", false),
        ("cmcc_unoffered_md5_still_rejected", "460", "02", "cmcc_unoffered_md5", false),
        ("globe_proactive_declaration_preserved", "515", "02", "required_first", true),
        ("kpn_proactive_declaration_preserved", "204", "08", "required_first", true),
    ];
    let mut results = Vec::new();
    for (id, mcc, mnc, mode, expected) in cases {
        let result = run_case_on_network(Scenario { id, wifi: false, mode, expected, fallbacks: true }, mcc, mnc, true).await;
        match mode {
            "cmcc_legacy" => {
                assert_eq!(result["candidate_trace"].as_array().unwrap().len(), 1);
                assert_eq!(result["status_trace"], json!([401, 200]));
            }
            "cmcc_demands_security" => assert_eq!(result["status_trace"], json!([421, 401, 200])),
            "cmcc_terminal_403" => {
                assert_eq!(result["request_count"], 1);
                assert_eq!(result["status_trace"], json!([403]));
            }
            "cmcc_unoffered_md5" => assert_eq!(result["digest_verified_count"], 0),
            "cmcc_regressed_primary" => assert_eq!(result["digest_verified_count"], 0),
            _ => {}
        }
        eprintln!("CMCC_DECLARATION_SIMULATION {id} passed={}", result["passed"]);
        results.push(result);
    }
    let passed = results.iter().all(|r| r["passed"] == true);
    let report = json!({"suite_id":"simadmin-cmcc-historical-declaration-v1", "passed":passed,
        "hardware_used":false,"live_network_verified":false,"evidence_kind":"offline_simulation","scenarios":results,
        "limitations":[
            "The September header flags are verified against Git snapshots; the registrar responses are a controlled model, not recovered CMCC captures",
            "Only the two proactive Require flags differ in the negative A/B; this does not prove the carrier's reason for its real 421",
            "Algorithms remain the current explicit strict allowlist; historical non-strict MD5 acceptance is not restored",
            "No modem, SIM, bearer, XFRM or operator-network certification; no additional catalog pruning authorized"
        ]});
    if let Ok(path) = std::env::var("SIMADMIN_CMCC_REGRESSION_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    assert!(passed);
}
