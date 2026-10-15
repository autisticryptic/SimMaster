use super::*;
use crate::hardware::devices::ims_bearer_policy::{
    DhcpPcscfPolicy, ImsApnAuthentication, PcoRequestPolicy,
};

fn policy(method: ImsApnAuthMethod) -> ImsBearerRequirements {
    ImsBearerRequirements { authentication: ImsApnAuthentication {
        method, username: Some("fixture-username".into()), password: Some("fixture-password".into()),
    }, ..Default::default() }
}

async fn start(transport: &NativeImsTransport, requirements: &ImsBearerRequirements)
    -> Result<(ImsBearerInfo, Box<dyn ImsBearerHandle + Send>), ImsBearerError> {
    transport.establish_ims_bearer_with_requirements("/dev/fixture", &transport.device.spec.selector(),
        "ims", None, 1, &[4], false, None, requirements).await
}

#[tokio::test]
async fn native_qmi_real_factory_emits_only_requested_auth_on_owned_ims() {
    for (method, token) in [(ImsApnAuthMethod::Pap, "PAP"), (ImsApnAuthMethod::Chap, "CHAP"),
        (ImsApnAuthMethod::PapOrChap, "BOTH")] {
        let (device, io) = memory_device(None);
        let transport = NativeImsTransport::new(device);
        let requirements = policy(method);
        let (_, handle) = start(&transport, &requirements).await.unwrap();
        {
            let requests = io.requests.lock().unwrap();
            let request = requests.iter().find(|r| r.arguments.iter().any(|a| a.starts_with("--wds-start-network="))).unwrap();
            let action = request.arguments.iter().find(|a| a.starts_with("--wds-start-network=")).unwrap();
            assert!(action.contains("apn=ims,ip-type=4"));
            assert!(action.contains(&format!("username=fixture-username,password=fixture-password,auth={token}")));
            assert!(request.arguments.contains(&"--client-cid=17".into()));
            assert!(request.arguments.contains(&"--client-no-release-cid".into()));
            assert!(!action.contains("autoconnect"));
            assert!(!format!("{request:?}").contains("fixture-password"));
            assert!(!format!("{request:?}").contains("fixture-username"));
        }
        let receipts = format!("{:?}", io.receipts.lock().unwrap());
        assert!(!receipts.contains("fixture-password"));
        assert!(!format!("{:?}", ims_apn_configuration("ims", &requirements)).contains("fixture-password"));
        handle.release().await;
        assert!(io.receipts.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn explicit_no_auth_and_legacy_unspecified_have_distinct_wire_intent() {
    for (method, explicit) in [(ImsApnAuthMethod::Unspecified, false), (ImsApnAuthMethod::None, true)] {
        let (device, io) = memory_device(None);
        let transport = NativeImsTransport::new(device);
        let requirements = ImsBearerRequirements { authentication: ImsApnAuthentication {
            method, ..Default::default()
        }, ..Default::default() };
        let (_, handle) = start(&transport, &requirements).await.unwrap();
        {
            let requests = io.requests.lock().unwrap();
            let action = requests.iter().flat_map(|r| r.arguments.iter())
                .find(|a| a.starts_with("--wds-start-network=")).unwrap();
            assert_eq!(action.contains(",auth=NONE"), explicit);
            assert!(!action.contains("username="));
            assert!(!action.contains("password="));
        }
        handle.release().await;
    }
}

#[tokio::test]
async fn native_mbim_real_factory_preserves_pap_chap_and_rejects_both_before_io() {
    for method in [ImsApnAuthMethod::Pap, ImsApnAuthMethod::Chap, ImsApnAuthMethod::PapOrChap] {
        let (device, io) = memory_device(None);
        let mut spec = device.spec.clone();
        spec.protocol = NativeProtocol::Mbim;
        let transport = NativeImsTransport::new(NativeDevice::new(spec, io.clone()));
        let requirements = policy(method);
        let result = start(&transport, &requirements).await;
        if method == ImsApnAuthMethod::PapOrChap {
            assert_eq!(result.err().unwrap().kind, ImsBearerErrorKind::RequirementsUnsupported);
            assert!(io.requests.lock().unwrap().is_empty());
            assert!(io.receipts.lock().unwrap().is_empty());
            continue;
        }
        let (_, handle) = result.unwrap();
        {
            let requests = io.requests.lock().unwrap();
            let action = requests.iter().flat_map(|r| r.arguments.iter()).find(|a| a.starts_with("--connect=")).unwrap();
            assert!(action.contains("session-id=2"));
            assert!(action.contains("context-type=ims"));
            let auth = if method == ImsApnAuthMethod::Pap { "pap" } else { "chap" };
            assert!(action.contains(&format!("auth={auth},username=fixture-username,password=fixture-password")));
        }
        handle.release().await;
        assert!(io.receipts.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn policy_and_cli_grammar_failures_make_zero_radio_profile_or_session_calls() {
    let (device, io) = memory_device(None);
    let transport = NativeImsTransport::new(device);
    let mut cases = Vec::new();
    let mut invalid = policy(ImsApnAuthMethod::Pap);
    invalid.authentication.username = Some("user,password=wrong".into());
    cases.push((invalid, ImsBearerErrorKind::RequirementsInvalid));
    let mut missing = policy(ImsApnAuthMethod::Chap);
    missing.authentication.password = None;
    cases.push((missing, ImsBearerErrorKind::RequirementsInvalid));
    for policy in [PcoRequestPolicy::Required, PcoRequestPolicy::Disabled] {
        cases.push((ImsBearerRequirements { pcscf_via_pco: policy, ..Default::default() },
            ImsBearerErrorKind::RequirementsUnsupported));
    }
    for (requirements, expected) in cases {
        assert_eq!(start(&transport, &requirements).await.err().unwrap().kind, expected);
    }
    assert!(io.requests.lock().unwrap().is_empty());
    assert!(io.receipts.lock().unwrap().is_empty());
    // This gate only promises information requests, never DHCP addressing.
    assert_eq!(transport.ims_bearer_capabilities().dhcp_information,
        if cfg!(target_os="linux") { CapabilitySupport::Supported } else { CapabilitySupport::Unsupported });
    let _ = DhcpPcscfPolicy::InformationRequest;
}

#[tokio::test]
async fn real_native_handle_reads_only_retained_client_without_resource_mutation() {
    let (device, io) = memory_device(None);
    let transport = NativeImsTransport::new(device.clone());
    let (_, mut handle) = start(&transport, &DEFAULT_IMS_BEARER_REQUIREMENTS).await.unwrap();
    *io.pcscf_result.lock().unwrap() = Some(Ok(super::super::super::io::RetainedWdsSettings {
        addresses: vec!["192.0.2.20".parse().unwrap()], domains: vec!["pcscf.example.test".into()],
    }));
    let before = io.requests.lock().unwrap().clone();
    let receipts = io.receipts.lock().unwrap().clone();
    let found = handle.discover_pcscf().await.unwrap().unwrap();
    assert_eq!(found.candidates, vec!["192.0.2.20".parse::<IpAddr>().unwrap()]);
    assert_eq!(found.domains, vec!["pcscf.example.test"]);
    assert_eq!(found.source, "retained_wds_current_settings");
    assert!(!found.ipv6_complete, "the pinned WDS schema must not suppress existing IPv6 observation");
    assert_eq!(*io.pcscf_calls.lock().unwrap(), vec![17]);
    assert_eq!(*io.requests.lock().unwrap(), before);
    assert_eq!(*io.receipts.lock().unwrap(), receipts);
    handle.release().await;
}

#[tokio::test]
async fn real_native_handle_distinguishes_absent_unsupported_malformed_and_lost() {
    let (device, io) = memory_device(None);
    let transport = NativeImsTransport::new(device);
    let (_, mut handle) = start(&transport, &DEFAULT_IMS_BEARER_REQUIREMENTS).await.unwrap();
    assert!(handle.discover_pcscf().await.unwrap().is_none());
    *io.pcscf_result.lock().unwrap() = Some(Ok(Default::default()));
    let empty = handle.discover_pcscf().await.unwrap().unwrap();
    assert!(empty.candidates.is_empty() && empty.domains.is_empty());
    *io.pcscf_result.lock().unwrap() = Some(Err(NativeError::Protocol("fixture_malformed".into())));
    assert_eq!(handle.discover_pcscf().await.err().unwrap().kind, ImsBearerErrorKind::PcscfUnavailable);
    *io.pcscf_result.lock().unwrap() = Some(Err(NativeError::OwnerConflict("fixture_owner_changed".into())));
    assert_eq!(handle.discover_pcscf().await.err().unwrap().kind, ImsBearerErrorKind::SessionLost);
    handle.release().await;
}
