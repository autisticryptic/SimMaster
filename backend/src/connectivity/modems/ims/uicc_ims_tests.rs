use super::*;
fn configured() -> IsimImsMaterial {
    IsimImsMaterial {
        impi: Some("alice@ims.example".into()), domain: Some("ims.example".into()),
        impus: vec!["sip:alice@ims.example".into(), "tel:+12025550123".into()],
        pcscf: vec!["192.0.2.1".into()],
    }
}
fn resolve(material: &IsimImsMaterial, explicit_domain: bool, explicit_realm: bool) -> ResolvedImsIdentity {
    resolve_ims_identity("001010123456789", USIM_AID_PREFIX, Some((isim::ISIM_AID_PREFIX, material)),
        "policy.example", "realm.example", explicit_domain, explicit_realm).unwrap()
}
#[test]
fn isim_complete_identity_and_aka_application_are_selected_together() {
    let selected = resolve(&configured(), false, false);
    assert_eq!(selected.source, ImsIdentitySource::Isim);
    assert_eq!(selected.identity.private_user, "alice@ims.example");
    assert_eq!(selected.identity.public_uri, "sip:alice@ims.example");
    assert_eq!(selected.identity.home_domain, "ims.example");
    assert_eq!(selected.realm, "ims.example");
    assert_eq!(selected.auth_aid, isim::ISIM_AID_PREFIX);
    assert_eq!(selected.isim_pcscf, ["192.0.2.1"]);
}
#[test]
fn isim_explicit_profile_domain_and_realm_do_not_rewrite_provisioned_identity() {
    let selected = resolve(&configured(), true, true);
    assert_eq!(selected.identity.private_user, "alice@ims.example");
    assert_eq!(selected.identity.public_uri, "sip:alice@ims.example");
    assert_eq!(selected.identity.home_domain, "policy.example");
    assert_eq!(selected.realm, "realm.example");
    let selected = resolve(&configured(), true, false);
    assert_eq!(selected.identity.home_domain, "policy.example");
    assert_eq!(selected.realm, "ims.example");
}
#[test]
fn isim_unconfigured_and_partial_material_fall_back_only_to_usim() {
    for material in [IsimImsMaterial::default(), IsimImsMaterial { domain: None, ..configured() },
        IsimImsMaterial { impi: None, ..configured() }, IsimImsMaterial { impus: vec![], ..configured() }] {
        let selected = resolve(&material, false, false);
        assert_eq!(selected.source, ImsIdentitySource::UsimDerived);
        assert_eq!(selected.auth_aid, USIM_AID_PREFIX);
        assert_eq!(selected.identity.private_user, "001010123456789@realm.example");
        assert_eq!(selected.identity.home_domain, "policy.example");
    }
}
#[test]
fn isim_corrupt_public_material_is_not_silently_unprovisioned() {
    let mut material = configured(); material.impi = Some("alice@ims.example\r\nX:x".into());
    assert!(resolve_ims_identity("001010123456789", USIM_AID_PREFIX,
        Some((isim::ISIM_AID_PREFIX, &material)), "policy.example", "realm.example", false, false).is_err());
    // Invalid even when another required field is missing.
    material.domain = None;
    assert!(resolve_ims_identity("001010123456789", USIM_AID_PREFIX,
        Some((isim::ISIM_AID_PREFIX, &material)), "policy.example", "realm.example", false, false).is_err());
}
#[test]
fn isim_binding_rejects_card_slot_endpoint_owner_and_imsi_changes() {
    let expected = UiccCardBinding {
        endpoint: "mock-reader".into(), slot: 1, iccid: "8900000000000000001".into(),
        imsi: "001010123456789".into(), owner: "mock-owner-1".into(),
    };
    assert_eq!(expected.verify(&expected.clone()), Ok(()));
    let mut changed = expected.clone(); changed.iccid.push('2');
    assert_eq!(expected.verify(&changed), Err("ims_uicc_binding_changed"));
    let mut changed = expected.clone(); changed.imsi.push('0');
    assert!(expected.verify(&changed).is_err());
    let mut changed = expected.clone(); changed.slot = 2;
    assert!(expected.verify(&changed).is_err());
    let mut changed = expected.clone(); changed.endpoint.push('2');
    assert!(expected.verify(&changed).is_err());
    let mut changed = expected.clone(); changed.owner.push('2');
    assert!(expected.verify(&changed).is_err());
    assert!(!format!("{expected:?}").contains("8900"));
}
#[tokio::test]
async fn isim_mock_aka_uses_captured_aid_and_rejects_swap_before_or_after() {
    let expected = UiccCardBinding {
        endpoint: "mock-reader".into(), slot: 1, iccid: "8900000000000000001".into(),
        imsi: "001010123456789".into(), owner: "mock-owner".into(),
    };
    let mut changed = expected.clone(); changed.iccid.push('2');
    for (before, after, should_authenticate) in [
        (expected.clone(), expected.clone(), true),
        (changed.clone(), expected.clone(), false),
        (expected.clone(), changed.clone(), true),
    ] {
        let should_succeed = before == expected && after == expected;
        let mut observations = std::collections::VecDeque::from([before, after]);
        let called = std::cell::Cell::new(false);
        let result = authenticate_bound_with(&expected, isim::ISIM_AID_PREFIX,
            || std::future::ready(Ok(observations.pop_front().unwrap())),
            |aid| {
                called.set(true);
                assert_eq!(aid, isim::ISIM_AID_PREFIX);
                std::future::ready(Ok(42))
            }).await;
        assert_eq!(called.get(), should_authenticate);
        assert_eq!(result.is_ok(), should_succeed);
    }
}

#[test]
fn isim_auth_aid_is_not_the_eap_usim_application() {
    let selected = resolve(&configured(), false, false);
    assert_ne!(selected.auth_aid, USIM_AID_PREFIX);
    let eap_aid = USIM_AID_PREFIX;
    let apdu = super::super::vowifi::qmi_uim::build_usim_authenticate_apdu(&[1; 16], &[2; 16]).unwrap();
    assert_eq!(apdu[3], 0x81); // Same AKA context, different selected application.
    assert_eq!(eap_aid.last(), Some(&2));
    assert_eq!(selected.auth_aid.last(), Some(&4));
}
