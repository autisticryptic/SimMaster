//! ModemManager implementation of the explicit radio-control seam.
//!
//! MM's Enable/State contract is retained, including its existing command
//! serialization and bounded transition waits. No constructor side effects,
//! new connection, automatic registration or bearer activation are added.

use std::sync::Arc;

use zbus::Connection;

use crate::hardware::devices::transport::TransportFuture;

use super::{
    bindings::ModemBinding,
    modem_manager,
    radio::{ModemRadioControl, RadioError, RadioState},
};

pub struct ModemManagerRadio {
    connection: Arc<Connection>,
}

impl ModemManagerRadio {
    pub fn new(connection: Arc<Connection>) -> Self {
        Self { connection }
    }
}

fn modem_selector(binding: &ModemBinding) -> Result<&str, RadioError> {
    if !binding.line_kind.is_empty() && binding.line_kind != "baseband" {
        return Err(RadioError::Unsupported("line_has_no_baseband".into()));
    }
    if !binding.present {
        return Err(RadioError::Absent);
    }
    if binding.modem_path.trim().is_empty() {
        return Err(RadioError::Unavailable("radio_mm_selector_missing".into()));
    }
    Ok(&binding.modem_path)
}

fn radio_state_from_mm(state: i32) -> RadioState {
    match state {
        3 => RadioState::Off,
        4 => RadioState::TurningOff,
        5 => RadioState::TurningOn,
        6..=11 => RadioState::On,
        // Failed, unknown, initializing, locked, or an unrecognized future
        // value cannot establish that RF is on OR off.
        _ => RadioState::Unknown,
    }
}

impl ModemRadioControl for ModemManagerRadio {
    fn observe<'a>(
        &'a self,
        binding: &'a ModemBinding,
    ) -> TransportFuture<'a, Result<RadioState, RadioError>> {
        Box::pin(async move {
            let modem_path = modem_selector(binding)?;
            modem_manager::get_modem_state_for_modem(self.connection.as_ref(), modem_path)
                .await
                .map(radio_state_from_mm)
                .map_err(|error| RadioError::Unavailable(error.to_string()))
        })
    }

    fn set_airplane_mode<'a>(
        &'a self,
        binding: &'a ModemBinding,
        enabled: bool,
    ) -> TransportFuture<'a, Result<(), RadioError>> {
        Box::pin(async move {
            let modem_path = modem_selector(binding)?;
            modem_manager::set_airplane_mode_for_modem(
                self.connection.as_ref(),
                modem_path,
                enabled,
            )
            .await
            .map_err(RadioError::Failed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mm_states_distinguish_stable_transition_and_unknown() {
        assert_eq!(radio_state_from_mm(3), RadioState::Off);
        assert_eq!(radio_state_from_mm(4), RadioState::TurningOff);
        assert_eq!(radio_state_from_mm(5), RadioState::TurningOn);
        for state in 6..=11 {
            assert_eq!(radio_state_from_mm(state), RadioState::On);
        }
        for state in [-10, -1, 0, 1, 2, 12, i32::MAX] {
            assert_eq!(radio_state_from_mm(state), RadioState::Unknown);
        }
    }

    #[test]
    fn target_validation_never_falls_back_to_another_modem() {
        let mut binding = ModemBinding::default();
        assert_eq!(modem_selector(&binding), Err(RadioError::Absent));
        binding.present = true;
        assert!(matches!(
            modem_selector(&binding),
            Err(RadioError::Unavailable(_))
        ));
        binding.modem_path = "/org/freedesktop/ModemManager1/Modem/7".into();
        assert_eq!(modem_selector(&binding), Ok(binding.modem_path.as_str()));
        binding.line_kind = "reader".into();
        assert!(matches!(
            modem_selector(&binding),
            Err(RadioError::Unsupported(_))
        ));
    }
}
