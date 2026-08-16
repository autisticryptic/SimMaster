//! Quectel modem-family classification.
//!
//! Generic Quectel control remains owned by ModemManager. This module only
//! turns the manufacturer/model strings already reported by ModemManager into
//! stable capability metadata for diagnostics and UI decisions.

mod ec2x;
mod eg600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuectelFamily {
    Ec20,
    Ec25,
    Eg25,
    Eg600,
    Compatible,
}

impl QuectelFamily {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ec20 => "quectel_ec20",
            Self::Ec25 => "quectel_ec25",
            Self::Eg25 => "quectel_eg25_family",
            Self::Eg600 => "quectel_eg600_family",
            Self::Compatible => "quectel_compatible",
        }
    }
}

pub fn classify(manufacturer: &str, model: &str) -> Option<QuectelFamily> {
    let manufacturer = manufacturer.trim().to_ascii_uppercase();
    let model = model.trim().to_ascii_uppercase();
    let quectel = manufacturer.contains("QUECTEL")
        || ec2x::classify(&model).is_some()
        || eg600::matches(&model);
    if !quectel {
        return None;
    }

    ec2x::classify(&model)
        .or_else(|| eg600::matches(&model).then_some(QuectelFamily::Eg600))
        .or(Some(QuectelFamily::Compatible))
}

pub fn control_transport(primary_port: &str, qmi_device: Option<&str>) -> &'static str {
    if qmi_device.is_some_and(|device| !device.trim().is_empty()) {
        "modemmanager_qmi_at"
    } else if !primary_port.trim().is_empty() {
        "modemmanager_at"
    } else {
        "modemmanager"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_requested_quectel_families() {
        assert_eq!(classify("Quectel", "EC20F"), Some(QuectelFamily::Ec20));
        assert_eq!(classify("Quectel", "EC25-E"), Some(QuectelFamily::Ec25));
        assert_eq!(classify("Quectel", "EG25-G"), Some(QuectelFamily::Eg25));
        assert_eq!(classify("Quectel", "EG600U-EA"), Some(QuectelFamily::Eg600));
    }

    #[test]
    fn unknown_quectel_models_stay_on_the_generic_compatible_path() {
        assert_eq!(
            classify("QUECTEL INCORPORATED", "RM520N-GL"),
            Some(QuectelFamily::Compatible)
        );
        assert_eq!(classify("QUALCOMM INCORPORATED", "0"), None);
    }

    #[test]
    fn qmi_and_at_transport_metadata_is_deterministic() {
        assert_eq!(
            control_transport("ttyUSB2", Some("/dev/cdc-wdm0")),
            "modemmanager_qmi_at"
        );
        assert_eq!(control_transport("ttyUSB2", None), "modemmanager_at");
        assert_eq!(control_transport("", None), "modemmanager");
    }
}
