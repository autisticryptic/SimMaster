//! Stable line descriptors and identity helpers shared by modem providers.
//!
//! No D-Bus connection or concrete backend is required to represent a line or
//! a standalone SIM reader. Serialized field names retain the existing API
//! contract while callers migrate away from the legacy MM module.

/// Stable description of one physical modem and its currently selected SIM.
///
/// `line_id` is anchored to physical hardware plus UIM slot, not a backend
/// object number or the currently inserted SIM. SIM-specific overrides retain
/// their separate SIM binding key; legacy IDs remain migration aliases only.
#[derive(Debug, Clone, Default, serde::Serialize, PartialEq, Eq)]
pub struct ModemBinding {
    pub line_id: String,
    /// Persistent one-based position assigned to the physical modem.
    pub display_order: u32,
    /// Non-sensitive display name for the physical slot (for example 基带 1).
    pub slot_label: String,
    /// How the physical slot was anchored (physdev, udev_path, equipment, ...).
    pub slot_source: String,
    /// Whether the anchor follows a physical board/port location.
    pub slot_stable: bool,
    /// Multiple discovered modem objects resolved to the same physical slot.
    pub slot_conflict: bool,
    pub modem_id: String,
    /// Legacy serialized control selector. Only the selected provider may
    /// interpret it as a backend path; native observations need not invent MM IDs.
    pub modem_path: String,
    pub manufacturer: String,
    pub model: String,
    /// Stable hardware family classification used for capability diagnostics.
    #[serde(default)]
    pub device_family: String,
    /// Control path selected for this line (for example ModemManager QMI+AT).
    #[serde(default)]
    pub control_transport: String,
    pub primary_port: String,
    pub qmi_device: Option<String>,
    pub uim_slot: u8,
    pub sim_path: Option<String>,
    pub sim_iccid: String,
    pub operator_id: String,
    pub state: String,
    pub present: bool,
    /// ModemManager `SimType`: "physical", "esim", or "unknown". Used by the
    /// per-line eSIM auto-detection so plain SIM lines never issue lpac calls.
    #[serde(default)]
    pub sim_type: String,
    /// ModemManager `EsimStatus`: "none", "no-profiles", "with-profiles", or
    /// "unknown". A eUICC chip is present when this is not "none"/"unknown".
    #[serde(default)]
    pub esim_status: String,
    /// What kind of line this is: "baseband" for a real ModemManager modem that
    /// can register on a cell and run VoLTE, or "reader" for a standalone SIM
    /// reader that only backs user-space VoWiFi + eSIM management. Empty is
    /// treated as "baseband" for backwards compatibility.
    #[serde(default)]
    pub line_kind: String,
    /// Raw physical slot selector. It is required for rebinding but must not
    /// be returned by the HTTP inventory endpoint.
    #[serde(skip)]
    pub hardware_key: String,
    /// IMEI/MEID of the module occupying the physical slot.
    #[serde(skip)]
    pub equipment_identifier: String,
    /// Selectors used by older releases, retained for config migration only.
    #[serde(skip)]
    pub legacy_hardware_keys: Vec<String>,
    /// Previous line IDs that may own the current SIM's saved profile.
    #[serde(skip)]
    pub legacy_line_ids: Vec<String>,
}

pub(super) fn stable_line_id(hardware_key: &str, sim_key: &str) -> String {
    let material = format!("{}\0{}", hardware_key.trim(), sim_key.trim());
    let digest = md5::compute(material.as_bytes());
    format!("line-{digest:x}")
}

pub(super) fn line_hardware_key(hardware_key: &str, uim_slot: u8) -> String {
    if uim_slot <= 1 {
        hardware_key.to_string()
    } else {
        format!("{hardware_key}#uim{uim_slot}")
    }
}

pub(super) fn physical_line_id(hardware_key: &str, uim_slot: u8) -> String {
    stable_line_id("physical-line", &line_hardware_key(hardware_key, uim_slot))
}

pub(super) fn physical_line_identity(
    hardware_key: &str,
    uim_slot: u8,
    sim_key: &str,
    legacy_hardware_keys: &[String],
) -> (String, Vec<String>) {
    let line_key = line_hardware_key(hardware_key, uim_slot);
    let line_id = physical_line_id(hardware_key, uim_slot);
    let legacy_line_ids = unique_non_empty(
        std::iter::once(stable_line_id(&line_key, sim_key)).chain(
            legacy_hardware_keys
                .iter()
                .map(|key| stable_line_id(&line_hardware_key(key, uim_slot), sim_key)),
        ),
    )
    .into_iter()
    .filter(|legacy_line_id| legacy_line_id != &line_id)
    .collect();
    (line_id, legacy_line_ids)
}

/// Build the stable line identity for a SIM reader. The reader selector is the
/// physical line anchor, so inserting another SIM does not replace the user's
/// VoWiFi, trunk, notification, or automation configuration.
pub fn reader_line_id(reader_id: &str, uim_slot: u8) -> String {
    stable_line_id(
        &format!("reader:{}", reader_id.trim()),
        &format!("uim:{uim_slot}"),
    )
}

/// Synthesize a `ModemBinding` for a standalone SIM reader line. Readers only
/// participate in VoWiFi and eSIM management (no cellular baseband), so cellular
/// fields are left empty and `line_kind` is "reader". `present` reflects whether
/// a live reader path/QMI device has been resolved for the slot.
pub fn reader_binding(
    reader_id: &str,
    label: &str,
    reader_path: &str,
    uim_slot: u8,
    qmi_device: Option<String>,
    present: bool,
    sim_iccid: String,
    operator_id: String,
    legacy_line_ids: Vec<String>,
) -> ModemBinding {
    ModemBinding {
        line_id: reader_line_id(reader_id, uim_slot),
        display_order: 0,
        slot_label: label.trim().to_string(),
        slot_source: "reader".to_string(),
        slot_stable: true,
        slot_conflict: false,
        modem_id: format!("reader:{}", reader_id.trim()),
        modem_path: String::new(),
        manufacturer: "SIM 读卡器".to_string(),
        model: reader_path.trim().to_string(),
        device_family: "usb_sim_reader".to_string(),
        control_transport: if reader_path.trim().starts_with("pcsc://") {
            "pcsc".to_string()
        } else {
            "qmi_uim".to_string()
        },
        primary_port: String::new(),
        qmi_device,
        uim_slot,
        sim_path: None,
        sim_iccid,
        operator_id,
        state: if present {
            "registered".to_string()
        } else {
            String::new()
        },
        present,
        sim_type: "physical".to_string(),
        esim_status: "none".to_string(),
        hardware_key: format!("reader:{}", reader_id.trim()),
        equipment_identifier: String::new(),
        legacy_hardware_keys: Vec::new(),
        legacy_line_ids,
        line_kind: "reader".to_string(),
    }
}

pub(super) fn unique_non_empty(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut result = Vec::new();
    for value in values {
        let value = value.trim().to_string();
        if !value.is_empty() && !result.contains(&value) {
            result.push(value);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_values_remain_compatible_with_existing_persistent_rows() {
        let (line_id, aliases) = physical_line_identity("sysfs:slot-1", 1, "iccid:456", &[]);
        assert_eq!(line_id, "line-fa9b70acd51cf9273a726ab948073c92");
        assert_eq!(aliases, ["line-04f94521498f4d2790805df0bb7bb29d"]);
        assert_eq!(
            reader_line_id(" Reader A ", 1),
            "line-97e0ac0294745e5f87ff08cbee7bc7cb"
        );
    }

    #[test]
    fn a_reader_binding_does_not_require_a_modem_backend_path() {
        let reader = reader_binding(
            "Reader A",
            "SIM reader",
            "pcsc://reader-a",
            1,
            None,
            true,
            "test-sim-a".to_string(),
            "00101".to_string(),
            Vec::new(),
        );
        assert_eq!(reader.line_kind, "reader");
        assert_eq!(reader.control_transport, "pcsc");
        assert!(reader.modem_path.is_empty());
        assert!(reader.qmi_device.is_none());
        assert_eq!(reader.line_id, reader_line_id("Reader A", 1));
    }

    #[test]
    fn serialization_preserves_legacy_fields_but_excludes_private_anchors() {
        let binding = ModemBinding {
            line_id: "line-test".to_string(),
            modem_path: "opaque-test-selector".to_string(),
            hardware_key: "private-physical-anchor".to_string(),
            equipment_identifier: "private-equipment-id".to_string(),
            legacy_hardware_keys: vec!["private-old-anchor".to_string()],
            legacy_line_ids: vec!["private-old-line".to_string()],
            ..Default::default()
        };
        let value = serde_json::to_value(binding).expect("serialize binding");
        assert_eq!(value["line_id"], "line-test");
        assert_eq!(value["modem_path"], "opaque-test-selector");
        for private in [
            "hardware_key",
            "equipment_identifier",
            "legacy_hardware_keys",
            "legacy_line_ids",
        ] {
            assert!(value.get(private).is_none(), "{private}");
        }
    }

    #[test]
    fn stable_line_identity_survives_modemmanager_renumbering() {
        assert_eq!(
            stable_line_id("imei:123", "iccid:456"),
            stable_line_id("imei:123", "iccid:456")
        );
    }

    #[test]
    fn physical_line_identity_survives_sim_changes() {
        let (first, first_aliases) = physical_line_identity("sysfs:slot-1", 1, "iccid:456", &[]);
        let (second, second_aliases) = physical_line_identity("sysfs:slot-1", 1, "iccid:789", &[]);
        assert_eq!(
            first, second,
            "the current SIM must not participate in the physical line ID"
        );
        assert_eq!(
            first_aliases,
            vec![stable_line_id("sysfs:slot-1", "iccid:456")]
        );
        assert_eq!(
            second_aliases,
            vec![stable_line_id("sysfs:slot-1", "iccid:789")]
        );
    }

    #[test]
    fn non_primary_uim_slot_gets_distinct_line_identity() {
        let hardware_key = "imei:123";
        assert_eq!(line_hardware_key(hardware_key, 1), hardware_key);
        assert_ne!(
            physical_line_id(hardware_key, 1),
            physical_line_id(hardware_key, 2)
        );
    }
}
