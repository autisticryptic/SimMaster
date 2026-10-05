use super::*;
use crate::connectivity::modems::ims::vowifi::profiles::{
    derive_standard_3gpp_profile, Standard3gppAccess,
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn derived_proposal_extension_preserves_dh_and_transport_attempt_budgets() {
    let profile = derive_standard_3gpp_profile("234", "15", Standard3gppAccess::WifiEpdg).unwrap();
    let groups = live_ike_proposal_groups(profile).unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].dh_group, DhGroup::Modp2048);
    assert_eq!(groups[1].dh_group, DhGroup::Modp1024);
    assert_eq!(groups[0].proposals[0], "aes256-sha256-prfsha512-modp2048");
    assert!(groups[0]
        .proposals
        .contains(&"aes256-sha512-prfsha256-modp2048"));
    assert!(groups[0]
        .proposals
        .contains(&"aes128-sha512-prfsha512-modp2048"));
    assert_eq!(LIVE_IKE_MAX_PROPOSAL_GROUPS_PER_PASS, 2);
    assert_eq!(LIVE_IKE_MAX_TRANSPORT_PATHS_PER_PASS, 2);
    assert!(profile.ikev2.esp_proposals.contains(&"aes128-sha512"));
    crate::connectivity::modems::ims::vowifi::ike_payloads::child_sa_proposal_from_profile_string(
        "aes128-sha512",
        1,
        &[0x12, 0x34, 0x56, 0x78],
    )
    .unwrap();
}

#[tokio::test]
async fn explicit_ike_auth_rejection_stops_address_replay_but_timeouts_can_fallback() {
    let addresses = [
        "192.0.2.1:500".parse().unwrap(),
        "192.0.2.2:500".parse().unwrap(),
    ];
    for reason in [
        "ike_auth_notify_authentication_failed",
        "ike_auth_notify_authorization_failed",
    ] {
        let attempts = AtomicUsize::new(0);
        let result =
            try_live_epdg_addresses::<(), _, _>(&addresses, LiveProbeDepth::FullHandshake, |_| {
                attempts.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Err(live_stage_error(reason)))
            })
            .await;
        assert_eq!(result.unwrap_err().reason, reason);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
    let attempts = AtomicUsize::new(0);
    let result = try_live_epdg_addresses(&addresses, LiveProbeDepth::FullHandshake, |_| {
        let n = attempts.fetch_add(1, Ordering::SeqCst);
        std::future::ready(if n == 0 {
            Err(live_stage_error("ike_sa_init_udp500_timeout"))
        } else {
            Ok(())
        })
    })
    .await;
    assert!(result.is_ok());
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    for reason in [
        "ike_auth_notify_ipv6_required",
        "ike_auth_notify_no_proposal_chosen",
        "ike_auth_progress_timeout",
    ] {
        assert!(
            !terminal_ike_auth_rejection(&live_stage_error(reason)),
            "{reason}"
        );
    }
}
