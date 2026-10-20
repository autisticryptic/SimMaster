//! Software USIM (five-code) support for host-side IMS.
//!
//! Scope: VoWiFi only. Cellular attach/PDN stay with the modem and its physical
//! card, so a virtual USIM never replaces a modem line's UICC.
//!
//! Layering:
//! - `milenage`: TS 35.206 card-side AKA math (validated with TS 35.208 vectors).
//! - `sqn`: per-profile 48-bit SQN store used for freshness and AUTS.
//! - `store`: at-rest encryption of the five codes.
//!
//! Non-goals: no ISO 7816 file system, no eUICC/EID handling, no modem APDU
//! interception (the baseband's card traffic is not visible to the host).
pub mod milenage;
pub mod sqn;
pub mod store;

pub use milenage::{Milenage, AUTN_LEN, AUTS_LEN, KEY_LEN};
pub use sqn::{Sqn, SQN_LEN};
pub use store::{open, seal, FiveCodes, MasterKey, OpMaterial};

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};


/// Result of a card-side AKA run. `auts` is set only for resynchronisation.
#[derive(Clone, PartialEq, Eq)]
pub struct AkaOutput {
    pub res: Vec<u8>,
    pub ck: Vec<u8>,
    pub ik: Vec<u8>,
    pub auts: Option<Vec<u8>>,
}

impl std::fmt::Debug for AkaOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AkaOutput")
            .field("res_len", &self.res.len())
            .field("ck_len", &self.ck.len())
            .field("ik_len", &self.ik.len())
            .field("auts", &self.auts.is_some())
            .finish()
    }
}

/// One installed virtual subscription: five codes plus its SQN state.
pub struct VirtualSim {
    profile_id: String,
    codes: FiveCodes,
    sqn: Mutex<Sqn>,
}

impl std::fmt::Debug for VirtualSim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VirtualSim")
            .field("profile_id", &self.profile_id)
            .field("iccid_tail", &self.codes.iccid.get(self.codes.iccid.len().saturating_sub(4)..).unwrap_or(""))
            .finish()
    }
}

impl VirtualSim {
    pub fn new(profile_id: &str, codes: FiveCodes) -> Arc<Self> {
        Arc::new(Self { profile_id: profile_id.to_owned(), codes, sqn: Mutex::new(Sqn::default()) })
    }

    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub fn codes(&self) -> &FiveCodes {
        &self.codes
    }

    /// Card-side AKA (TS 33.102): verify AUTN, return RES/CK/IK, or AUTS when
    /// the sequence number is not fresh or the MAC does not verify.
    pub fn authenticate(&self, rand: &[u8], autn: &[u8]) -> Result<AkaOutput, &'static str> {
        let rand: [u8; 16] = rand.try_into().map_err(|_| "ims_virtual_sim_rand_invalid")?;
        let autn: [u8; 16] = autn.try_into().map_err(|_| "ims_virtual_sim_autn_invalid")?;
        let mut amf = [0u8; 2];
        amf.copy_from_slice(&autn[6..8]);
        let milenage = self.codes.milenage();
        let mut sqn = self.sqn.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let resync = |sqn: &mut Sqn| -> AkaOutput {
            let auts = milenage.auts(&rand, &sqn.next().to_bytes(), &amf);
            AkaOutput { res: Vec::new(), ck: Vec::new(), ik: Vec::new(), auts: Some(auts.to_vec()) }
        };
        let Ok((network_sqn, _amf)) = milenage.verify_autn(&rand, &autn) else {
            // MAC mismatch: the network and the card disagree; resynchronise.
            return Ok(resync(&mut sqn));
        };
        let network = Sqn::from_bytes(&network_sqn);
        if !sqn.accepts(network) {
            // Replayed or out-of-order SQN is never accepted.
            return Ok(resync(&mut sqn));
        }
        sqn.advance(network);
        let (res, ck, ik, _ak) = milenage.f2345(&rand);
        Ok(AkaOutput { res: res.to_vec(), ck: ck.to_vec(), ik: ik.to_vec(), auts: None })
    }
}

static REGISTRY: OnceLock<Mutex<HashMap<String, Arc<VirtualSim>>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, Arc<VirtualSim>>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Bind one virtual subscription to a line. A line has at most one profile, so
/// a replacement must be explicit (remove first) instead of silently winning.
pub fn install_for_line(line_id: &str, profile: Arc<VirtualSim>) -> Result<(), &'static str> {
    if line_id.is_empty() || profile.profile_id().is_empty() {
        return Err("ims_virtual_sim_profile_invalid");
    }
    let mut map = registry().lock().map_err(|_| "ims_virtual_sim_profile_invalid")?;
    if map.contains_key(line_id) {
        return Err("ims_virtual_sim_profile_duplicate");
    }
    map.insert(line_id.to_owned(), profile);
    Ok(())
}

pub fn for_line(line_id: &str) -> Option<Arc<VirtualSim>> {
    registry().lock().ok()?.get(line_id).cloned()
}

pub fn remove_for_line(line_id: &str) -> bool {
    registry().lock().map(|mut map| map.remove(line_id).is_some()).unwrap_or(false)
}

#[cfg(test)]
pub(crate) fn clear_for_test() {
    if let Ok(mut map) = registry().lock() { map.clear(); }
}

#[cfg(test)]
#[path = "virtual_sim_tests.rs"]
mod tests;
