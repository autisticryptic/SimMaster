//! Explicit EC2x/EG25 maintenance. No arbitrary AT input, implicit MBN choice,
//! background modem writes or automatic rollback. Native owns every exchange.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::hardware::cellular::backends::{
    native::NativeDevice,
    protocol::{at_payload, csv},
    NativeError,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MaintenanceAction {
    SetIms { mode: u8 },
    SetUsbNetwork { mode: u8 },
    SelectMbn { profile: String },
    Reboot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MbnProfile {
    pub index: u32,
    pub selected: bool,
    pub activated: bool,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostics {
    pub model: String,
    pub ims_mode: Option<u8>,
    pub usb_network_mode: Option<u8>,
    pub usb_composition: Option<Vec<String>>,
    pub mbn_auto_select: Option<bool>,
    pub mbn_profiles: Vec<MbnProfile>,
    pub unavailable: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct MaintenancePlan {
    pub line_id: String,
    pub action: MaintenanceAction,
    pub expected_revision: String,
    pub before: Diagnostics,
    pub reboot_required: bool,
    pub requires_confirmation: bool,
}

#[derive(Debug, Serialize)]
pub struct MaintenanceResult {
    pub status: &'static str,
    pub completed_steps: Vec<&'static str>,
    pub failed_step: Option<&'static str>,
    pub restart_required: bool,
    pub reconciliation_required: bool,
}

fn invalid(reason: &'static str) -> NativeError {
    NativeError::Protocol(reason.into())
}

pub fn validate_action(action: &MaintenanceAction) -> Result<(), NativeError> {
    match action {
        MaintenanceAction::SetIms { mode } if *mode <= 2 => Ok(()),
        // EC2x values only: QMI/RMNET=0, ECM=1. Other families need their own driver.
        MaintenanceAction::SetUsbNetwork { mode } if *mode <= 1 => Ok(()),
        MaintenanceAction::SelectMbn { profile }
            if !profile.is_empty()
                && profile.len() <= 128
                && profile
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-. /".contains(&b)) =>
        {
            Ok(())
        }
        MaintenanceAction::Reboot => Ok(()),
        _ => Err(invalid("quectel_maintenance_action_invalid")),
    }
}

fn qcfg(output: &str, name: &str) -> Option<Vec<String>> {
    let rows: Vec<_> = output
        .lines()
        .filter_map(|line| {
            let fields = csv(line.trim().strip_prefix("+QCFG:")?).ok()?;
            (fields.first()?.eq_ignore_ascii_case(name)).then_some(fields)
        })
        .collect();
    let [row] = rows.as_slice() else { return None };
    Some(row.iter().skip(1).cloned().collect())
}

fn qcfg_number(output: &str, name: &str, maximum: u8) -> Option<u8> {
    qcfg(output, name)?
        .first()?
        .parse::<u8>()
        .ok()
        .filter(|v| *v <= maximum)
}

fn parse_mbn(output: &str) -> Result<Vec<MbnProfile>, NativeError> {
    let mut profiles = Vec::new();
    for row in output
        .lines()
        .filter_map(|line| line.trim().strip_prefix("+QMBNCFG:"))
    {
        let fields = csv(row.trim())?;
        if fields.first().map(String::as_str) != Some("List") {
            continue;
        }
        if fields.len() < 5 || profiles.len() >= 64 {
            return Err(invalid("quectel_mbn_list_invalid"));
        }
        let index = fields[1]
            .parse()
            .map_err(|_| invalid("quectel_mbn_list_invalid"))?;
        if profiles
            .iter()
            .any(|p: &MbnProfile| p.index == index || p.name == fields[4])
        {
            return Err(invalid("quectel_mbn_list_ambiguous"));
        }
        if !matches!(fields[2].as_str(), "0" | "1") || !matches!(fields[3].as_str(), "0" | "1") {
            return Err(invalid("quectel_mbn_list_invalid"));
        }
        profiles.push(MbnProfile {
            index,
            selected: fields[2] == "1",
            activated: fields[3] == "1",
            name: fields[4].clone(),
        });
    }
    Ok(profiles)
}

fn auto_select(output: &str) -> Option<bool> {
    let rows: Vec<_> = output
        .lines()
        .filter_map(|line| {
            let fields = csv(line.trim().strip_prefix("+QMBNCFG:")?).ok()?;
            if fields.first()?.eq_ignore_ascii_case("AutoSel") && fields.len() == 2 {
                match fields[1].as_str() {
                    "0" => Some(false),
                    "1" => Some(true),
                    _ => None,
                }
            } else {
                None
            }
        })
        .collect();
    match rows.as_slice() {
        [value] => Some(*value),
        _ => None,
    }
}

async fn at(device: &NativeDevice, command: &str) -> Result<String, NativeError> {
    device.io.execute(&device.at_request(command)?).await
}

async fn diagnostics_owned(device: &NativeDevice) -> Result<Diagnostics, NativeError> {
    let model_reply = at(device, "AT+CGMM").await?;
    let model = model_reply
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && *line != "OK" && !line.starts_with("AT"))
        .ok_or_else(|| invalid("quectel_model_unconfirmed"))?
        .trim_start_matches("+CGMM:")
        .trim()
        .to_string();
    if !matches!(
        super::classify("", &model),
        Some(super::QuectelFamily::Ec20 | super::QuectelFamily::Ec25 | super::QuectelFamily::Eg25)
    ) {
        return Err(NativeError::Unsupported(
            "quectel_maintenance_model_unsupported",
        ));
    }
    let mut unavailable = Vec::new();
    let mut replies = Vec::new();
    for (name, command) in [
        ("ims", "AT+QCFG=\"ims\""),
        ("usbnet", "AT+QCFG=\"usbnet\""),
        ("usbcfg", "AT+QCFG=\"usbcfg\""),
        ("mbn_auto", "AT+QMBNCFG=\"AutoSel\""),
        ("mbn_list", "AT+QMBNCFG=\"List\""),
    ] {
        match at(device, command).await {
            Ok(reply) => replies.push(reply),
            Err(NativeError::OwnerConflict(reason)) => {
                return Err(NativeError::OwnerConflict(reason))
            }
            Err(_) => {
                unavailable.push(name);
                replies.push(String::new());
            }
        }
    }
    let profiles = parse_mbn(&replies[4]);
    if profiles.is_err() {
        unavailable.push("mbn_list_invalid");
    }
    Ok(Diagnostics {
        model,
        ims_mode: qcfg_number(&replies[0], "ims", 2),
        usb_network_mode: qcfg_number(&replies[1], "usbnet", 3),
        usb_composition: qcfg(&replies[2], "usbcfg"),
        mbn_auto_select: auto_select(&replies[3]),
        mbn_profiles: profiles.unwrap_or_default(),
        unavailable,
    })
}

fn validate_against(action: &MaintenanceAction, before: &Diagnostics) -> Result<(), NativeError> {
    validate_action(action)?;
    let available = match action {
        MaintenanceAction::SetIms { .. } => before.ims_mode.is_some(),
        MaintenanceAction::SetUsbNetwork { .. } => before.usb_network_mode.is_some(),
        MaintenanceAction::SelectMbn { profile } => {
            before.mbn_auto_select.is_some()
                && before.mbn_profiles.iter().any(|p| &p.name == profile)
        }
        MaintenanceAction::Reboot => true,
    };
    if available {
        Ok(())
    } else {
        Err(invalid("quectel_maintenance_state_unconfirmed"))
    }
}

fn revision(line_id: &str, action: &MaintenanceAction, before: &Diagnostics) -> String {
    let bytes =
        serde_json::to_vec(&(line_id, action, before)).expect("serializable maintenance plan");
    format!("{:x}", Sha256::digest(bytes))
}

async fn idle_owned(device: &NativeDevice) -> Result<(), NativeError> {
    device.ensure_available()?;
    if !device
        .active_interfaces
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_empty()
    {
        return Err(invalid("native_maintenance_active_bearer"));
    }
    let calls = at(device, "AT+CLCC").await?;
    if at_payload(&calls, "+CLCC:").is_some() || !calls.lines().any(|line| line.trim() == "OK") {
        return Err(invalid("native_maintenance_call_state_not_idle"));
    }
    Ok(())
}

pub async fn inspect(device: Arc<NativeDevice>) -> Result<Diagnostics, NativeError> {
    let _gate = device.operation.lock().await;
    device.ensure_available()?;
    diagnostics_owned(&device).await
}

pub async fn plan(
    device: Arc<NativeDevice>,
    action: MaintenanceAction,
) -> Result<MaintenancePlan, NativeError> {
    validate_action(&action)?;
    let _gate = device.operation.lock().await;
    idle_owned(&device).await?;
    let before = diagnostics_owned(&device).await?;
    validate_against(&action, &before)?;
    Ok(MaintenancePlan {
        line_id: device.spec.line_id(),
        expected_revision: revision(&device.spec.line_id(), &action, &before),
        reboot_required: true,
        requires_confirmation: true,
        action,
        before,
    })
}

pub async fn apply(
    device: Arc<NativeDevice>,
    action: MaintenanceAction,
    expected: String,
    confirm_line_id: String,
) -> Result<MaintenanceResult, NativeError> {
    validate_action(&action)?;
    if confirm_line_id != device.spec.line_id()
        || expected.len() != 64
        || !expected.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid("native_maintenance_confirmation_required"));
    }
    // Shield writes from HTTP cancellation; never release the physical gate
    // while an accepted command may still be changing the USB/SIM state.
    tokio::spawn(async move {
        let _gate = device.operation.lock().await;
        idle_owned(&device).await?;
        let before = diagnostics_owned(&device).await?;
        validate_against(&action, &before)?;
        if expected != revision(&device.spec.line_id(), &action, &before) {
            return Err(invalid("native_maintenance_plan_stale"));
        }
        let receipt = format!("session-{}-maintenance", device.spec.line_id());
        device.io.save_receipt(&receipt, &serde_json::to_vec(&serde_json::json!({
            "line_id": device.spec.line_id(), "purpose": "quectel_maintenance", "action": action,
            "expected_revision": expected, "state": "write_pending"
        })).map_err(|_| invalid("native_maintenance_receipt_invalid"))?, true)?;
        let commands: Vec<(&str, String)> = match &action {
            MaintenanceAction::SetIms { mode } => {
                vec![("set_ims", format!("AT+QCFG=\"ims\",{mode}"))]
            }
            MaintenanceAction::SetUsbNetwork { mode } => {
                vec![("set_usbnet", format!("AT+QCFG=\"usbnet\",{mode}"))]
            }
            MaintenanceAction::SelectMbn { profile } => vec![
                ("disable_mbn_auto", "AT+QMBNCFG=\"AutoSel\",0".into()),
                ("select_mbn", format!("AT+QMBNCFG=\"Select\",\"{profile}\"")),
            ],
            MaintenanceAction::Reboot => vec![("reboot", "AT+CFUN=1,1".into())],
        };
        let mut result = MaintenanceResult {
            status: "unconfirmed",
            completed_steps: Vec::new(),
            failed_step: None,
            restart_required: true,
            reconciliation_required: true,
        };
        for (step, command) in commands {
            if at(&device, &command).await.is_err() {
                result.failed_step = Some(step);
                device.mark_maintenance_required();
                return Ok(result); // No inverse commands or automatic retry.
            }
            result.completed_steps.push(step);
        }
        if matches!(action, MaintenanceAction::Reboot) {
            // CFUN may reset/re-enumerate before a reply or while the reply is
            // in flight. Keep the receipt: no old CID/AT session can be reused.
            result.status = "reboot_requested";
            device.mark_maintenance_required();
            return Ok(result);
        }
        let verified = match &action {
            MaintenanceAction::SetIms { mode } => {
                at(&device, "AT+QCFG=\"ims\"")
                    .await
                    .ok()
                    .and_then(|r| qcfg_number(&r, "ims", 2))
                    == Some(*mode)
            }
            MaintenanceAction::SetUsbNetwork { mode } => {
                at(&device, "AT+QCFG=\"usbnet\"")
                    .await
                    .ok()
                    .and_then(|r| qcfg_number(&r, "usbnet", 3))
                    == Some(*mode)
            }
            MaintenanceAction::SelectMbn { profile } => {
                let auto = at(&device, "AT+QMBNCFG=\"AutoSel\"")
                    .await
                    .ok()
                    .and_then(|r| auto_select(&r));
                let profiles = at(&device, "AT+QMBNCFG=\"List\"")
                    .await
                    .ok()
                    .and_then(|r| parse_mbn(&r).ok());
                auto == Some(false)
                    && profiles.is_some_and(|ps| {
                        ps.iter().any(|p| p.selected && &p.name == profile)
                            && ps.iter().filter(|p| p.selected).count() == 1
                    })
            }
            MaintenanceAction::Reboot => false,
        };
        if verified && device.io.clear_receipt(&receipt).is_ok() {
            result.status = "verified_setting";
            result.reconciliation_required = false;
        } else {
            result.failed_step = Some("readback_or_receipt_cleanup");
            device.mark_maintenance_required();
        }
        // Settings may require a separate explicit reboot. Never reboot merely
        // because an IMS/MBN/USB setting was requested.
        Ok(result)
    })
    .await
    .map_err(|_| NativeError::CommandFailed("native_maintenance_task_failed"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FakeIo {
        commands: std::sync::Mutex<Vec<String>>,
        receipts: std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
        mode: std::sync::atomic::AtomicU8,
        fail_write: bool,
        invalidated: std::sync::atomic::AtomicBool,
    }
    impl crate::hardware::cellular::backends::io::NativeIo for FakeIo {
        fn execute<'a>(
            &'a self,
            request: &'a crate::hardware::cellular::backends::protocol::CommandRequest,
        ) -> crate::hardware::devices::transport::TransportFuture<'a, Result<String, NativeError>>
        {
            Box::pin(async move {
                let command = &request.arguments[0];
                self.commands.lock().unwrap().push(command.clone());
                let reply = match command.as_str() {
                    "AT+CGMM" => "EC25-E\r\nOK".into(),
                    "AT+CLCC" => "OK".into(),
                    "AT+QCFG=\"ims\"" => format!(
                        "+QCFG: \"ims\",{}\r\nOK",
                        self.mode.load(std::sync::atomic::Ordering::Acquire)
                    ),
                    "AT+QCFG=\"usbnet\"" => "+QCFG: \"usbnet\",0\r\nOK".into(),
                    "AT+QCFG=\"usbcfg\"" => {
                        "+QCFG: \"usbcfg\",0x2c7c,0x0125,1,1,1,1,1\r\nOK".into()
                    }
                    "AT+QMBNCFG=\"AutoSel\"" => "+QMBNCFG: \"AutoSel\",1\r\nOK".into(),
                    "AT+QMBNCFG=\"List\"" => {
                        "+QMBNCFG: \"List\",0,1,1,\"Commercial-A\"\r\nOK".into()
                    }
                    "AT+QCFG=\"ims\",1" => {
                        if self.fail_write {
                            return Err(NativeError::CommandFailed("fixture_timeout"));
                        }
                        self.mode.store(1, std::sync::atomic::Ordering::Release);
                        "OK".into()
                    }
                    _ => return Err(NativeError::CommandFailed("unexpected_fixture_command")),
                };
                Ok(reply)
            })
        }
        fn save_receipt(&self, key: &str, bytes: &[u8], create: bool) -> Result<(), NativeError> {
            let mut receipts = self.receipts.lock().unwrap();
            if create && receipts.contains_key(key) {
                return Err(invalid("existing_fixture_receipt"));
            }
            receipts.insert(key.into(), bytes.to_vec());
            Ok(())
        }
        fn clear_receipt(&self, key: &str) -> Result<(), NativeError> {
            self.receipts
                .lock()
                .unwrap()
                .remove(key)
                .ok_or_else(|| invalid("missing_fixture_receipt"))?;
            Ok(())
        }
        fn invalidate(&self) {
            self.invalidated
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
    fn fixture(fail_write: bool) -> (Arc<NativeDevice>, Arc<FakeIo>) {
        let io = Arc::new(FakeIo {
            commands: Default::default(),
            receipts: Default::default(),
            mode: std::sync::atomic::AtomicU8::new(0),
            fail_write,
            invalidated: Default::default(),
        });
        let device = NativeDevice::new(
            crate::hardware::cellular::backends::config::NativeDeviceConfig {
                hardware_key: "fixture-quectel".into(),
                sysfs_anchor: "/sys/devices/fixture".into(),
                protocol: crate::hardware::cellular::backends::config::NativeProtocol::At,
                control_device: "/dev/fixture".into(),
                at_device: None,
                sms_reception_enabled: false,
                uim_slot: 1,
                ims: None,
                data: None,
            },
            io.clone(),
        );
        (device, io)
    }
    #[tokio::test]
    async fn explicit_plan_apply_verifies_setting_without_automatic_reboot() {
        let (device, io) = fixture(false);
        let plan = plan(device.clone(), MaintenanceAction::SetIms { mode: 1 })
            .await
            .unwrap();
        let result = apply(device, plan.action, plan.expected_revision, plan.line_id)
            .await
            .unwrap();
        assert_eq!(result.status, "verified_setting");
        assert!(!result.reconciliation_required);
        assert!(io.receipts.lock().unwrap().is_empty());
        assert!(!io
            .commands
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("CFUN")));
    }
    #[tokio::test]
    async fn stale_plan_wrong_line_and_active_bearer_never_write() {
        let (device, io) = fixture(false);
        let plan = plan(device.clone(), MaintenanceAction::SetIms { mode: 1 })
            .await
            .unwrap();
        assert!(apply(
            device.clone(),
            plan.action.clone(),
            plan.expected_revision.clone(),
            "wrong-line".into()
        )
        .await
        .is_err());
        io.mode.store(2, std::sync::atomic::Ordering::Release);
        assert!(apply(
            device.clone(),
            plan.action.clone(),
            plan.expected_revision,
            plan.line_id
        )
        .await
        .is_err());
        assert!(!io
            .commands
            .lock()
            .unwrap()
            .iter()
            .any(|s| s == "AT+QCFG=\"ims\",1"));
        device
            .active_interfaces
            .lock()
            .unwrap()
            .insert("wwan0".into(), "owned".into());
        assert!(super::plan(device, plan.action).await.is_err());
        assert!(io.receipts.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn ambiguous_write_retains_receipt_and_fences_further_io() {
        let (device, io) = fixture(true);
        let plan = plan(device.clone(), MaintenanceAction::SetIms { mode: 1 })
            .await
            .unwrap();
        let result = apply(
            device.clone(),
            plan.action,
            plan.expected_revision,
            plan.line_id,
        )
        .await
        .unwrap();
        assert_eq!(result.status, "unconfirmed");
        assert!(result.reconciliation_required);
        assert_eq!(io.receipts.lock().unwrap().len(), 1);
        assert!(io.invalidated.load(std::sync::atomic::Ordering::Acquire));
        assert!(device.at("AT").await.is_err());
    }

    #[test]
    fn validates_only_known_modes_and_safe_explicit_mbn_names() {
        for action in [
            MaintenanceAction::SetIms { mode: 3 },
            MaintenanceAction::SetUsbNetwork { mode: 2 },
            MaintenanceAction::SelectMbn {
                profile: "x\"\rAT+CFUN=1,1".into(),
            },
        ] {
            assert!(validate_action(&action).is_err());
        }
        assert!(validate_action(&MaintenanceAction::SelectMbn {
            profile: "Commercial-VoLTE".into()
        })
        .is_ok());
    }
    #[test]
    fn parses_real_qcfg_and_mbn_rows_without_cross_field_guessing() {
        assert_eq!(qcfg_number("+QCFG: \"ims\",1,1\r\nOK", "ims", 2), Some(1));
        assert_eq!(qcfg_number("+QCFG: \"usbnet\",0", "ims", 2), None);
        assert_eq!(
            qcfg_number("+QCFG: \"ims\",1\n+QCFG: \"ims\",2", "ims", 2),
            None
        );
        let rows = parse_mbn("+QMBNCFG: \"List\",0,1,1,\"Commercial-A\",0x123\nOK").unwrap();
        assert_eq!(rows[0].name, "Commercial-A");
        assert!(rows[0].selected && rows[0].activated);
        assert!(
            parse_mbn("+QMBNCFG: \"List\",0,1,1,\"A\"\n+QMBNCFG: \"List\",0,0,0,\"B\"").is_err()
        );
        assert_eq!(auto_select("+QMBNCFG: \"AutoSel\",0\nOK"), Some(false));
    }
    #[test]
    fn plans_bind_action_line_and_observed_settings() {
        let mut before = Diagnostics {
            model: "EC25".into(),
            ims_mode: Some(0),
            usb_network_mode: Some(0),
            usb_composition: None,
            mbn_auto_select: None,
            mbn_profiles: vec![],
            unavailable: vec![],
        };
        let action = MaintenanceAction::SetIms { mode: 1 };
        let original = revision("line-a", &action, &before);
        assert_ne!(original, revision("line-b", &action, &before));
        assert_ne!(
            original,
            revision("line-a", &MaintenanceAction::Reboot, &before)
        );
        before.ims_mode = Some(2);
        assert_ne!(original, revision("line-a", &action, &before));
        assert!(validate_against(
            &MaintenanceAction::SelectMbn {
                profile: "guessed".into()
            },
            &before
        )
        .is_err());
    }
}
