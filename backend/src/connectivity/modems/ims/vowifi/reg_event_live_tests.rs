//! Captured-owner recovery gate and real runtime/cache projection; no device I/O.
use super::*;
use crate::connectivity::core::{registration::ImsRegistrationAccess, context::ImsIdentity};
use std::sync::atomic::{AtomicUsize, Ordering};

fn owner(line: &str, card_id: &str) -> (Arc<LiveImsSelection>, &'static CarrierProfile, LiveRegisterRequestContext) {
    let profile = profiles::derive_standard_3gpp_profile("001", "01", profiles::Standard3gppAccess::WifiEpdg).unwrap();
    let realm = "ims.mnc001.mcc001.3gppnetwork.org";
    let identity = LiveImsRegisterIdentity {
        shared: ImsIdentity { private_user: format!("001010000000001@{realm}"),
            public_uri: format!("sip:001010000000001@{realm}"), contact_user: "001010000000001".into(),
            home_domain: realm.into(), contact_user_phone: false }, shape: "reg_event_test",
    };
    let mut context = LiveRegisterRequestContext::new(profile, identity, "127.0.0.1:5062".parse().unwrap(), "127.0.0.1".parse().unwrap()).unwrap();
    let card = crate::connectivity::modems::ims::uicc_ims::UiccCardBinding {
        endpoint: format!("/synthetic/{line}"), slot: 1, owner: "mock-owner".into(),
        iccid: card_id.into(), imsi: "001010000000001".into(),
    };
    let selected = live_identity::selection_for_test(line, context.identity.clone(), context.target.clone(), card);
    context.uicc = Some(selected.clone());
    (selected, profile, context)
}
fn factory(line: &str, profile: &'static CarrierProfile, context: LiveRegisterRequestContext) -> VowifiUnregisterFactory {
    VowifiUnregisterFactory {
        line_id: line.into(), profile, context, variant: live_register_header_variants(profile)[0],
        next_cseq: std::sync::atomic::AtomicU32::new(8), expires_seconds: std::sync::atomic::AtomicU32::new(600),
        refresh_authorization: std::sync::Mutex::new(None), unregister_authorization: std::sync::Mutex::new(None),
        unregister_resync_sent: std::sync::atomic::AtomicBool::new(false), security_verify: Some("retained-verify".into()),
    }
}
async fn fixture() -> (String, VowifiUnregisterFactory, Arc<VowifiRuntime>) {
    static IDS: AtomicUsize = AtomicUsize::new(0);
    let line = format!("n5-live-recovery-{}", IDS.fetch_add(1, Ordering::Relaxed));
    let (uicc, profile, context) = owner(&line, "test-card");
    xcap_binding_cache().lock().await.insert(line.clone(), LiveXcapBinding {
        profile, local_address: "127.0.0.1".parse().unwrap(), username: context.identity.private_user.clone(), uicc,
    });
    ims_register_ready_cache().lock().await.insert(line.clone(), LiveImsRegisterReady {
        profile_id: profile.meta.profile_id, expires_at: Instant::now() + Duration::from_secs(600),
        registration: RegisteredImsContext::from_response(ImsRegistrationAccess::Vowifi,
            b"SIP/2.0 200 OK\r\nExpires: 600\r\nContent-Length: 0\r\n\r\n", 600),
        sms_capability_advertised: true, receiver_transport: "tcp",
    });
    let runtime = Arc::new(VowifiRuntime::for_line(&line));
    runtime.seed_registered_for_reg_event_test().await;
    bind_reg_event_runtime(&line, &runtime);
    let f = factory(&line, profile, context);
    (line, f, runtime)
}

#[tokio::test]
async fn reg_event_callback_revokes_actual_runtime_and_caches_but_retains_owner_for_ack() {
    let (line, factory, runtime) = fixture().await;
    factory.reg_event_check_owner().await.unwrap();
    assert!(runtime.snapshot().await.live_readiness.ims_registered);
    factory.reg_event_loss(Some(300)).await;
    let status = runtime.snapshot().await;
    assert!(!status.live_readiness.ims_registered && !status.live_readiness.sms_ready && !status.live_readiness.voice_ready);
    assert_eq!(status.degraded_reason.as_deref(), Some(LOSS_REASON));
    assert!(!ims_register_ready_cache().lock().await.contains_key(&line));
    assert!(!xcap_binding_cache().lock().await.contains_key(&line));
    assert!(live_ims_refresh_rebuild_pending_for_line(&line).await);
    assert!(!live_ims_registration_valid_for_line(&line).await);
    assert!(live_reg_event_retry_remaining_for_line(&line).await.unwrap() > Duration::from_secs(290));
    assert_eq!(check_retry_gate(&line).await.unwrap_err().reason, LOSS_REASON);
    factory.reg_event_check_owner().await.unwrap(); // exact revoked channel may ACK duplicates
    assert_eq!(factory.security_verify.as_deref(), Some("retained-verify"));
}

#[tokio::test]
async fn reg_event_wait_survives_profile_cache_cleanup_and_rejects_old_owner_success() {
    let (line, factory, _) = fixture().await;
    let old = factory.context.uicc.as_ref().unwrap().clone();
    factory.reg_event_loss(Some(90)).await;
    clear_live_ims_refresh_failure_for_line(&line).await;
    assert!(check_retry_gate(&line).await.is_err());
    let (new, _, _) = owner(&line, "test-card");
    assert!(accepted_owner(&line, &new).await.is_err());
    revoked().lock().await.get_mut(&line).unwrap().not_before = Some(Instant::now());
    assert!(accepted_owner(&line, &old).await.is_err(), "same old channel is not a recovered registration");
    accepted_owner(&line, &new).await.unwrap();
    assert!(!registration_revoked(&line).await);
}

#[tokio::test]
async fn reg_event_long_wait_is_stopped_and_new_sim_does_not_inherit_it() {
    let (line, factory, _) = fixture().await;
    factory.reg_event_loss(Some(u32::MAX)).await;
    assert_eq!(live_reg_event_retry_remaining_for_line(&line).await, Some(Duration::MAX));
    let (same_card, _, _) = owner(&line, "test-card");
    assert!(accepted_owner(&line, &same_card).await.is_err());
    let (new_card, _, _) = owner(&line, "different-card");
    assert_eq!(live_reg_event_retry_remaining_for_line(&line).await, None);
    accepted_owner(&line, &new_card).await.unwrap();
}

#[tokio::test]
async fn reg_event_same_sim_reenumeration_does_not_clear_network_wait() {
    let (line, factory, _) = fixture().await;
    factory.reg_event_loss(Some(300)).await;
    let context = &factory.context;
    let new_card = crate::connectivity::modems::ims::uicc_ims::UiccCardBinding {
        endpoint: format!("/renumbered/{line}"), slot: 1, owner: "new-modem-owner".into(),
        iccid: "test-card".into(), imsi: "001010000000001".into(),
    };
    let selected = live_identity::selection_for_test(&line, context.identity.clone(), context.target.clone(), new_card);
    assert!(live_reg_event_retry_remaining_for_line(&line).await.unwrap() > Duration::from_secs(290));
    assert!(accepted_owner(&line, &selected).await.is_err());
}

#[tokio::test]
async fn reg_event_stale_factory_cannot_clear_replacement_runtime_or_bindings() {
    let (line, old, runtime) = fixture().await;
    let (new, profile, context) = owner(&line, "test-card");
    xcap_binding_cache().lock().await.insert(line.clone(), LiveXcapBinding {
        profile, local_address: "127.0.0.1".parse().unwrap(), username: context.identity.private_user.clone(), uicc: new.clone(),
    });
    assert_eq!(old.reg_event_check_owner().await.unwrap_err().code(), "ims_uicc_binding_changed");
    old.reg_event_loss(Some(500)).await;
    assert!(!registration_revoked(&line).await);
    assert!(runtime.snapshot().await.live_readiness.ims_registered);
    assert!(Arc::ptr_eq(&xcap_binding_cache().lock().await.get(&line).unwrap().uicc, &new));
}

#[test]
fn reg_event_auth_never_accepts_auts_or_empty_proof_as_authenticated_subscription() {
    for header in ["Authorization: Digest response=\"\",auts=\"abc\"", "Authorization: Digest response=\"\"", "Authorization: Digest nonce=\"abc\""] {
        assert!(require_subscribe_proof(header).is_err());
    }
    assert!(require_subscribe_proof("Authorization: Digest response=\"0123456789abcdef0123456789abcdef\"").is_ok());
}
