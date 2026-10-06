//! History-inspired protocol regressions, NOT replay of real SIM credentials
//! or certification of a carrier. Historical logs often omit the chosen
//! algorithm, so AES/null/alias alternatives are tested without attributing
//! an unrecorded mechanism to a particular card.
use super::*;

#[tokio::test]
async fn offline_historical_registration_matrix() {
    let cases = [
        ("globe_ipv4_sha1_aes", "515", "02", false, "first_security", true),
        ("globe_ipv4_sha1_null", "515", "02", false, "second_security", true),
        ("globe_ipv4_sha1_alias_aes", "515", "02", false, "sha1_alias_aes", true),
        ("globe_ipv4_sha1_alias_null", "515", "02", false, "sha1_alias_null", true),
        ("kpn_ipv6_sha1_null", "204", "08", true, "second_security", true),
        ("kpn_ipv6_sha1_alias_null", "204", "08", true, "sha1_alias_null", true),
        ("kpn_initial_403_stops", "204", "08", true, "403", false),
        ("kpn_authenticated_403_stops", "204", "08", true, "post_auth_403", false),
        ("sim01_45400_ipv6_udp_aka", "454", "00", true, "baseline", true),
        ("sim02_46000_ipv6_udp_aka", "460", "00", true, "baseline", true),
        ("sim03_45403_ipv4_udp_aka", "454", "03", false, "baseline", true),
        ("sim04_45507_ipv6_udp_aka", "455", "07", true, "baseline", true),
        ("sim06_46011_ipv6_sha1_aes", "460", "11", true, "first_security", true),
        ("sim06_46011_ipv6_sha1_alias_aes", "460", "11", true, "sha1_alias_aes", true),
        ("derived_unoffered_md5_stops", "515", "02", false, "unoffered_security", false),
        ("malformed_aka_nonce_stops", "515", "02", false, "bad_nonce", false),
        ("explicit_disabled_not_overridden", "515", "02", false, "unsolicited_disabled", false),
        ("incorrect_digest_stops", "204", "08", true, "bad_proof", false),
    ];
    let mut results = vec![];
    for (id, mcc, mnc, ipv6, mode, expected) in cases {
        let mut result = run_case_on_network(Scenario {
            id, wifi: false, mode, expected, fallbacks: true,
        }, mcc, mnc, ipv6).await;
        result["home_plmn"] = json!(format!("{mcc}{mnc}"));
        result["wire_address_family"] = json!(if ipv6 { "ipv6" } else { "ipv4" });
        // The fixture contains no socket or bearer; family affects only SIP
        // serialization. It cannot validate family fallback or an MTU.
        if mode == "bad_nonce" || mode == "unoffered_security" || mode == "unsolicited_disabled" {
            assert_eq!(result["digest_verified_count"], 0);
            assert_eq!(result["request_count"], 1);
        }
        if mode == "post_auth_403" {
            assert_eq!(result["digest_verified_count"], 1);
            assert_eq!(result["status_trace"], json!([401, 403]));
        }
        eprintln!("HISTORY_SIMULATION {id} passed={} registered={}", result["passed"], result["observed_success"]);
        results.push(result);
    }
    let passed = results.iter().all(|r| r["passed"] == true);
    let report = json!({
        "suite_id": "simadmin-offline-history-registration-v1",
        "evidence_kind": "offline_simulation",
        "live_network_verified": false, "hardware_used": false,
        "passed": passed, "scenarios": results,
        "scope": "current production derivation, SIP request/response/security selection and REGISTER/Digest-AKA with synthetic identities and an independent in-memory registrar",
        "limitations": [
            "Historical carrier names/PLMNs select representative protocol cases, not recorded real challenges or credentials",
            "Chosen historical IPsec algorithms were not always recorded; tested alternatives are not claims about those carriers",
            "No real SIM, bearer family fallback, MM reset, profile lease, entitlement, NAS, XFRM, socket MTU, or roaming visited-network transition is simulated",
            "No registration verdict for historical cards that failed before SIP or lack enough recorded parameters",
            "Initial REGISTER transaction coverage only; production refresh tests are separate"
        ]
    });
    if let Ok(path) = std::env::var("SIMADMIN_HISTORY_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    assert!(passed, "one or more history-inspired protocol regressions failed");
}
