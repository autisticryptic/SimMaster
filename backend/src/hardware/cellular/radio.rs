//! Backend-neutral radio observation and explicit control.
//!
//! Obtaining a provider or observing a modem must not enable it, register it,
//! or connect a bearer. An implementation reports what its backend can prove;
//! this is not a guarantee of zero RF activity since the device was powered on.

use std::{error::Error, fmt};

use crate::hardware::devices::transport::TransportFuture;

use super::bindings::ModemBinding;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RadioState {
    On,
    Off,
    TurningOn,
    TurningOff,
    #[default]
    Unknown,
}

impl RadioState {
    /// Only a stable observation can confirm an airplane-mode state.
    pub fn airplane_enabled(self) -> Option<bool> {
        match self {
            Self::Off => Some(true),
            Self::On => Some(false),
            Self::TurningOn | Self::TurningOff | Self::Unknown => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RadioError {
    Absent,
    Unsupported(String),
    Unavailable(String),
    Failed(String),
}

impl RadioError {
    pub fn reason(&self) -> &str {
        match self {
            Self::Absent => "radio_device_absent",
            Self::Unsupported(reason) | Self::Unavailable(reason) | Self::Failed(reason) => reason,
        }
    }
}

impl fmt::Display for RadioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.reason())
    }
}

impl Error for RadioError {}

/// Injected alongside discovery, using the same selected backend and connection.
/// The caller coordinates line intents/bearers; the backend serializes its
/// physical control commands. This seam alone is not a multi-SIM device owner.
pub trait ModemRadioControl: Send + Sync {
    fn observe<'a>(
        &'a self,
        binding: &'a ModemBinding,
    ) -> TransportFuture<'a, Result<RadioState, RadioError>>;

    fn set_airplane_mode<'a>(
        &'a self,
        binding: &'a ModemBinding,
        enabled: bool,
    ) -> TransportFuture<'a, Result<(), RadioError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_stable_radio_observations_confirm_airplane_mode() {
        assert_eq!(RadioState::Off.airplane_enabled(), Some(true));
        assert_eq!(RadioState::On.airplane_enabled(), Some(false));
        for state in [
            RadioState::TurningOn,
            RadioState::TurningOff,
            RadioState::Unknown,
        ] {
            assert_eq!(state.airplane_enabled(), None);
        }
    }

    struct NativeFixture;

    impl ModemRadioControl for NativeFixture {
        fn observe<'a>(
            &'a self,
            _binding: &'a ModemBinding,
        ) -> TransportFuture<'a, Result<RadioState, RadioError>> {
            Box::pin(async { Ok(RadioState::Off) })
        }

        fn set_airplane_mode<'a>(
            &'a self,
            _binding: &'a ModemBinding,
            _enabled: bool,
        ) -> TransportFuture<'a, Result<(), RadioError>> {
            Box::pin(async { Err(RadioError::Unsupported("fixture_read_only".into())) })
        }
    }

    #[tokio::test]
    async fn radio_provider_needs_neither_dbus_nor_a_modemmanager_selector() {
        let provider: Box<dyn ModemRadioControl> = Box::new(NativeFixture);
        let binding = ModemBinding {
            present: true,
            line_id: "line-native-fixture".into(),
            ..Default::default()
        };
        assert!(binding.modem_path.is_empty());
        assert_eq!(provider.observe(&binding).await, Ok(RadioState::Off));
        assert!(matches!(
            provider.set_airplane_mode(&binding, false).await,
            Err(RadioError::Unsupported(_))
        ));
    }
}
