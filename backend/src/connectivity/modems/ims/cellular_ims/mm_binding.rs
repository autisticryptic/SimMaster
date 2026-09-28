//! MM-only admission after a physical SIM, eSIM profile or modem binding change.
//! Identity is private in-memory state; never log/serialize it. Unknown samples
//! pause new work but do not erase the last known SIM or repeatedly tear it down.

use crate::hardware::cellular::bindings::ModemBinding;

#[derive(Clone, PartialEq, Eq)]
struct Identity {
    modem: String,
    sim: String,
    iccid: String,
    port: String,
    slot: u8,
}

fn identity(binding: &ModemBinding) -> Option<Identity> {
    let sim = binding.sim_path.as_deref()?;
    let iccid = crate::platform::utils::normalize_iccid(&binding.sim_iccid);
    if !binding.present
        || binding.slot_conflict
        || sim == "/"
        || sim.is_empty()
        || iccid.is_empty()
        || binding.primary_port.is_empty()
    {
        return None;
    }
    Some(Identity {
        modem: binding.modem_path.clone(),
        sim: sim.to_string(),
        iccid,
        port: binding.primary_port.clone(),
        slot: binding.uim_slot,
    })
}

#[derive(Default)]
pub(super) struct Calibration {
    initialized: bool,
    enabled: bool,
    known: Option<Identity>,
    observed: Option<Identity>,
    stable: u8,
    revision: u64,
    pending: bool,
    maintenance: Option<u64>,
}

impl Calibration {
    /// Returns true only for a confirmed binding change, not an unknown sample.
    pub fn observe(&mut self, binding: &ModemBinding) -> bool {
        let enabled = binding
            .modem_path
            .starts_with("/org/freedesktop/ModemManager1/Modem/");
        if !enabled {
            // Native/reader lifecycles are intentionally unchanged.
            self.enabled = false;
            return false;
        }
        self.enabled = true;
        if self.maintenance.is_some() {
            return false;
        }
        let sample = identity(binding);
        if !self.initialized {
            self.initialized = true;
            self.known = sample.clone();
            self.observed = sample;
            self.stable = 1;
            return false;
        }
        self.stable = if sample.is_some() && sample == self.observed {
            self.stable.saturating_add(1).min(2)
        } else {
            1
        };
        self.observed = sample.clone();
        let Some(sample) = sample else {
            return false;
        };
        if self.known.as_ref() != Some(&sample) {
            self.known = Some(sample);
            self.revision = self.revision.wrapping_add(1);
            self.pending = true;
            return true;
        }
        false
    }

    pub fn expected_sim(&self) -> Option<(String, u8)> {
        if !self.enabled {
            return None;
        }
        self.known
            .as_ref()
            .map(|identity| (identity.iccid.clone(), identity.slot))
    }

    /// Unknown inventory reads pause admission, not an already verified live
    /// bearer. Confirmed changes and explicit maintenance still cancel it.
    pub fn can_publish(&self) -> bool {
        !self.enabled || (self.maintenance.is_none() && !self.pending)
    }

    pub fn ready(&self) -> bool {
        !self.enabled || (self.maintenance.is_none() && self.observed.is_some() && !self.pending)
    }

    pub fn ticket(&self) -> Option<u64> {
        (self.enabled
            && self.maintenance.is_none()
            && self.pending
            && self.observed.is_some()
            && self.stable >= 2)
            .then_some(self.revision)
    }

    pub fn switch_in_progress(&self) -> bool {
        self.maintenance.is_some()
    }

    pub fn begin_switch(&mut self) -> Option<u64> {
        if !self.enabled {
            return None;
        }
        self.revision = self.revision.wrapping_add(1);
        self.maintenance = Some(self.revision);
        self.pending = true;
        Some(self.revision)
    }

    pub fn end_switch(&mut self, ticket: u64) {
        if self.maintenance == Some(ticket) {
            self.maintenance = None;
            self.observed = None;
            self.stable = 0;
        }
    }

    pub fn finish(&mut self, ticket: u64) -> bool {
        if self.ticket() != Some(ticket) {
            return false;
        }
        self.pending = false;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(card: &str) -> ModemBinding {
        ModemBinding {
            modem_path: "/org/freedesktop/ModemManager1/Modem/0".into(),
            sim_path: Some("/org/freedesktop/ModemManager1/SIM/0".into()),
            sim_iccid: card.into(),
            primary_port: "wwan0qmi0".into(),
            present: true,
            uim_slot: 1,
            ..Default::default()
        }
    }

    #[test]
    fn same_slot_physical_or_esim_change_requires_stable_recalibration() {
        let mut state = Calibration::default();
        let a = binding("8900000000000000001");
        let b = binding("8900000000000000002");
        assert!(!state.observe(&a));
        assert!(state.ready());
        assert!(state.observe(&b));
        assert!(!state.ready());
        assert_eq!(state.ticket(), None);
        assert!(!state.observe(&b));
        let ticket = state.ticket().unwrap();
        assert!(state.finish(ticket));
        assert!(state.ready());
        assert!(!state.observe(&b));
        assert_eq!(state.ticket(), None);
    }

    #[test]
    fn unknown_observation_does_not_forget_card_or_repeat_teardown() {
        let mut state = Calibration::default();
        let a = binding("8900000000000000001");
        state.observe(&a);
        let mut unknown = a.clone();
        unknown.sim_iccid.clear();
        for _ in 0..5 {
            assert!(!state.observe(&unknown));
            assert!(!state.ready());
            assert_eq!(state.ticket(), None);
        }
        assert!(!state.observe(&a));
        assert!(state.ready());
        unknown.present = false;
        assert!(!state.observe(&unknown));
        assert!(!state.observe(&a));
        assert!(state.ready());
    }

    #[test]
    fn delayed_cleanup_cannot_acknowledge_a_later_sim_or_unknown_sample() {
        let mut state = Calibration::default();
        state.observe(&binding("8900000000000000001"));
        let b = binding("8900000000000000002");
        state.observe(&b);
        state.observe(&b);
        let old = state.ticket().unwrap();
        let c = binding("8900000000000000003");
        state.observe(&c);
        state.observe(&c);
        assert!(!state.finish(old));
        let current = state.ticket().unwrap();
        let mut unknown = c.clone();
        unknown.sim_iccid.clear();
        state.observe(&unknown);
        assert!(!state.finish(current));
        state.observe(&c);
        assert_eq!(state.ticket(), None);
        state.observe(&c);
        assert!(state.finish(current));
    }

    #[test]
    fn modem_or_sim_object_replacement_and_slot_conflict_fail_closed() {
        let mut state = Calibration::default();
        let mut a = binding("8900000000000000001");
        state.observe(&a);
        a.modem_path.push('1');
        assert!(state.observe(&a));
        state.observe(&a);
        assert!(state.finish(state.ticket().unwrap()));
        a.sim_path = Some("/org/freedesktop/ModemManager1/SIM/1".into());
        assert!(state.observe(&a));
        a.slot_conflict = true;
        state.observe(&a);
        assert_eq!(state.ticket(), None);
        assert!(!state.ready());
    }

    #[test]
    fn explicit_switch_blocks_old_samples_and_old_completion() {
        let mut state = Calibration::default();
        let a = binding("8900000000000000001");
        state.observe(&a);
        let old = state.begin_switch().unwrap();
        for _ in 0..3 {
            state.observe(&a);
            assert!(!state.ready());
            assert_eq!(state.ticket(), None);
        }
        let latest = state.begin_switch().unwrap();
        state.end_switch(old);
        assert!(!state.ready());
        state.end_switch(latest);
        state.observe(&a);
        assert_eq!(state.ticket(), None);
        state.observe(&a);
        assert!(state.finish(state.ticket().unwrap()));
        assert!(state.ready());
    }

    #[test]
    fn native_and_other_lines_are_not_calibrated() {
        let mut first = Calibration::default();
        let mut other = Calibration::default();
        first.observe(&binding("8900000000000000001"));
        other.observe(&binding("8900000000000000002"));
        first.observe(&binding("8900000000000000003"));
        assert!(other.ready());
        let mut native = binding("8900000000000000004");
        native.modem_path = "native:modem0".into();
        assert!(!first.observe(&native));
        assert!(first.ready());
        assert_eq!(first.ticket(), None);
    }
}
