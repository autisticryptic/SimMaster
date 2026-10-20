//! Five-code AKA behaviour and per-line installation. Actions-only.
use super::*;
use crate::connectivity::modems::ims::virtual_sim::store::OpMaterial;

const ICCID: &str = "8944000000000000001";
const IMSI: &str = "001010123456789";
const KI: &str = "465b5ce8b199b49faa5f0a2ee238a6bc";
const OP: &str = "cdc202d5123e20f62b6d676ac72cb318";
const RAND: &str = "23553cbe9637a89d218ae64dae47bf35";
const SQN: &str = "ff9bb4d0b607";
const AMF: &str = "b9b9";

fn hex16(value: &str) -> [u8; 16] {
    let mut out = [0u8; 16];
    for i in 0..16 { out[i] = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).unwrap(); }
    out
}

fn sim(ki: [u8; 16]) -> Arc<VirtualSim> {
    let codes = FiveCodes::from_op_and_opc(ICCID, IMSI, ki, Some(hex16(OP)), None, None).unwrap();
    VirtualSim::new("profile-a", codes)
}

/// Build a network AUTN for this subscription (SQN || AMF || MAC-A).
fn autn(profile: &VirtualSim, sqn: &str) -> [u8; 16] {
    let mut sqn_bytes = [0u8; 6];
    for i in 0..6 { sqn_bytes[i] = u8::from_str_radix(&sqn[i * 2..i * 2 + 2], 16).unwrap(); }
    let mut amf = [0u8; 2];
    amf[0] = u8::from_str_radix(&AMF[0..2], 16).unwrap();
    amf[1] = u8::from_str_radix(&AMF[2..4], 16).unwrap();
    let m = profile.codes().milenage();
    let (mac_a, _) = m.f1(&sqn_bytes, &amf, &hex16(RAND));
    let (_res, _ck, _ik, ak) = m.f2345(&hex16(RAND));
    let mut out = [0u8; 16];
    for i in 0..6 { out[i] = sqn_bytes[i] ^ ak[i]; }
    out[6..8].copy_from_slice(&amf);
    out[8..].copy_from_slice(&mac_a);
    out
}

#[test]
fn a_valid_autn_yields_the_published_res_ck_and_ik() {
    let profile = sim(hex16(KI));
    let out = profile.authenticate(&hex16(RAND), &autn(&profile, SQN)).unwrap();
    assert_eq!(out.res, vec![0xa5, 0x42, 0x11, 0xd5, 0xe3, 0xba, 0x50, 0xbf]);
    assert_eq!(out.ck[..4], [0xb4, 0x0b, 0xa9, 0xa3]);
    assert!(out.ik.len() == 16 && out.auts.is_none());
}

#[test]
fn a_replayed_sequence_number_triggers_auts_instead_of_success() {
    let profile = sim(hex16(KI));
    let challenge = autn(&profile, SQN);
    assert!(profile.authenticate(&hex16(RAND), &challenge).unwrap().auts.is_none());
    // The same SQN again must not be accepted twice.
    let replayed = profile.authenticate(&hex16(RAND), &challenge).unwrap();
    assert!(replayed.res.is_empty());
    assert_eq!(replayed.auts.as_ref().map(Vec::len), Some(14));
}

#[test]
fn a_wrong_subscriber_key_produces_auts_not_a_response() {
    let profile = sim(hex16("000102030405060708090a0b0c0d0e0f"));
    let out = profile.authenticate(&hex16(RAND), &autn(&sim(hex16(KI)), SQN)).unwrap();
    assert!(out.res.is_empty() && out.ck.is_empty() && out.ik.is_empty());
    assert_eq!(out.auts.as_ref().map(Vec::len), Some(14));
}

#[test]
fn malformed_challenges_are_rejected_without_state_change() {
    let profile = sim(hex16(KI));
    assert_eq!(profile.authenticate(&[0u8; 8], &[0u8; 16]).err(), Some("ims_virtual_sim_rand_invalid"));
    assert_eq!(profile.authenticate(&hex16(RAND), &[0u8; 8]).err(), Some("ims_virtual_sim_autn_invalid"));
}

#[test]
fn one_profile_per_line_and_replacement_must_be_explicit() {
    clear_for_test();
    let profile = sim(hex16(KI));
    install_for_line("wlan-line-1", profile.clone()).unwrap();
    assert!(Arc::ptr_eq(&for_line("wlan-line-1").unwrap(), &profile));
    assert_eq!(install_for_line("wlan-line-1", profile.clone()).err(), Some("ims_virtual_sim_profile_duplicate"));
    assert_eq!(install_for_line("", profile.clone()).err(), Some("ims_virtual_sim_profile_invalid"));
    assert!(remove_for_line("wlan-line-1"));
    assert!(for_line("wlan-line-1").is_none());
    assert!(!remove_for_line("wlan-line-1"));
    clear_for_test();
}

#[test]
fn debug_never_prints_keys_or_full_identifiers() {
    let profile = sim(hex16(KI));
    let text = format!("{profile:?}");
    assert!(text.contains("VirtualSim") && text.contains("profile-a"));
    for secret in [KI, OP, IMSI] {
        assert!(!text.contains(secret), "debug leaked {secret}");
    }
    let output = profile.authenticate(&hex16(RAND), &autn(&profile, SQN)).unwrap();
    let rendered = format!("{output:?}");
    assert!(!rendered.contains("a54211d5") && !rendered.contains("b40ba9a3"));
}

#[test]
fn op_material_variants_resolve_to_the_same_milenage() {
    let with_op = FiveCodes::from_op_and_opc(ICCID, IMSI, hex16(KI), Some(hex16(OP)), None, None).unwrap();
    let with_opc = FiveCodes::from_op_and_opc(ICCID, IMSI, hex16(KI), None,
        Some(with_op.opc()), None).unwrap();
    assert_eq!(with_op.opc(), with_opc.opc());
    let _ = OpMaterial::Opc(with_op.opc());
}
