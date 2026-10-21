//! Five-code parsing and at-rest encryption. Actions-only; no database.
use super::*;
use crate::connectivity::modems::ims::virtual_sim::milenage::Milenage;

const ICCID: &str = "8944000000000000001";
const IMSI: &str = "001010123456789";
const KI: &str = "465b5ce8b199b49faa5f0a2ee238a6bc";
const OP: &str = "cdc202d5123e20f62b6d676ac72cb318";
const OPC: &str = "cd63cb71954a9f4e48a5994e37a02baf";

fn key16(value: &str) -> [u8; 16] {
    let mut out = [0u8; 16];
    for i in 0..16 { out[i] = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).unwrap(); }
    out
}

fn codes() -> FiveCodes {
    FiveCodes::new(ICCID, IMSI, key16(KI), OpMaterial::Opc(key16(OPC)), Some("+8613800138000")).unwrap()
}

#[test]
fn opc_is_derived_when_only_op_is_provided() {
    let from_op = FiveCodes::from_op_and_opc(ICCID, IMSI, key16(KI), Some(key16(OP)), None, None).unwrap();
    assert_eq!(from_op.opc(), key16(OPC));
    let from_opc = FiveCodes::from_op_and_opc(ICCID, IMSI, key16(KI), None, Some(key16(OPC)), None).unwrap();
    assert_eq!(from_opc.opc(), key16(OPC));
}

#[test]
fn mismatched_op_and_opc_is_rejected_instead_of_guessed() {
    let wrong_opc = key16("00000000000000000000000000000000");
    assert_eq!(FiveCodes::from_op_and_opc(ICCID, IMSI, key16(KI), Some(key16(OP)), Some(wrong_opc), None).err(),
        Some("ims_virtual_sim_op_opc_mismatch"));
    assert_eq!(FiveCodes::from_op_and_opc(ICCID, IMSI, key16(KI), None, None, None).err(),
        Some("ims_virtual_sim_op_missing"));
}

#[test]
fn invalid_identifiers_are_rejected() {
    for (iccid, imsi, msisdn, expected) in [
        ("123", IMSI, None, "ims_virtual_sim_iccid_invalid"),
        (ICCID, "12ab", None, "ims_virtual_sim_imsi_invalid"),
        (ICCID, IMSI, Some("+86abc"), "ims_virtual_sim_msisdn_invalid"),
    ] {
        assert_eq!(FiveCodes::new(iccid, imsi, key16(KI), OpMaterial::Opc(key16(OPC)), msisdn).err(),
            Some(expected));
    }
    // Spaces in the ICCID are a printing artefact, not a different card.
    let spaced = FiveCodes::new("8944 0000 0000 0000 001", IMSI, key16(KI),
        OpMaterial::Opc(key16(OPC)), None).unwrap();
    assert_eq!(spaced.iccid, ICCID);
}


#[test]
fn debug_never_prints_key_material() {
    let codes = codes();
    for text in [format!("{codes:?}"), format!("{:?}", codes.opc())] {
        assert!(!text.contains(KI) && !text.contains(OPC) && !text.contains(OP));
    }
}

#[test]
fn derived_milenage_matches_the_published_vector() {
    // Test Set 1 from TS 35.208 with OP-only provisioning.
    let codes = FiveCodes::from_op_and_opc(ICCID, IMSI, key16(KI), Some(key16(OP)), None, None).unwrap();
    let m: Milenage = codes.milenage();
    let mut rand = [0u8; 16];
    for i in 0..16 { rand[i] = u8::from_str_radix(&"23553cbe9637a89d218ae64dae47bf35"[i * 2..i * 2 + 2], 16).unwrap(); }
    let (res, ck, _ik, ak) = m.f2345(&rand);
    assert_eq!(res.to_vec(), vec![0xa5, 0x42, 0x11, 0xd5, 0xe3, 0xba, 0x50, 0xbf]);
    assert_eq!(ck[..4], [0xb4, 0x0b, 0xa9, 0xa3]);
    assert_eq!(ak.to_vec(), vec![0xaa, 0x68, 0x9c, 0x64, 0x83, 0x70]);
}
