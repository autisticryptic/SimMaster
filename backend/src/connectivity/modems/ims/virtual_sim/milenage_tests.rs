//! TS 35.208 Test Set 1 vectors plus card-side error paths. Actions-only.
use super::*;

fn hex(value: &str) -> Vec<u8> {
    (0..value.len()).step_by(2).map(|i| u8::from_str_radix(&value[i..i + 2], 16).unwrap()).collect()
}
fn key(value: &str) -> Key {
    let mut out = [0u8; 16];
    out.copy_from_slice(&hex(value));
    out
}

const K: &str = "465b5ce8b199b49faa5f0a2ee238a6bc";
const RAND: &str = "23553cbe9637a89d218ae64dae47bf35";
const SQN: &str = "ff9bb4d0b607";
const AMF: &str = "b9b9";
const OP: &str = "cdc202d5123e20f62b6d676ac72cb318";
const OPC: &str = "cd63cb71954a9f4e48a5994e37a02baf";

fn rand() -> [u8; 16] { key(RAND) }
fn sqn() -> [u8; 6] { let mut out = [0u8; 6]; out.copy_from_slice(&hex(SQN)); out }
fn amf() -> [u8; 2] { let mut out = [0u8; 2]; out.copy_from_slice(&hex(AMF)); out }

#[test]
fn opc_is_derived_from_op_with_the_published_vector() {
    assert_eq!(derive_opc(&key(K), &key(OP)), key(OPC));
}

#[test]
fn f1_matches_the_published_mac_a_and_mac_s() {
    let m = Milenage::new(key(K), key(OPC));
    let (mac_a, mac_s) = m.f1(&sqn(), &amf(), &rand());
    assert_eq!(mac_a.to_vec(), hex("4a9ffac354dfafb3"));
    assert_eq!(mac_s.to_vec(), hex("01cfaf9ec4e871e9"));
}

#[test]
fn f2345_match_the_published_res_ck_ik_ak() {
    let m = Milenage::new(key(K), key(OPC));
    let (res, ck, ik, ak) = m.f2345(&rand());
    assert_eq!(res.to_vec(), hex("a54211d5e3ba50bf"));
    assert_eq!(ck.to_vec(), hex("b40ba9a3c58b2a05bbf0d987b21bf8cb"));
    assert_eq!(ik.to_vec(), hex("f769bcd751044604127672711c6d3441"));
    assert_eq!(ak.to_vec(), hex("aa689c648370"));
}

#[test]
fn f5_star_matches_the_published_ak_star() {
    let m = Milenage::new(key(K), key(OPC));
    assert_eq!(m.f5_star(&rand()).to_vec(), hex("451e8beca43b"));
}

#[test]
fn autn_is_accepted_only_with_a_matching_mac() {
    let m = Milenage::new(key(K), key(OPC));
    let (mac_a, _) = m.f1(&sqn(), &amf(), &rand());
    let (_res, _ck, _ik, ak) = m.f2345(&rand());
    let mut autn = [0u8; AUTN_LEN];
    for i in 0..6 { autn[i] = sqn()[i] ^ ak[i]; }
    autn[6..8].copy_from_slice(&amf());
    autn[8..].copy_from_slice(&mac_a);
    assert_eq!(m.verify_autn(&rand(), &autn).unwrap(), (sqn(), amf()));

    let mut tampered = autn;
    tampered[15] ^= 1;
    assert_eq!(m.verify_autn(&rand(), &tampered), Err("ims_virtual_sim_autn_mac_invalid"));
    // A different subscriber key must not validate the same AUTN.
    let other = Milenage::new(key("000102030405060708090a0b0c0d0e0f"), key(OPC));
    assert!(other.verify_autn(&rand(), &autn).is_err());
}

#[test]
fn auts_conceals_sqn_ms_and_carries_mac_s() {
    let m = Milenage::new(key(K), key(OPC));
    let auts = m.auts(&rand(), &sqn(), &amf());
    assert_eq!(auts.len(), AUTS_LEN);
    let ak_star = m.f5_star(&rand());
    for i in 0..6 { assert_eq!(auts[i] ^ ak_star[i], sqn()[i]); }
    let (_mac_a, mac_s) = m.f1(&sqn(), &amf(), &rand());
    assert_eq!(&auts[6..], &mac_s[..]);
}

#[test]
fn debug_never_prints_key_material() {
    let m = Milenage::new(key(K), key(OPC));
    let text = format!("{m:?}");
    assert_eq!(text, "Milenage(<redacted>)");
    for secret in [K, OPC, OP] {
        assert!(!text.contains(secret), "debug leaked {secret}");
    }
}

#[test]
fn aes_kernel_matches_the_published_rijndael_vector() {
    // 3GPP TS 35.207 section 3.3 Test Set 1 (Rijndael kernel, AES-128).
    let block: [u8; 16] = hex("ee36f7cf037d37d3692f7f0399e7949a").try_into().unwrap();
    assert_eq!(encrypt(&key("465b5ce8b199b49faa5f0a2ee238a6bc"), &block).to_vec(),
        hex("9e2980c59739da67b136355e3cede6a2"));
}
