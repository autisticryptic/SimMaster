//! Same-card material changes through the production WiFi factory/operator.
use super::*;
use crate::connectivity::modems::ims::uicc_ims::provisioning::UiccImsProvisioning;

fn changed() -> UiccImsProvisioning {
    UiccImsProvisioning::new(crate::connectivity::modems::ims::cellular_ims::identity::UiccApplications {
        usim_aid: Some(USIM_AID_PREFIX.to_vec()), isim_aid: Some(super::super::super::qmi_uim::isim::ISIM_AID_PREFIX.to_vec()),
    }, Some(super::super::super::qmi_uim::IsimImsMaterial { impi: Some("updated@ims.example.test".into()),
        domain: Some("ims.example.test".into()), impus: vec!["sip:updated@ims.example.test".into()],
        pcscf: vec!["pcscf.example.test".into()] })).unwrap()
}
fn change(factory: &VowifiUnregisterFactory) {
    live_identity::seed_provisioning_for_test(factory.context.uicc.as_ref().unwrap(), Ok(changed()));
}
async fn quiet(peer: &UdpSocket) {
    assert!(tokio::time::timeout(Duration::from_millis(70), peer.recv_from(&mut [0u8; 8192])).await.is_err());
}

#[tokio::test]
async fn same_card_update_refuses_refresh_before_nonce_channel_or_lease_changes() {
    let (factory, mut channel, peer) = fixture(false).await;
    change(&factory);
    let result = factory.refresh_with_recovery(&mut channel, Some(Instant::now() + Duration::from_secs(600))).await.unwrap_err();
    assert_eq!(result.code(), "ims_uicc_provisioning_changed"); assert!(result.retained_until.is_none());
    assert!(register_reason_blocks_profile_fallback(result.code()));
    assert_retained(&factory, 1); assert_eq!(factory.next_cseq.load(Ordering::Relaxed), 8);
    quiet(&peer).await;
}

#[tokio::test]
async fn changed_material_after_200_never_publishes_new_nonce_or_registration() {
    let (factory, mut channel, peer) = fixture(false).await;
    let peer_factory = factory.clone();
    let sender = tokio::spawn(async move {
        let (request, source) = receive(&peer).await;
        change(&peer_factory);
        peer.send_to(&response(&request, "200 OK", &format!("Expires: 7200\r\n{}", info(&request, false))), source).await.unwrap();
        peer
    });
    let result = factory.refresh_with_recovery(&mut channel, Some(Instant::now() + Duration::from_secs(600))).await.unwrap_err();
    assert_eq!(result.code(), "ims_uicc_provisioning_changed"); assert!(result.retained_until.is_none());
    assert_retained(&factory, 2); quiet(&sender.await.unwrap()).await;
}

#[tokio::test]
async fn changed_material_after_503_preserves_the_original_scope_wait() {
    let (factory, mut channel, peer) = fixture(false).await;
    let peer_factory = factory.clone();
    let sender = tokio::spawn(async move {
        let (request, source) = receive(&peer).await;
        change(&peer_factory);
        peer.send_to(&response(&request, "503 Service Unavailable", "Retry-After: 120\r\n"), source).await.unwrap();
    });
    let result = factory.refresh_with_recovery(&mut channel, Some(Instant::now() + Duration::from_secs(600))).await.unwrap_err();
    sender.await.unwrap();
    assert_eq!(result.code(), "ims_uicc_provisioning_changed");
    assert!(!register_retry_live::restart_admission(&factory.line_id).await.is_allowed());
    clear_live_runtime_for_line(&factory.line_id).await;
    assert!(!register_retry_live::restart_admission(&factory.line_id).await.is_allowed());
}

#[tokio::test]
async fn stale_min_expires_and_unregister_do_not_bypass_changed_provisioning() {
    let (factory, _channel, peer) = fixture(false).await;
    let mut auth = VowifiRefreshAuthenticator { factory: &factory,
        authorization: factory.refresh_authorization.lock().unwrap().clone(), resync_sent: false, expires: 3600 };
    change(&factory);
    let old = factory.initial_request().unwrap();
    assert_eq!(auth.authenticated_request(&challenge_response(&old, false), 9).await.unwrap_err().code(), "ims_uicc_provisioning_changed");
    assert_eq!(auth.rebuild_register_with_min_expires(b"", 10, 7200, true).await.unwrap_err().code(), "ims_uicc_provisioning_changed");
    assert!(factory.unregister_admitted().await.is_err());
    assert_eq!(factory.unregister_accepted().await.unwrap_err().code(), "ims_uicc_provisioning_changed");
    quiet(&peer).await;
}

#[tokio::test]
async fn installed_operator_revokes_readiness_instead_of_retaining_stale_registration() {
    use crate::connectivity::modems::ims::vowifi::operator::{self, RegisteredVoiceContext};
    let (factory, channel, peer) = fixture(false).await;
    operator::install_registered_channel(RegisteredVoiceContext {
        line_id: factory.line_id.clone(), profile_id: factory.profile.meta.profile_id,
        identity: factory.context.identity.shared.clone(), route: channel.route(),
        registration: RegisteredImsContext::from_response(ImsRegistrationAccess::Vowifi, b"SIP/2.0 200 OK\r\nExpires: 3600\r\n\r\n", 3600),
        security_verify: Some(VERIFY.into()), pani: "IEEE-802.11".into(), user_agent: "r5-fixture".into(),
        expires_at: Instant::now() + Duration::from_secs(600), tcp_keepalive_interval: None, options_ping_interval: None,
        unregister: Some(factory.clone()), media_route_installer: None, media_interface: None,
        media_operator_creator: crate::connectivity::core::media::test_operator_socket_creator(),
    }, channel).await;
    tokio::time::timeout(Duration::from_secs(1), async {
        while !operator::operator_link_for_line(&factory.line_id).is_available() { tokio::task::yield_now().await; }
    }).await.unwrap();
    change(&factory);
    let result = operator::refresh_registered_for_line(&factory.line_id, factory.profile.meta.profile_id).await.unwrap().unwrap_err();
    assert_eq!(result.code(), "ims_uicc_provisioning_changed");
    assert!(!operator::operator_link_for_line(&factory.line_id).is_available());
    assert_retained(&factory, 1);
    let _ = operator::disconnect_line(&factory.line_id).await;
    quiet(&peer).await;
}

#[tokio::test]
async fn unreadable_provisioning_never_turns_into_a_profile_retry_or_valid_old_lease() {
    let (factory, mut channel, peer) = fixture(false).await;
    live_identity::seed_provisioning_for_test(factory.context.uicc.as_ref().unwrap(), Err("isim_apdu_exchange_failed"));
    let result = factory.refresh_with_recovery(&mut channel, Some(Instant::now() + Duration::from_secs(600))).await.unwrap_err();
    assert_eq!(result.code(), "isim_apdu_exchange_failed"); assert!(result.retained_until.is_none());
    assert!(register_reason_blocks_profile_fallback(result.code())); assert_retained(&factory, 1); quiet(&peer).await;
}
