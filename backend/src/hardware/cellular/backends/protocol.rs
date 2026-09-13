//! Pure native protocol codecs. Unknown is a real state, never "home"/RF-on.

use super::{config::NativeProtocol, NativeError};
use crate::hardware::cellular::radio::RadioState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Qmi,
    QmiControl,
    Mbim,
    At,
    AtUssd,
    AtSms,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRequest {
    pub tool: Tool,
    pub device: String,
    pub arguments: Vec<String>,
    pub timeout_seconds: u64,
}

impl CommandRequest {
    pub fn query(protocol: NativeProtocol, device: &str, action: &str) -> Self {
        let (tool, arguments) = match protocol {
            NativeProtocol::Qmi => (
                Tool::Qmi,
                vec![
                    "-d".into(),
                    device.into(),
                    "--device-open-proxy".into(),
                    action.into(),
                ],
            ),
            NativeProtocol::Mbim => (
                Tool::Mbim,
                vec![
                    "-d".into(),
                    device.into(),
                    "--device-open-proxy".into(),
                    action.into(),
                ],
            ),
            NativeProtocol::At => (Tool::At, vec![action.into()]),
        };
        Self {
            tool,
            device: device.into(),
            arguments,
            timeout_seconds: 20,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RegistrationState {
    Home,
    Roaming,
    Searching,
    Denied,
    Idle,
    #[default]
    Unknown,
}

impl RegistrationState {
    pub fn registered(self) -> bool {
        matches!(self, Self::Home | Self::Roaming)
    }

    pub fn roaming(self) -> Result<bool, NativeError> {
        match self {
            Self::Home => Ok(false),
            Self::Roaming => Ok(true),
            _ => Err(NativeError::Unavailable(
                "native_roaming_state_unknown".into(),
            )),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Home => "registered",
            Self::Roaming => "roaming",
            Self::Searching => "searching",
            Self::Denied => "denied",
            Self::Idle => "unregistered",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct NetworkSnapshot {
    pub registration: RegistrationState,
    pub plmn: Option<String>,
    pub operator: String,
    pub technology: String,
    pub tac: Option<u32>,
    pub cell_id: Option<u64>,
    pub signal_percent: Option<u8>,
}

pub fn labelled<'a>(text: &'a str, label: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let (key, value) = line.trim().trim_start_matches('[').split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(label)
            .then(|| value.trim().trim_matches(['\'', '"']))
    })
}

pub fn valid_plmn(value: &str) -> bool {
    matches!(value.len(), 5 | 6) && value.bytes().all(|c| c.is_ascii_digit())
}

pub fn csv(value: &str) -> Result<Vec<String>, NativeError> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in value.chars() {
        match ch {
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut current).trim().to_string()),
            '\r' | '\n' | '\0' => return Err(NativeError::Protocol("native_csv_invalid".into())),
            _ => current.push(ch),
        }
    }
    if quoted {
        return Err(NativeError::Protocol(
            "native_csv_unterminated_quote".into(),
        ));
    }
    fields.push(current.trim().to_string());
    Ok(fields)
}

pub fn at_payload<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix(prefix).map(str::trim))
}

/// Map actual RSSI observations to the same 0..100 scale as AT+CSQ.
/// Unknown sentinels/malformed output stay unknown, not a fabricated zero.
pub fn parse_signal_percent(protocol: NativeProtocol, text: &str) -> Option<u8> {
    match protocol {
        NativeProtocol::Qmi => {
            let value = labelled(text, "RSSI")
                .filter(|s| s.contains("dBm"))
                .or_else(|| {
                    // Older NAS implementations only support Get Signal Strength.
                    text.split_once("Current:")?.1.lines().find_map(|line| {
                        let (key, value) = line.trim().split_once(':')?;
                        (key.starts_with("Network ") && value.contains("dBm"))
                            .then(|| value.trim().trim_matches('\''))
                    })
                })?;
            let dbm = value.split_whitespace().next()?.parse::<i16>().ok()?;
            (-150..=-1)
                .contains(&dbm)
                .then(|| (((dbm + 113).clamp(0, 62) * 100) / 62) as u8)
        }
        NativeProtocol::Mbim | NativeProtocol::At => {
            let value = if protocol == NativeProtocol::At {
                at_payload(text, "+CSQ:")?.split(',').next()?.trim()
            } else {
                labelled(text, "RSSI")?
            };
            let csq = value.parse::<u16>().ok()?;
            (csq <= 31).then(|| ((csq * 100) / 31) as u8)
        }
    }
}

pub fn parse_radio(protocol: NativeProtocol, text: &str) -> RadioState {
    match protocol {
        NativeProtocol::Qmi => match labelled(text, "Mode") {
            Some("online") => RadioState::On,
            Some("low-power" | "offline" | "persistent-low-power") => RadioState::Off,
            _ => RadioState::Unknown,
        },
        NativeProtocol::Mbim => {
            let hardware = labelled(text, "Hardware radio state");
            let software = labelled(text, "Software radio state");
            match (hardware, software) {
                (Some("on"), Some("on")) => RadioState::On,
                (Some("off"), _) | (_, Some("off")) => RadioState::Off,
                _ => RadioState::Unknown,
            }
        }
        NativeProtocol::At => match at_payload(text, "+CFUN:").and_then(|s| s.split(',').next()) {
            Some("1") => RadioState::On,
            Some("0" | "4") => RadioState::Off,
            _ => RadioState::Unknown,
        },
    }
}

pub fn parse_registration(protocol: NativeProtocol, text: &str) -> NetworkSnapshot {
    let mut snapshot = NetworkSnapshot::default();
    match protocol {
        NativeProtocol::Qmi | NativeProtocol::Mbim => {
            let state = labelled(text, "Registration state").unwrap_or_default();
            snapshot.registration = match state {
                "home" => RegistrationState::Home,
                "roaming" | "partner" => RegistrationState::Roaming,
                "registered" => match labelled(text, "Roaming status") {
                    Some("off") => RegistrationState::Home,
                    Some("on") => RegistrationState::Roaming,
                    _ => RegistrationState::Unknown,
                },
                "searching" | "not-registered-searching" => RegistrationState::Searching,
                "denied" | "registration-denied" => RegistrationState::Denied,
                "idle" | "deregistered" | "not-registered" => RegistrationState::Idle,
                _ => RegistrationState::Unknown,
            };
            snapshot.plmn = labelled(text, "Provider ID")
                .filter(|s| valid_plmn(s))
                .map(str::to_string)
                .or_else(|| {
                    let mcc = labelled(text, "MCC")?;
                    let mnc = labelled(text, "MNC")?;
                    // Numeric QMI MNC output may omit a leading zero. Never
                    // invent the MNC length from a number; require 2/3 digits.
                    let plmn = format!("{mcc}{mnc}");
                    (mcc.len() == 3 && matches!(mnc.len(), 2 | 3) && valid_plmn(&plmn))
                        .then_some(plmn)
                });
            snapshot.operator = labelled(text, "Provider name")
                .or_else(|| labelled(text, "Description"))
                .unwrap_or_default()
                .into();
            snapshot.technology = labelled(text, "Current cellular class")
                .or_else(|| labelled(text, "Data class"))
                .unwrap_or_default()
                .to_ascii_lowercase();
            if text.lines().any(|l| l.trim().ends_with("'lte'")) {
                snapshot.technology = "lte".into();
            }
        }
        NativeProtocol::At => {
            if let Some(payload) =
                at_payload(text, "+CEREG:").or_else(|| at_payload(text, "+CGREG:"))
            {
                let Ok(fields) = csv(payload) else {
                    return snapshot;
                };
                // Query form is <n>,<stat>,...; unsolicited form is <stat>,...
                let offset = if payload
                    .split(',')
                    .nth(1)
                    .is_some_and(|s| !s.trim().starts_with('"'))
                    && fields.get(1).is_some_and(|v| v.parse::<u8>().is_ok())
                {
                    1
                } else {
                    0
                };
                snapshot.registration = match fields.get(offset).and_then(|s| s.parse::<u8>().ok())
                {
                    Some(1 | 9) => RegistrationState::Home,
                    Some(5 | 10) => RegistrationState::Roaming,
                    Some(2) => RegistrationState::Searching,
                    Some(3) => RegistrationState::Denied,
                    Some(0 | 4) => RegistrationState::Idle,
                    _ => RegistrationState::Unknown,
                };
                snapshot.tac = fields
                    .get(offset + 1)
                    .and_then(|s| u32::from_str_radix(s, 16).ok());
                snapshot.cell_id = fields
                    .get(offset + 2)
                    .and_then(|s| u64::from_str_radix(s, 16).ok());
                snapshot.technology = match fields.get(offset + 3).map(String::as_str) {
                    Some("7") => "lte",
                    Some("0" | "1" | "3") => "gsm",
                    Some("2" | "4" | "5" | "6") => "umts",
                    Some("10" | "11" | "12" | "13") => "nr",
                    _ => "unknown",
                }
                .into();
            }
            if let Some(fields) = at_payload(text, "+COPS:").and_then(|p| csv(p).ok()) {
                if fields.get(1).is_some_and(|s| s == "2") {
                    snapshot.plmn = fields.get(2).filter(|s| valid_plmn(s)).cloned();
                } else {
                    snapshot.operator = fields.get(2).cloned().unwrap_or_default();
                }
            }
        }
    }
    snapshot
}

pub fn single_line(command: &str) -> Result<(), NativeError> {
    if command.is_empty() || command.len() > 8192 || command.chars().any(char::is_control) {
        Err(NativeError::Protocol(
            "native_command_not_single_line".into(),
        ))
    } else {
        Ok(())
    }
}

pub fn safe_parameter(value: &str) -> Result<&str, NativeError> {
    if value
        .chars()
        .any(|c| c.is_control() || matches!(c, ',' | '\'' | '"' | '\\'))
    {
        Err(NativeError::Protocol(
            "native_parameter_requires_unsupported_escaping".into(),
        ))
    } else {
        Ok(value)
    }
}

pub fn phone_number(value: &str) -> Result<&str, NativeError> {
    let number = value.strip_prefix('+').unwrap_or(value);
    if number.is_empty()
        || number.len() > 32
        || !number
            .bytes()
            .all(|c| c.is_ascii_digit() || b"*#".contains(&c))
    {
        Err(NativeError::Protocol("native_phone_number_invalid".into()))
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_queries_decode_qmi_without_at_and_keep_unknown_sentinels_unknown() {
        assert_eq!(
            parse_signal_percent(
                NativeProtocol::Qmi,
                "LTE:\n RSSI: '-55 dBm'\n RSRQ: '-13 dB'\n RSRP: '-88 dBm'\n SNR: '5.6 dB'"
            ),
            Some(93)
        );
        assert_eq!(
            parse_signal_percent(
                NativeProtocol::Qmi,
                "Current:\n Network 'lte': '-55 dBm'\nRSSI:\n Network 'lte': '-55 dBm'"
            ),
            Some(93)
        );
        for text in ["", "RSSI: '127 dBm'", "RSSI: 'unknown'", "RSRP: '-88 dBm'"] {
            assert_eq!(parse_signal_percent(NativeProtocol::Qmi, text), None);
        }
        assert_eq!(
            parse_signal_percent(NativeProtocol::Mbim, "RSSI: '31'"),
            Some(100)
        );
        assert_eq!(
            parse_signal_percent(NativeProtocol::Mbim, "RSSI: '99'"),
            None
        );
        assert_eq!(
            parse_signal_percent(NativeProtocol::At, "+CSQ: 0,99\r\nOK"),
            Some(0)
        );
        assert_eq!(
            parse_signal_percent(NativeProtocol::At, "+CSQ: 99,99\r\nOK"),
            None
        );
    }

    #[test]
    fn unknown_network_and_radio_are_never_treated_as_home_or_online() {
        for protocol in [
            NativeProtocol::Qmi,
            NativeProtocol::Mbim,
            NativeProtocol::At,
        ] {
            assert_eq!(parse_radio(protocol, "").airplane_enabled(), None);
            assert!(parse_registration(protocol, "")
                .registration
                .roaming()
                .is_err());
        }
        assert!(
            parse_registration(NativeProtocol::Qmi, "Registration state: 'registered'")
                .registration
                .roaming()
                .is_err()
        );
    }

    #[test]
    fn qmi_mbim_and_at_observations_are_protocol_specific() {
        assert_eq!(
            parse_radio(NativeProtocol::Qmi, "Mode: 'low-power'"),
            RadioState::Off
        );
        assert_eq!(
            parse_radio(
                NativeProtocol::Mbim,
                "Hardware radio state: 'off'\nSoftware radio state: 'on'"
            ),
            RadioState::Off
        );
        assert_eq!(
            parse_radio(NativeProtocol::At, "+CFUN: 1\r\nOK"),
            RadioState::On
        );
        let n = parse_registration(
            NativeProtocol::At,
            "+CEREG: 2,5,\"01AC\",\"00001234\",7\r\n+COPS: 0,2,\"00101\",7",
        );
        assert_eq!(n.registration, RegistrationState::Roaming);
        assert_eq!(n.plmn.as_deref(), Some("00101"));
        assert_eq!(n.cell_id, Some(0x1234));
        assert_eq!(n.technology, "lte");
    }

    #[test]
    fn native_command_arguments_do_not_admit_extra_at_or_cli_operations() {
        assert!(phone_number("+123456").is_ok());
        assert!(phone_number("123;AT+CFUN=1").is_err());
        assert!(single_line("AT+CIMI\rATD123;").is_err());
        assert!(safe_parameter("apn,ip-type=6").is_err());
        assert!(csv("1,\"unfinished").is_err());
    }
}
