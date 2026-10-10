use super::*;
fn identity(user: &str) -> LiveImsRegisterIdentity {
    LiveImsRegisterIdentity {
        shared: crate::connectivity::core::context::ImsIdentity {
            private_user: format!("{user}@ims.example"), public_uri: format!("sip:{user}@ims.example"),
            contact_user: user.into(), home_domain: "ims.example".into(), contact_user_phone: false,
        },
        shape: "isim",
    }
}
fn target() -> LiveImsTarget {
    LiveImsTarget { domain: "ims.example".into(), realm: "ims.example".into(), registrar: None, pcscf: vec![] }
}
#[test]
fn isim_vowifi_format_fallback_never_overwrites_provisioned_identity() {
    let base = identity("alice");
    for format in [LiveRegisterIdentityFormat::ImsiHomeDomain,
        LiveRegisterIdentityFormat::PrefixedImsiHomeDomain, LiveRegisterIdentityFormat::ImsiPhoneUri,
        LiveRegisterIdentityFormat::MsisdnPhoneUri] {
        let selected = format_identity(&base, &target(), true, format, Some("+12025550123".into())).unwrap();
        assert_eq!(selected.private_user, base.private_user);
        assert_eq!(selected.public_uri, base.public_uri);
        assert_eq!(selected.contact_user, base.contact_user);
        assert_eq!(selected.shape, "isim");
    }
    // A missing MSISDN does not trigger a probe/error for a configured ISIM.
    assert!(format_identity(&base, &target(), true, LiveRegisterIdentityFormat::MsisdnPhoneUri, None).is_ok());
}
#[test]
fn isim_vowifi_unprovisioned_keeps_existing_usim_format_candidates() {
    let base = identity("001010123456789");
    let selected = format_identity(&base, &target(), false, LiveRegisterIdentityFormat::PrefixedImsiHomeDomain, None).unwrap();
    assert_eq!(selected.private_user, "0001010123456789@ims.example");
    let selected = format_identity(&base, &target(), false, LiveRegisterIdentityFormat::MsisdnPhoneUri, Some("+12025550123".into())).unwrap();
    assert_eq!(selected.private_user, base.private_user);
    assert_eq!(selected.public_uri, "sip:+12025550123@ims.example;user=phone");
    assert!(selected.contact_user_phone);
}
#[test]
fn isim_vowifi_eap_entrypoint_remains_usim_and_ims_is_separate() {
    // Static contract guard supplements bound-AKA mock tests without device I/O.
    let source = include_str!("live.rs");
    let eap = source.split("pub async fn authenticate_live_sim_for_line(").nth(1).unwrap()
        .split("fn read_non_empty_config").next().unwrap();
    assert!(eap.contains("USIM_AID_PREFIX"));
    assert!(!eap.contains("LiveImsSelection"));
    assert!(source.contains("context.uicc.as_ref()"));
}
#[test]
fn isim_pcsc_ims_uses_captured_aid_and_eap_keeps_usim_default() {
    let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/hardware/devices/pcsc/mod.rs"));
    assert!(source.contains("authenticate_with_aid(path, USIM_AID_PREFIX, rand, autn)"));
    assert!(source.contains("&[select_application_apdu(aid)?, authenticate]"));
    let ims = include_str!("live_identity.rs");
    assert!(ims.contains("pcsc::authenticate_with_aid(&device.pcsc_reader, &aid"));
}
