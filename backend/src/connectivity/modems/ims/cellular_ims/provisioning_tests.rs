//! Same-card provisioning changes through real cellular REGISTER procedures.
use super::*;
use crate::connectivity::modems::ims::uicc_ims::provisioning::{self, UiccImsProvisioning};

async fn fixture() -> (CellularImsLiveSession, CellularImsRuntime, UdpSocket, UdpSocket) {
    let profile = crate::connectivity::modems::ims::vowifi::profiles::derive_standard_3gpp_profile(
        "001", "01", crate::connectivity::modems::ims::vowifi::profiles::Standard3gppAccess::LteEpc).unwrap();
    protected_session_with_profile(profile).await
}
fn changed() -> UiccImsProvisioning {
    UiccImsProvisioning::new(identity::UiccApplications {
        usim_aid: Some(crate::connectivity::modems::ims::vowifi::qmi_uim::USIM_AID_PREFIX.to_vec()),
        isim_aid: Some(identity::ISIM_AID_PREFIX.to_vec()),
    }, Some(crate::connectivity::modems::ims::vowifi::qmi_uim::IsimImsMaterial {
        impi: Some("updated@ims.example.test".into()), domain: Some("ims.example.test".into()),
        impus: vec!["sip:updated@ims.example.test".into()], pcscf: vec!["new.example.test".into()],
    })).unwrap()
}
fn change(session: &CellularImsLiveSession) {
    provisioning::seed_for_test(&session.device.modem_id, session.auth_binding.as_ref().unwrap(), changed());
}
async fn quiet(server: &UdpSocket) {
    assert!(tokio::time::timeout(Duration::from_millis(70), server.recv_from(&mut [0; 8192])).await.is_err());
}

#[tokio::test]
async fn same_card_ef_update_blocks_refresh_before_nonce_sa_or_wire_changes() {
    let (mut session, runtime, server, _client) = fixture().await;
    let old = session.registration.clone(); let old_route = session.channel.send_route();
    change(&session);
    let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
    let result = refresh_live_registration(&mut session, &runtime, "r5-before", &db).await;
    assert!(matches!(result.outcome, RegistrationRefreshResult::RebuildAccess(_)));
    assert_eq!(result.error.unwrap().code(), "ims_uicc_provisioning_changed");
    assert_eq!(session.refresh_authorization.as_ref().unwrap().nonce_count, 2);
    assert_eq!(session.registration, old); assert_eq!(session.channel.send_route(), old_route);
    quiet(&server).await;
}

#[tokio::test]
async fn ef_update_during_success_never_commits_new_lease_or_nextnonce() {
    let (mut session, runtime, server, _client) = fixture().await;
    let old = session.registration.clone();
    let old_cseq = session.next_register_cseq;
    session.channel.reserve_security_ports_for_test(session.security_binding.port_s);
    let card = session.auth_binding.clone().unwrap(); let modem = session.device.modem_id.clone();
    let sender = tokio::spawn(async move {
        let (request, source) = receive(&server).await;
        provisioning::seed_for_test(&modem, &card, changed());
        server.send_to(&response(&request, "200 OK", "Expires: 7200\r\nAuthentication-Info: nextnonce=\"unused\"\r\n"), source).await.unwrap();
        server
    });
    let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
    let result = refresh_live_registration(&mut session, &runtime, "r5-late", &db).await;
    let server = sender.await.unwrap();
    assert!(matches!(result.outcome, RegistrationRefreshResult::RebuildAccess(_)));
    assert_eq!(result.error.unwrap().code(), "ims_uicc_provisioning_changed");
    assert_eq!(session.registration, old);
    assert_eq!(session.refresh_authorization.as_ref().unwrap().challenge.nonce, "old-nonce");
    assert_eq!(session.refresh_authorization.as_ref().unwrap().nonce_count, 3);
    assert_eq!(session.next_register_cseq, old_cseq + 1);
    quiet(&server).await;
}

#[tokio::test]
async fn ef_update_during_temporary_failure_preserves_same_subscription_wait() {
    let (mut session, runtime, server, _client) = fixture().await;
    session.channel.reserve_security_ports_for_test(session.security_binding.port_s);
    let card = session.auth_binding.clone().unwrap(); let modem = session.device.modem_id.clone();
    let sender = tokio::spawn(async move {
        let (request, source) = receive(&server).await;
        provisioning::seed_for_test(&modem, &card, changed());
        server.send_to(&response(&request, "503 Service Unavailable", "Retry-After: 120\r\n"), source).await.unwrap();
    });
    let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
    let result = refresh_live_registration(&mut session, &runtime, "r5-wait", &db).await;
    sender.await.unwrap();
    assert_eq!(result.error.unwrap().code(), "ims_uicc_provisioning_changed");
    assert!(!session.endpoint_retries.lock().await.restart_admission(Instant::now()).is_allowed());
}

#[tokio::test]
async fn unregister_does_not_send_old_material_or_confirm_a_late_changed_sample() {
    for late in [false, true] {
        let (session, runtime, server, _client) = fixture().await;
        let card = session.auth_binding.clone().unwrap(); let modem = session.device.modem_id.clone();
        if !late { change(&session); }
        let live = CellularImsLiveHandle::new(); *live.session.lock().await = Some(session);
        let sender = tokio::spawn(async move {
            if late {
                let (request, source) = receive(&server).await;
                provisioning::seed_for_test(&modem, &card, changed());
                server.send_to(&response(&request, "200 OK", "Expires: 0\r\n"), source).await.unwrap();
            }
            server
        });
        assert_eq!(unregister_live_session(&live, &runtime).await, UnregisterResult::AccessLost);
        quiet(&sender.await.unwrap()).await;
    }
}

#[tokio::test]
async fn stale_and_min_expires_cannot_reuse_credentials_after_same_card_update() {
    let (mut session, runtime, server, _client) = fixture().await;
    let mut auth = authenticator(&session, &runtime, session.security_binding);
    change(&session);
    assert_eq!(auth.prepare_authenticated_channel(b"SIP/2.0 401 Unauthorized\r\n\r\n", &mut session.channel).await.unwrap_err().code(), "ims_uicc_provisioning_changed");
    assert_eq!(auth.rebuild_register_with_min_expires(b"", 2, 7200, false).await.unwrap_err().code(), "ims_uicc_provisioning_changed");
    assert!(auth.pending.is_none()); quiet(&server).await;
}

#[tokio::test]
async fn provisioning_read_failure_is_not_a_derived_identity_or_keepalive_success() {
    let (mut session, runtime, server, _client) = fixture().await;
    provisioning::seed_result_for_test(&session.device.modem_id, session.auth_binding.as_ref().unwrap(), Err("isim_apdu_exchange_failed"));
    let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
    let result = refresh_live_registration(&mut session, &runtime, "r5-read-error", &db).await;
    assert!(matches!(result.outcome, RegistrationRefreshResult::RebuildAccess(_)));
    assert_eq!(result.error.unwrap().code(), "isim_apdu_exchange_failed"); quiet(&server).await;
}
