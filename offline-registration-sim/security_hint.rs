//! Synthetic 421/494 protocol cases inspired by the CMCC trace, not carrier certification.
use super::*;

#[tokio::test]
async fn offline_security_hint_registration_matrix() {
    let cases = [
        ("cmcc_421_preferred_aes_reoffer", "hint_aes", true),
        ("cmcc_421_sha1_alias_reoffer", "hint_alias", true),
        ("cmcc_494_preferred_aes_reoffer", "hint_494", true),
        ("hint_cannot_steer_to_null", "hint_null", false),
        ("hint_cannot_add_md5", "hint_unknown", false),
        ("repeat_421_stops", "hint_repeat", false),
        ("reoffer_403_stops", "hint_403", false),
        ("challenge_cannot_escape_singleton", "hint_changed", false),
        ("challenge_requires_security", "hint_missing", false),
        ("reoffer_rejects_unprotected_200", "hint_direct200", false),
    ];
    let mut results = Vec::new();
    for (id, mode, expected) in cases {
        let result = run_case_on_network(Scenario { id, wifi: false, mode, expected, fallbacks: true }, "460", "02", true).await;
        let candidates = result["candidate_trace"].as_array().unwrap();
        assert_eq!(candidates[0], "standard_3gpp_conservative");
        assert!(candidates.iter().all(|v| v != "generic_ims_register_fallback"));
        let blocked_hint = matches!(mode, "hint_null" | "hint_unknown");
        assert_eq!(candidates.len(), if blocked_hint { 1 } else { 2 });
        if !blocked_hint { assert_eq!(candidates[1], "standard_3gpp_security_hint_reoffer"); }
        if !expected { assert_eq!(result["digest_verified_count"], 0); }
        eprintln!("SECURITY_HINT_SIMULATION {id} passed={} registered={}", result["passed"], result["observed_success"]);
        results.push(result);
    }
    let passed = results.iter().all(|r| r["passed"] == true);
    let report = json!({
        "suite_id": "simadmin-offline-security-hint-v1", "evidence_kind": "offline_simulation",
        "passed": passed, "scenarios": results, "hardware_used": false, "live_network_verified": false,
        "limitations": [
            "Real trace omitted Security-Server/Warning values; fixtures use synthetic complete AES proposals, not replayed CMCC data",
            "No real SIM, XFRM installation, sockets, modem, or network authentication is exercised",
            "This does not authorize additional catalog pruning or certify a carrier"
        ]
    });
    if let Ok(path) = std::env::var("SIMADMIN_SECURITY_HINT_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    assert!(passed);
}
