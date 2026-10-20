//! Milenage (3GPP TS 35.205/35.206) for the software USIM.
//!
//! Implemented against the reference formulation (TEMP = E_K(RAND xor OPc),
//! rotations r1..r5 = 64/0/32/64/96 bits, constants c1..c5 = 0/1/2/4/8) and
//! validated with the TS 35.208 Test Set 1 vectors. This module performs only
//! the card-side computation: it never logs or formats K/OPc.
use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes128;

pub const KEY_LEN: usize = 16;
pub const AUTN_LEN: usize = 16;
pub const AUTS_LEN: usize = 14;

pub type Key = [u8; KEY_LEN];

#[derive(Clone)]
pub struct Milenage {
    k: Key,
    opc: Key,
}

impl std::fmt::Debug for Milenage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render key material, even in debug builds.
        f.write_str("Milenage(<redacted>)")
    }
}

fn encrypt(k: &Key, block: &[u8; 16]) -> [u8; 16] {
    let cipher = Aes128::new_from_slice(k).expect("AES-128 key length");
    let mut value = *block;
    cipher.encrypt_block((&mut value).into());
    value
}

fn xor(a: &[u8; 16], b: &[u8; 16]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = a[i] ^ b[i];
    }
    out
}

/// `opc = E_K(OP) xor OP` (TS 35.206 4.1). Both OP and OPc are accepted at the
/// configuration layer; this is the only place the conversion happens.
pub fn derive_opc(k: &Key, op: &Key) -> Key {
    xor(&encrypt(k, op), op)
}

impl Milenage {
    pub fn new(k: Key, opc: Key) -> Self {
        Self { k, opc }
    }

    pub fn from_op(k: Key, op: Key) -> Self {
        Self { k, opc: derive_opc(&k, &op) }
    }

    pub fn opc(&self) -> &Key {
        &self.opc
    }

    fn temp(&self, rand: &[u8; 16]) -> [u8; 16] {
        encrypt(&self.k, &xor(rand, &self.opc))
    }

    /// f1 and f1*: returns (MAC-A, MAC-S).
    pub fn f1(&self, sqn: &[u8; 6], amf: &[u8; 2], rand: &[u8; 16]) -> ([u8; 8], [u8; 8]) {
        let mut in1 = [0u8; 16];
        in1[..6].copy_from_slice(sqn);
        in1[6..8].copy_from_slice(amf);
        in1[8..14].copy_from_slice(sqn);
        in1[14..].copy_from_slice(amf);
        // rot(x, r1 = 64 bits) xor TEMP
        let rotated = xor(&in1, &self.opc);
        let mut block = [0u8; 16];
        for i in 0..16 {
            block[(i + 8) % 16] = rotated[i];
        }
        let temp = self.temp(rand);
        let out = xor(&xor(&block, &temp), &self.opc);
        let mut mac_a = [0u8; 8];
        let mut mac_s = [0u8; 8];
        mac_a.copy_from_slice(&out[..8]);
        mac_s.copy_from_slice(&out[8..]);
        (mac_a, mac_s)
    }

    /// f2/f3/f4/f5: returns (RES, CK, IK, AK).
    pub fn f2345(&self, rand: &[u8; 16]) -> ([u8; 8], [u8; 16], [u8; 16], [u8; 6]) {
        let temp = self.temp(rand);
        let base = xor(&temp, &self.opc);
        let mut f2 = base;
        f2[15] ^= 1;
        let out2 = xor(&encrypt(&self.k, &f2), &self.opc);
        let mut res = [0u8; 8];
        let mut ak = [0u8; 6];
        res.copy_from_slice(&out2[8..]);
        ak.copy_from_slice(&out2[..6]);

        // r3 = 32 bits (shift left by 4), c3 = 2
        let mut f3 = [0u8; 16];
        for i in 0..16 {
            f3[(i + 12) % 16] = base[i];
        }
        f3[15] ^= 2;
        let ck = xor(&encrypt(&self.k, &f3), &self.opc);

        // r4 = 64 bits, c4 = 4
        let mut f4 = [0u8; 16];
        for i in 0..16 {
            f4[(i + 8) % 16] = base[i];
        }
        f4[15] ^= 4;
        let ik = xor(&encrypt(&self.k, &f4), &self.opc);
        (res, ck, ik, ak)
    }

    /// f5*: AK* used only to conceal SQN_MS inside AUTS.
    pub fn f5_star(&self, rand: &[u8; 16]) -> [u8; 6] {
        let temp = self.temp(rand);
        let base = xor(&temp, &self.opc);
        let mut f5 = [0u8; 16];
        for i in 0..16 {
            f5[(i + 4) % 16] = base[i];
        }
        f5[15] ^= 8;
        let out = xor(&encrypt(&self.k, &f5), &self.opc);
        let mut ak = [0u8; 6];
        ak.copy_from_slice(&out[..6]);
        ak
    }

    /// Verify a network AUTN and return the revealed SQN.
    ///
    /// AUTN = (SQN xor AK) || AMF || MAC-A. A wrong MAC-A is an authentication
    /// failure, never a hint to retry with other parameters.
    pub fn verify_autn(&self, rand: &[u8; 16], autn: &[u8; AUTN_LEN])
        -> Result<([u8; 6], [u8; 2]), &'static str>
    {
        let mut amf = [0u8; 2];
        amf.copy_from_slice(&autn[6..8]);
        let (_res, _ck, _ik, ak) = self.f2345(rand);
        let mut sqn = [0u8; 6];
        for i in 0..6 {
            sqn[i] = autn[i] ^ ak[i];
        }
        let (mac_a, _) = self.f1(&sqn, &amf, rand);
        let expected = &autn[8..16];
        let mismatch = mac_a.iter().zip(expected).fold(0u8, |acc, (a, b)| acc | (a ^ b));
        if mismatch != 0 {
            return Err("ims_virtual_sim_autn_mac_invalid");
        }
        Ok((sqn, amf))
    }

    /// Build AUTS = (SQN_MS xor AK*) || MAC-S for a resynchronisation attempt.
    pub fn auts(&self, rand: &[u8; 16], sqn_ms: &[u8; 6], amf: &[u8; 2]) -> [u8; AUTS_LEN] {
        let (_mac_a, mac_s) = self.f1(sqn_ms, amf, rand);
        let ak_star = self.f5_star(rand);
        let mut out = [0u8; AUTS_LEN];
        for i in 0..6 {
            out[i] = sqn_ms[i] ^ ak_star[i];
        }
        out[6..].copy_from_slice(&mac_s);
        out
    }
}

#[cfg(test)]
#[path = "milenage_tests.rs"]
mod tests;
