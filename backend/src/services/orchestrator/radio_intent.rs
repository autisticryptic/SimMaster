//! Pure decisions for saved radio/data intents, not a boot-time RF guarantee.
//!
//! Callers must sample the binding and saved profile under their operation
//! lock, then keep that lock through the selected action. Discovery, RF,
//! registration, ordinary data and IMS are deliberately separate concerns.

use crate::{
    hardware::cellular::radio::{RadioError, RadioState},
    platform::config::LineProfileConfig,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeRestorePlan {
    Offline,
    NoCellularRadio,
    ApplyAirplane,
    StopData,
    StartData,
    LeaveUnchanged,
}

pub fn runtime_restore_plan(
    present: bool,
    has_radio: bool,
    profile: &LineProfileConfig,
) -> RuntimeRestorePlan {
    if !has_radio {
        RuntimeRestorePlan::NoCellularRadio
    } else if !present {
        RuntimeRestorePlan::Offline
    } else if profile.airplane_mode_enabled {
        // Disabling a line must not erase an explicit "keep RF off" intent.
        RuntimeRestorePlan::ApplyAirplane
    } else if !profile.enabled {
        RuntimeRestorePlan::StopData
    } else if profile.data_connection_enabled {
        RuntimeRestorePlan::StartData
    } else {
        // A false airplane switch is not a command to auto-enable at boot.
        // Existing CS registration policy is independent of the data switch.
        RuntimeRestorePlan::LeaveUnchanged
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataStartPurpose {
    SavedIntent,
    TemporaryAutomation,
}

pub fn data_start_admission(
    present: bool,
    has_radio: bool,
    current: &LineProfileConfig,
    purpose: DataStartPurpose,
) -> Result<(), &'static str> {
    if !has_radio {
        Err("line_has_no_baseband")
    } else if !present {
        Err("line_not_present")
    } else if !current.enabled {
        Err("line_disabled")
    } else if current.airplane_mode_enabled {
        Err("line_airplane_mode_enabled")
    } else if purpose == DataStartPurpose::SavedIntent && !current.data_connection_enabled {
        Err("line_data_not_requested")
    } else {
        Ok(())
    }
}

/// The native-call fallback must fail closed. An unknown/transitional query
/// cannot turn failed IMS routing into a potentially billable cellular call.
pub fn native_call_radio_admission(
    airplane_requested: bool,
    observation: Result<RadioState, RadioError>,
) -> Result<(), &'static str> {
    if airplane_requested {
        return Err("cs_blocked_by_airplane_mode");
    }
    match observation {
        Ok(RadioState::On) => Ok(()),
        Ok(RadioState::Off | RadioState::TurningOff) => Err("cs_blocked_by_airplane_mode"),
        _ => Err("airplane_mode_state_unavailable"),
    }
}

pub struct AirplaneIntentView {
    pub radio_state: RadioState,
    pub observed: Option<bool>,
    pub error: Option<String>,
    pub phase: &'static str,
    pub stage: &'static str,
    /// Compatibility projection only, never proof of the observed RF state.
    pub legacy_enabled: bool,
    pub legacy_powered: bool,
    pub legacy_online: bool,
}

pub fn airplane_intent_view(
    requested: bool,
    observation: Result<RadioState, RadioError>,
) -> AirplaneIntentView {
    let (state, error, phase, stage) = match observation {
        Err(RadioError::Absent) => (
            RadioState::Unknown,
            Some(RadioError::Absent.to_string()),
            "offline",
            "设备离线，射频状态未知；配置将在设备恢复后应用",
        ),
        Err(RadioError::Unsupported(reason)) => (
            RadioState::Unknown,
            Some(reason),
            "unsupported",
            "此线路不支持蜂窝射频控制",
        ),
        Err(error) => (
            RadioState::Unknown,
            Some(error.to_string()),
            "unknown",
            "无法读取射频状态，尚未确认飞行模式",
        ),
        Ok(RadioState::Unknown) => (
            RadioState::Unknown,
            Some("radio_state_unknown".into()),
            "unknown",
            "射频状态未知，尚未确认飞行模式",
        ),
        Ok(state) => {
            let (phase, stage) = match (state, requested) {
                (RadioState::Off, true) => ("enabled", "移动射频已关闭"),
                (RadioState::On, false) => {
                    ("disabled", "移动射频已开启（不代表已驻网或数据已连接）")
                }
                (RadioState::Off, false) => ("disabling", "期望开启移动射频，当前仍关闭"),
                (RadioState::On, true) => ("enabling", "期望关闭移动射频，当前仍开启"),
                (RadioState::TurningOff, _) => ("enabling", "移动射频正在关闭，等待确认"),
                (RadioState::TurningOn, _) => ("disabling", "移动射频正在开启，等待确认"),
                (RadioState::Unknown, _) => unreachable!(),
            };
            (state, None, phase, stage)
        }
    };
    AirplaneIntentView {
        radio_state: state,
        observed: state.airplane_enabled(),
        error,
        phase,
        stage,
        // Retain old fields for older API clients. New consumers use requested,
        // observed and phase separately instead of inferring success from this.
        legacy_enabled: requested || matches!(state, RadioState::Off | RadioState::TurningOff),
        legacy_powered: state != RadioState::Unknown,
        legacy_online: state == RadioState::On,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> LineProfileConfig {
        LineProfileConfig::for_line("line-radio-intent-test")
    }

    #[test]
    fn saved_airplane_takes_precedence_even_on_a_disabled_line() {
        let mut current = profile();
        for enabled in [false, true] {
            for data in [false, true] {
                current.enabled = enabled;
                current.data_connection_enabled = data;
                current.airplane_mode_enabled = true;
                assert_eq!(
                    runtime_restore_plan(true, true, &current),
                    RuntimeRestorePlan::ApplyAirplane
                );
            }
        }
    }

    #[test]
    fn restore_does_not_imply_auto_enable_or_touch_absent_devices_and_readers() {
        let mut current = profile();
        current.enabled = true;
        current.data_connection_enabled = false;
        current.airplane_mode_enabled = false;
        assert_eq!(
            runtime_restore_plan(true, true, &current),
            RuntimeRestorePlan::LeaveUnchanged
        );
        current.data_connection_enabled = true;
        assert_eq!(
            runtime_restore_plan(true, true, &current),
            RuntimeRestorePlan::StartData
        );
        current.enabled = false;
        assert_eq!(
            runtime_restore_plan(true, true, &current),
            RuntimeRestorePlan::StopData
        );
        current.airplane_mode_enabled = true;
        assert_eq!(
            runtime_restore_plan(false, true, &current),
            RuntimeRestorePlan::Offline
        );
        assert_eq!(
            runtime_restore_plan(true, false, &current),
            RuntimeRestorePlan::NoCellularRadio
        );
    }

    #[test]
    fn temporary_data_is_explicit_and_never_bypasses_airplane_or_disabled_line() {
        let mut current = profile();
        current.enabled = true;
        current.data_connection_enabled = false;
        current.airplane_mode_enabled = false;
        assert_eq!(
            data_start_admission(true, true, &current, DataStartPurpose::SavedIntent),
            Err("line_data_not_requested")
        );
        assert_eq!(
            data_start_admission(true, true, &current, DataStartPurpose::TemporaryAutomation),
            Ok(())
        );
        for purpose in [
            DataStartPurpose::SavedIntent,
            DataStartPurpose::TemporaryAutomation,
        ] {
            current.data_connection_enabled = true;
            current.airplane_mode_enabled = true;
            assert_eq!(
                data_start_admission(true, true, &current, purpose),
                Err("line_airplane_mode_enabled")
            );
            current.airplane_mode_enabled = false;
            current.enabled = false;
            assert_eq!(
                data_start_admission(true, true, &current, purpose),
                Err("line_disabled")
            );
            current.enabled = true;
            assert_eq!(
                data_start_admission(false, true, &current, purpose),
                Err("line_not_present")
            );
            assert_eq!(
                data_start_admission(true, false, &current, purpose),
                Err("line_has_no_baseband")
            );
        }
    }

    #[test]
    fn unknown_unavailable_and_transitions_never_confirm_normal_rf() {
        for requested in [false, true] {
            for observation in [
                Ok(RadioState::Unknown),
                Err(RadioError::Unavailable("query failed".into())),
                Err(RadioError::Failed("command failed".into())),
            ] {
                let view = airplane_intent_view(requested, observation);
                assert_eq!(view.observed, None);
                assert_eq!(view.phase, "unknown");
                assert!(view.error.is_some());
                assert!(!view.legacy_online);
            }
            for state in [RadioState::TurningOn, RadioState::TurningOff] {
                let view = airplane_intent_view(requested, Ok(state));
                assert_eq!(view.observed, None);
                assert!(!matches!(view.phase, "enabled" | "disabled"));
            }
        }
    }

    #[test]
    fn requested_and_observed_are_independent_including_mismatches() {
        for requested in [false, true] {
            for (state, observed) in [(RadioState::On, false), (RadioState::Off, true)] {
                let view = airplane_intent_view(requested, Ok(state));
                assert_eq!(view.observed, Some(observed));
                if requested == observed {
                    assert_eq!(view.phase, if requested { "enabled" } else { "disabled" });
                } else {
                    assert_eq!(view.phase, if requested { "enabling" } else { "disabling" });
                }
            }
        }
        assert_eq!(
            airplane_intent_view(true, Err(RadioError::Absent)).phase,
            "offline"
        );
        assert_eq!(
            airplane_intent_view(false, Err(RadioError::Unsupported("reader".into()))).phase,
            "unsupported"
        );
    }

    #[test]
    fn native_voice_fallback_requires_confirmed_rf_and_no_flight_intent() {
        assert_eq!(
            native_call_radio_admission(false, Ok(RadioState::On)),
            Ok(())
        );
        assert_eq!(
            native_call_radio_admission(true, Ok(RadioState::On)),
            Err("cs_blocked_by_airplane_mode")
        );
        for observation in [
            Ok(RadioState::Off),
            Ok(RadioState::TurningOff),
            Ok(RadioState::TurningOn),
            Ok(RadioState::Unknown),
            Err(RadioError::Unavailable("no query result".into())),
            Err(RadioError::Absent),
            Err(RadioError::Unsupported("reader".into())),
        ] {
            assert!(native_call_radio_admission(false, observation.clone()).is_err());
            assert!(native_call_radio_admission(true, observation).is_err());
        }
    }
}
