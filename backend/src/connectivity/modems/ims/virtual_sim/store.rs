//! Five-code storage: strict parsing and the on-disk record format.
//!
//! Records are stored as plaintext on the host by explicit product decision:
//! the subscription data already lives on this machine, and decrypting on every
//! read only added latency. Keys are still never logged or returned by the API.
use super::milenage::{self, Key, Milenage};

const FORMAT: u8 = 1;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OpMaterial {
    /// Operator variant already derived with Ki.
    Opc(Key),
    /// Raw OP; OPc is derived with Ki when needed.
    Op(Key),
}

impl std::fmt::Debug for OpMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Opc(_) => "OpMaterial::Opc(<redacted>)",
            Self::Op(_) => "OpMaterial::Op(<redacted>)",
        })
    }
}

/// The provisioning data an operator/MVNO hands out for one subscription.
#[derive(Clone, PartialEq, Eq)]
pub struct FiveCodes {
    pub iccid: String,
    pub imsi: String,
    ki: Key,
    op: OpMaterial,
    pub msisdn: Option<String>,
}

impl std::fmt::Debug for FiveCodes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // ICCID/IMSI are needed for identification; keys never are.
        f.debug_struct("FiveCodes")
            .field("iccid_tail", &self.iccid.get(self.iccid.len().saturating_sub(4)..).unwrap_or(""))
            .field("imsi_prefix", &self.imsi.get(..6.min(self.imsi.len())).unwrap_or(""))
            .field("op_kind", &match self.op { OpMaterial::Opc(_) => "opc", OpMaterial::Op(_) => "op" })
            .field("has_msisdn", &self.msisdn.is_some())
            .finish()
    }
}

fn digits(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_digit())
}

impl FiveCodes {
    pub fn new(iccid: &str, imsi: &str, ki: Key, op: OpMaterial, msisdn: Option<&str>)
        -> Result<Self, &'static str>
    {
        let iccid = iccid.trim().replace(' ', "");
        let imsi = imsi.trim().to_owned();
        if !digits(&iccid, 18, 22) {
            return Err("ims_virtual_sim_iccid_invalid");
        }
        if !digits(&imsi, 5, 16) {
            return Err("ims_virtual_sim_imsi_invalid");
        }
        let msisdn = msisdn.map(str::trim).filter(|v| !v.is_empty()).map(|v| {
            v.strip_prefix('+').unwrap_or(v).to_owned()
        });
        if let Some(number) = msisdn.as_deref() {
            if !digits(number, 5, 15) {
                return Err("ims_virtual_sim_msisdn_invalid");
            }
        }
        Ok(Self { iccid, imsi, ki, op, msisdn })
    }

    pub fn ki(&self) -> &Key { &self.ki }

    /// Whether the stored material is already OPc (no derivation needed).
    pub fn is_opc(&self) -> bool { matches!(self.op, OpMaterial::Opc(_)) }

    /// The material exactly as provisioned: OP when the record holds OP.
    pub fn raw_op(&self) -> Key {
        match self.op {
            OpMaterial::Opc(key) | OpMaterial::Op(key) => key,
        }
    }

    /// Resolved OPc (deriving from OP when that is what was provisioned).
    pub fn opc(&self) -> Key {
        match self.op {
            OpMaterial::Opc(opc) => opc,
            OpMaterial::Op(op) => milenage::derive_opc(&self.ki, &op),
        }
    }

    pub fn milenage(&self) -> Milenage {
        Milenage::new(self.ki, self.opc())
    }

    /// Accepting both OP and OPc is a configuration convenience only; a record
    /// that carries both must match, so a typo cannot silently change identity.
    pub fn from_op_and_opc(iccid: &str, imsi: &str, ki: Key, op: Option<Key>, opc: Option<Key>,
        msisdn: Option<&str>) -> Result<Self, &'static str>
    {
        let material = match (op, opc) {
            (Some(op), Some(opc)) => {
                if milenage::derive_opc(&ki, &op) != opc {
                    return Err("ims_virtual_sim_op_opc_mismatch");
                }
                OpMaterial::Opc(opc)
            }
            (Some(op), None) => OpMaterial::Op(op),
            (None, Some(opc)) => OpMaterial::Opc(opc),
            (None, None) => return Err("ims_virtual_sim_op_missing"),
        };
        Self::new(iccid, imsi, ki, material, msisdn)
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = vec![FORMAT];
        for text in [Some(self.iccid.as_str()), Some(self.imsi.as_str()), self.msisdn.as_deref()] {
            match text {
                Some(value) => {
                    out.push(value.len() as u8);
                    out.extend_from_slice(value.as_bytes());
                }
                None => out.push(0),
            }
        }
        out.extend_from_slice(&self.ki);
        match &self.op {
            OpMaterial::Opc(key) => { out.push(0); out.extend_from_slice(key); }
            OpMaterial::Op(key) => { out.push(1); out.extend_from_slice(key); }
        }
        out
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        let mut cursor = 0usize;
        let mut take = |len: usize| -> Result<&[u8], &'static str> {
            let slice = bytes.get(cursor..cursor + len).ok_or("ims_virtual_sim_record_truncated")?;
            cursor += len;
            Ok(slice)
        };
        if take(1)?[0] != FORMAT {
            return Err("ims_virtual_sim_record_version_unsupported");
        }
        let mut texts: [Option<String>; 3] = [None, None, None];
        for slot in texts.iter_mut() {
            let len = take(1)?[0] as usize;
            if len == 0 { continue; }
            let value = take(len)?;
            if !value.iter().all(|b| b.is_ascii_graphic()) {
                return Err("ims_virtual_sim_record_invalid");
            }
            *slot = Some(String::from_utf8(value.to_vec()).map_err(|_| "ims_virtual_sim_record_invalid")?);
        }
        let mut ki = [0u8; 16];
        ki.copy_from_slice(take(16)?);
        let kind = take(1)?[0];
        let mut op = [0u8; 16];
        op.copy_from_slice(take(16)?);
        if cursor != bytes.len() {
            return Err("ims_virtual_sim_record_trailing");
        }
        let material = match kind {
            0 => OpMaterial::Opc(op),
            1 => OpMaterial::Op(op),
            _ => return Err("ims_virtual_sim_record_invalid"),
        };
        Self::new(texts[0].as_deref().unwrap_or_default(), texts[1].as_deref().unwrap_or_default(),
            ki, material, texts[2].as_deref())
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
