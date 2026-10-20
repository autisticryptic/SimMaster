//! 48-bit SQN handling for the software USIM (TS 33.102 / TS 35.206).
//!
//! The card-side rule used here: the network's SQN must be strictly greater
//! than the last accepted one; otherwise the UE resynchronises with AUTS. The
//! value is persisted by the caller so a restart cannot replay an old SQN.
pub const SQN_LEN: usize = 6;
const MASK48: u64 = 0x0000_ffff_ffff_ffff;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Sqn(u64);

impl Sqn {
    pub fn from_value(value: u64) -> Self {
        Self(value & MASK48)
    }

    pub fn from_bytes(bytes: &[u8; SQN_LEN]) -> Self {
        let mut value = 0u64;
        for byte in bytes {
            value = (value << 8) | u64::from(*byte);
        }
        Self(value)
    }

    pub fn to_bytes(self) -> [u8; SQN_LEN] {
        let mut out = [0u8; SQN_LEN];
        for (index, byte) in out.iter_mut().enumerate() {
            *byte = ((self.0 >> (8 * (SQN_LEN - 1 - index))) & 0xff) as u8;
        }
        out
    }

    pub fn value(self) -> u64 {
        self.0
    }

    /// Freshness check: only a strictly newer network SQN is acceptable.
    pub fn accepts(self, network: Self) -> bool {
        network.0 > self.0
    }

    /// Record the accepted network SQN (never moves backwards).
    pub fn advance(&mut self, network: Self) {
        if network.0 > self.0 {
            self.0 = network.0;
        }
    }

    /// SQN_MS used in AUTS: one past the last accepted value, wrapping inside
    /// the 48-bit space.
    pub fn next(self) -> Self {
        Self(self.0.wrapping_add(1) & MASK48)
    }
}

#[cfg(test)]
#[path = "sqn_tests.rs"]
mod tests;
