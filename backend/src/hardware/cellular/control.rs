//! Compatibility facade for legacy API/service call signatures.
//!
//! Only the MM branch uses its D-Bus argument. Native controllers are selected
//! once at startup and use real native selectors; an unknown native target or
//! failed native operation MUST NOT fall through to ModemManager.

use super::{
    backends::{self, native::NativeDevice, protocol::RegistrationState, NativeError},
    modem_manager as mm,
    observations::ModemObservationProvider,
    radio::{ModemRadioControl, RadioState},
};
use crate::{
    api::models::*,
    platform::{config::ApnConfig, db::Database},
};
use std::{collections::HashMap, sync::Arc};
use zbus::Connection;

pub use super::bindings::{ModemBinding, SimIdentity};
// Storage/format compatibility helpers, not device IO or ownership.
pub(crate) use mm::BearerTrafficStats;
pub use mm::{
    cache_own_numbers_for_identity, cache_smsc_for_identity, cached_sim_metadata_for_identity,
    clear_non_manual_own_numbers_for_iccid, get_baseband_restart_progress_for_line,
    invalidate_sim_identity_cache, record_restart_step, record_restart_step_for_line,
    reset_baseband_restart_progress, reset_baseband_restart_progress_for_line,
    with_baseband_restart_progress, BasebandRestartRunGuard, QmiServingSystem,
};

fn bus_error(error: NativeError) -> zbus::Error {
    zbus::fdo::Error::Failed(error.to_string()).into()
}

fn route(selector: &str) -> Result<Option<Arc<NativeDevice>>, NativeError> {
    if let Some(fleet) = backends::active_native() {
        return fleet.device(selector).map(Some);
    }
    if backends::is_native_selector(selector) {
        return Err(NativeError::Unavailable(
            "native_backend_not_selected".into(),
        ));
    }
    Ok(None)
}

pub fn runtime_radio(connection: Arc<Connection>) -> Arc<dyn ModemRadioControl> {
    if let Some(fleet) = backends::active_native() {
        fleet.clone()
    } else {
        Arc::new(super::mm_radio::ModemManagerRadio::new(connection))
    }
}

pub async fn discover_modem_bindings(conn: &Connection) -> zbus::Result<Vec<ModemBinding>> {
    if let Some(fleet) = backends::active_native() {
        return fleet
            .discover()
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()).into());
    }
    mm::discover_modem_bindings(conn).await
}

pub async fn list_modem_paths(conn: &Connection) -> zbus::Result<Vec<String>> {
    if let Some(fleet) = backends::active_native() {
        return Ok(fleet.all().iter().map(|d| d.spec.selector()).collect());
    }
    mm::list_modem_paths(conn).await
}

macro_rules! query {
    ($name:ident -> $result:ty, |$device:ident| $native:expr) => {
        pub async fn $name(conn: &Connection, modem_path: &str) -> zbus::Result<$result> {
            if let Some($device) = route(modem_path).map_err(bus_error)? {
                let result: Result<$result, NativeError> = ($native).await;
                result.map_err(bus_error)
            } else {
                mm::$name(conn, modem_path).await
            }
        }
    };
}

query!(get_device_info_for_modem -> DeviceInfoResponse, |device| async {
    let snapshot = device.refresh().await?;
    Ok(DeviceInfoResponse {
        imei: snapshot.equipment_identifier, manufacturer: snapshot.manufacturer, model: snapshot.model,
        revision: None, powered: snapshot.radio != RadioState::Unknown, online: snapshot.radio == RadioState::On,
    })
});

query!(get_network_info_for_modem -> NetworkInfoResponse, |device| async {
    let snapshot = device.network().await?;
    let (mcc, mnc) = split_plmn(snapshot.plmn.as_deref());
    Ok(NetworkInfoResponse { operator_name: snapshot.operator, registration_status: snapshot.registration.label().into(),
        technology_preference: snapshot.technology, signal_strength: snapshot.signal_percent.unwrap_or(0), mcc, mnc })
});

query!(get_is_roaming_for_modem -> bool, |device| async {
    device.network().await?.registration.roaming()
});

query!(get_modem_state_for_modem -> i32, |device| async {
    // Numeric state is a compatibility projection for legacy UI/watchdogs,
    // never a fake MM object or protocol requirement of the native controller.
    Ok(match device.radio().await? {
        RadioState::Off => 3,
        RadioState::TurningOff => 4,
        RadioState::TurningOn => 5,
        RadioState::Unknown => 0,
        RadioState::On => match device.network().await?.registration {
            RegistrationState::Home | RegistrationState::Roaming => 8,
            RegistrationState::Searching => 7,
            _ => 6,
        },
    })
});

query!(get_signal_strength_for_modem -> SignalStrengthResponse, |device| async {
    let percent = device.network().await?.signal_percent
        .ok_or_else(|| NativeError::Unavailable("native_signal_unknown".into()))?;
    Ok(SignalStrengthResponse { strength: percent.into() })
});

query!(get_cells_data_for_modem -> CellsResponse, |device| async {
    let network = device.network().await?;
    let serving = ServingCell { tech: network.technology.clone(), cell_id: network.cell_id.unwrap_or(0), tac: network.tac.unwrap_or(0) };
    let cells = network.cell_id.map(|id| CellInfo {
        is_serving: true, tech: network.technology, cell_id: id, ..Default::default()
    }).into_iter().collect();
    Ok(CellsResponse { serving_cell: serving, cells })
});

query!(get_cell_location_for_modem -> CellLocationResponse, |device| async {
    let network = device.network().await?;
    let (mcc, mnc) = split_plmn(network.plmn.as_deref());
    let cell = network.cell_id.zip(network.tac).zip(mcc.zip(mnc)).map(|((cid, lac), (mcc, mnc))| CellLocationInfo {
        mcc, mnc, cid, lac, signal_strength: network.signal_percent.unwrap_or(0).into(),
        radio_type: network.technology, ..Default::default()
    });
    Ok(CellLocationResponse { available: cell.is_some(), cell_info: cell, ..Default::default() })
});

query!(list_current_calls_for_modem -> CallListResponse, |device| async { device.calls().await });
query!(get_call_settings_for_modem -> CallSettingsResponse, |device| async { device.call_settings().await });
query!(hangup_all_calls_for_modem -> (), |device| async { device.hangup(None).await });

// A missing device-specific capability is reported, never implemented by
// quietly invoking MM or returning a fabricated unlocked/default state.
query!(get_radio_mode_for_modem -> RadioModeResponse, |device| async {
    device.radio_mode().await
});
query!(get_band_lock_status_for_modem -> BandLockStatus, |device| async {
    device.bands().await
});
query!(get_operators_list_for_modem -> OperatorListResponse, |device| async {
    let network = device.network().await?;
    let (mcc, mnc) = split_plmn(network.plmn.as_deref());
    let operators = mcc.zip(mnc).map(|(mcc,mnc)| OperatorInfo {
        path: format!("{mcc}{mnc}"), name: network.operator,
        status: if network.registration.registered() { "current" } else { "unknown" }.into(),
        mcc, mnc, technologies: vec![network.technology],
    }).into_iter().collect();
    Ok(OperatorListResponse { operators })
});
query!(scan_operators_for_modem -> OperatorListResponse, |device| async {
    let output = device.at("AT+COPS=?").await?;
    parse_operator_scan(&output)
});

pub async fn sim_identity_for_modem(conn: &Connection, modem_path: &str) -> Option<SimIdentity> {
    match route(modem_path) {
        Ok(Some(device)) => device.sim_identity().await.ok(),
        Ok(None) => mm::sim_identity_for_modem(conn, modem_path).await,
        Err(_) => None,
    }
}

pub async fn get_sim_info_for_modem_with_cache(
    conn: &Connection,
    modem_path: &str,
    db: Option<&Database>,
) -> zbus::Result<SimInfoResponse> {
    let Some(device) = route(modem_path).map_err(bus_error)? else {
        return mm::get_sim_info_for_modem_with_cache(conn, modem_path, db).await;
    };
    let snapshot = device.refresh().await.map_err(bus_error)?;
    let (mcc, mnc) = split_plmn(Some(&snapshot.identity.operator_id));
    let (phone_numbers, sms_center, phone_number_is_manual, sms_center_is_manual) = db
        .map(|db| cached_sim_metadata_for_identity(db, &snapshot.identity))
        .unwrap_or_default();
    Ok(SimInfoResponse {
        present: !snapshot.identity.imsi.is_empty(),
        iccid: snapshot.identity.iccid,
        imsi: snapshot.identity.imsi,
        mcc: mcc.unwrap_or_default(),
        mnc: mnc.unwrap_or_default(),
        modem_path: device.spec.selector(),
        sim_path: device.spec.selector(),
        active: true,
        sim_type: "unknown".into(),
        esim_status: "unknown".into(),
        registered_operator_code: snapshot.network.plmn.unwrap_or_default(),
        registered_operator_name: snapshot.network.operator,
        lock_status: snapshot.pin_state,
        phone_numbers,
        sms_center,
        phone_number_is_manual,
        sms_center_is_manual,
        ..Default::default()
    })
}

fn split_plmn(plmn: Option<&str>) -> (Option<String>, Option<String>) {
    match plmn.filter(|p| backends::protocol::valid_plmn(p)) {
        Some(value) => (Some(value[..3].into()), Some(value[3..].into())),
        None => (None, None),
    }
}

pub async fn get_serving_system_qmicli(
    conn: &Connection,
    path: &str,
) -> Result<QmiServingSystem, String> {
    let Some(device) = route(path).map_err(|e| e.to_string())? else {
        return mm::get_serving_system_qmicli(conn, path).await;
    };
    let network = device.network().await.map_err(|e| e.to_string())?;
    let roaming = network.registration.roaming().map_err(|e| e.to_string())?;
    let (mcc, mnc) = split_plmn(network.plmn.as_deref());
    Ok(QmiServingSystem {
        registration_status: network.registration.label().into(),
        mcc,
        mnc,
        technology: network.technology,
        roaming,
        tac: network.tac.unwrap_or(0),
        cell_id: network.cell_id.unwrap_or(0),
    })
}

pub async fn at_command_device_for_modem(conn: &Connection, path: &str) -> Result<String, String> {
    let Some(device) = route(path).map_err(|e| e.to_string())? else {
        return mm::at_command_device_for_modem(conn, path).await;
    };
    device
        .at_request("AT")
        .map(|r| r.device)
        .map_err(|e| e.to_string())
}

pub async fn start_cell_monitoring_for_modem(path: &str) -> Result<(), String> {
    if let Some(device) = route(path).map_err(|e| e.to_string())? {
        device
            .network()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else {
        mm::start_cell_monitoring_for_modem(path).await
    }
}

pub async fn stop_cell_monitoring_for_modem(path: &str) -> Result<(), String> {
    if route(path).map_err(|e| e.to_string())?.is_some() {
        Ok(())
    } else {
        mm::stop_cell_monitoring_for_modem(path).await
    }
}

pub async fn set_radio_mode_for_modem(
    conn: &Connection,
    path: &str,
    mode: RadioMode,
) -> zbus::Result<()> {
    if let Some(device) = route(path).map_err(bus_error)? {
        device.set_radio_mode(mode).await.map_err(bus_error)
    } else {
        mm::set_radio_mode_for_modem(conn, path, mode).await
    }
}

pub async fn set_band_lock_for_modem(
    conn: &Connection,
    path: &str,
    request: &BandLockRequest,
) -> zbus::Result<()> {
    if let Some(device) = route(path).map_err(bus_error)? {
        device.set_bands(request).await.map_err(bus_error)
    } else {
        mm::set_band_lock_for_modem(conn, path, request).await
    }
}

pub async fn resolve_data_apn_config(
    conn: &Connection,
    path: &str,
    configured: Option<&ApnConfig>,
) -> ApnConfig {
    if backends::active_native().is_some() || backends::is_native_selector(path) {
        // Native mode must not guess "internet" after an MM query fails.
        return configured.cloned().unwrap_or_default();
    }
    mm::resolve_data_apn_config(conn, path, configured).await
}

query!(get_data_connection_status_for_modem -> bool, |device| async {
    Ok(device.active_interfaces.lock().unwrap().contains_key("data"))
});

query!(data_interface_for_modem -> Option<String>, |device| async {
    let _ = device;
    // This legacy probe is used to remove host/MM bearers before UE bringup.
    // Native resources are owned/released by their retained transport handle.
    Ok(None)
});

pub async fn disconnect_data_via_modem(conn: &Connection, path: &str) -> Result<(), String> {
    if route(path).map_err(|e| e.to_string())?.is_some() {
        Ok(())
    } else {
        mm::disconnect_data_via_modem(conn, path).await
    }
}

pub async fn set_modem_enabled(
    conn: &Connection,
    path: &str,
    enabled: bool,
) -> Result<i32, String> {
    if let Some(device) = route(path).map_err(|e| e.to_string())? {
        device.airplane(!enabled).await.map_err(|e| e.to_string())?;
        Ok(if enabled { 6 } else { 3 })
    } else {
        mm::set_modem_enabled(conn, path, enabled).await
    }
}

pub async fn request_operator_registration_for_modem(
    conn: &Connection,
    path: &str,
    plmn: &str,
) -> Result<(), String> {
    if let Some(device) = route(path).map_err(|e| e.to_string())? {
        device.register(plmn).await.map_err(|e| e.to_string())
    } else {
        mm::request_operator_registration_for_modem(conn, path, plmn).await
    }
}

pub async fn register_operator_for_modem(
    conn: &Connection,
    path: &str,
    plmn: &str,
) -> Result<(), String> {
    if let Some(device) = route(path).map_err(|e| e.to_string())? {
        device.register(plmn).await.map_err(|e| e.to_string())
    } else {
        mm::register_operator_for_modem(conn, path, plmn).await
    }
}

pub async fn get_bearer_stats_by_interface(
    conn: &Connection,
) -> zbus::Result<HashMap<String, BearerTrafficStats>> {
    if backends::active_native().is_some() {
        return Ok(HashMap::new());
    }
    mm::get_bearer_stats_by_interface(conn).await
}
pub async fn get_bearer_stats_for_interface(
    conn: &Connection,
    name: &str,
) -> zbus::Result<Option<BearerTrafficStats>> {
    if backends::active_native().is_some() {
        return Ok(None);
    }
    mm::get_bearer_stats_for_interface(conn, name).await
}

pub async fn make_call_on_modem(
    conn: &Connection,
    path: &str,
    number: &str,
) -> zbus::Result<String> {
    if let Some(device) = route(path).map_err(bus_error)? {
        device.dial(number).await.map_err(bus_error)
    } else {
        mm::make_call_on_modem(conn, path, number).await
    }
}
pub async fn get_call_by_path_for_modem(
    conn: &Connection,
    path: &str,
    call: &str,
) -> zbus::Result<CallInfo> {
    if let Some(device) = route(path).map_err(bus_error)? {
        device.call(call).await.map_err(bus_error)
    } else {
        mm::get_call_by_path_for_modem(conn, path, call).await
    }
}
pub async fn hangup_call_on_modem(conn: &Connection, path: &str, call: &str) -> zbus::Result<()> {
    if let Some(device) = route(path).map_err(bus_error)? {
        device.hangup(Some(call)).await.map_err(bus_error)
    } else {
        mm::hangup_call_on_modem(conn, path, call).await
    }
}
pub async fn answer_call_on_modem(conn: &Connection, path: &str, call: &str) -> zbus::Result<()> {
    if let Some(device) = route(path).map_err(bus_error)? {
        device.answer(call).await.map_err(bus_error)
    } else {
        mm::answer_call_on_modem(conn, path, call).await
    }
}
pub async fn send_call_dtmf_on_modem(
    conn: &Connection,
    path: &str,
    call: &str,
    digit: &str,
) -> zbus::Result<()> {
    if let Some(device) = route(path).map_err(bus_error)? {
        device.dtmf(call, digit).await.map_err(bus_error)
    } else {
        mm::send_call_dtmf_on_modem(conn, path, call, digit).await
    }
}
pub async fn set_call_waiting_for_modem(
    conn: &Connection,
    path: &str,
    enabled: bool,
) -> zbus::Result<()> {
    if let Some(device) = route(path).map_err(bus_error)? {
        device
            .at(&format!("AT+CCWA=1,{},1", u8::from(enabled)))
            .await
            .map(|_| ())
            .map_err(bus_error)
    } else {
        mm::set_call_waiting_for_modem(conn, path, enabled).await
    }
}

pub async fn run_ussd_at_command_for_modem(
    conn: &Connection,
    path: &str,
    command: &str,
) -> Result<String, String> {
    if let Some(device) = route(path).map_err(|e| e.to_string())? {
        device.ussd(command).await.map_err(|e| e.to_string())
    } else {
        mm::run_ussd_at_command_for_modem(conn, path, command).await
    }
}
pub async fn cancel_ussd_at_command_for_modem(
    conn: &Connection,
    path: &str,
) -> Result<String, String> {
    if let Some(device) = route(path).map_err(|e| e.to_string())? {
        device.at("AT+CUSD=2").await.map_err(|e| e.to_string())
    } else {
        mm::cancel_ussd_at_command_for_modem(conn, path).await
    }
}

pub async fn send_sms_via_modem(
    conn: &Connection,
    path: &str,
    number: &str,
    text: &str,
) -> zbus::Result<String> {
    if let Some(device) = route(path).map_err(bus_error)? {
        device.send_sms(number, text).await.map_err(bus_error)
    } else {
        mm::send_sms_via_modem(conn, path, number, text).await
    }
}

pub async fn restart_baseband_via_modem(
    conn: &Connection,
    line_id: &str,
    path: &str,
    data: bool,
    roaming: bool,
    apn: Option<ApnConfig>,
) -> Result<BasebandRestartResponse, String> {
    if let Some(device) = route(path).map_err(|e| e.to_string())? {
        native_reset(device, line_id, false).await
    } else {
        mm::restart_baseband_via_modem(conn, line_id, path, data, roaming, apn).await
    }
}
pub async fn recover_absent_baseband_via_qmi(
    conn: &Connection,
    line_id: &str,
    port: &str,
    data: bool,
    roaming: bool,
    apn: Option<ApnConfig>,
) -> Result<BasebandRestartResponse, String> {
    if let Some(fleet) = backends::active_native() {
        return native_reset(
            fleet.by_control_device(port).map_err(|e| e.to_string())?,
            line_id,
            false,
        )
        .await;
    }
    mm::recover_absent_baseband_via_qmi(conn, line_id, port, data, roaming, apn).await
}
pub async fn power_cycle_sim_for_profile_switch_via_modem(
    conn: &Connection,
    line_id: &str,
    path: &str,
    port: Option<&str>,
    data: bool,
    roaming: bool,
    apn: Option<ApnConfig>,
) -> Result<BasebandRestartResponse, String> {
    if let Some(device) = route(path).map_err(|e| e.to_string())? {
        native_reset(device, line_id, true).await
    } else {
        mm::power_cycle_sim_for_profile_switch_via_modem(
            conn, line_id, path, port, data, roaming, apn,
        )
        .await
    }
}

async fn native_reset(
    device: Arc<NativeDevice>,
    line_id: &str,
    sim_only: bool,
) -> Result<BasebandRestartResponse, String> {
    if device.spec.line_id() != line_id {
        return Err("native_reset_line_owner_mismatch".into());
    }
    if !device.active_interfaces.lock().unwrap().is_empty() {
        return Err("native_reset_requires_releasing_owned_bearers".into());
    }
    device.reset(sim_only).await.map_err(|e| e.to_string())?;
    record_restart_step_for_line(
        line_id,
        "原生重置请求",
        "ok",
        Some("等待设备重新出现；未启动 MM 或普通数据".into()),
    );
    Ok(get_baseband_restart_progress_for_line(line_id))
}

pub async fn ensure_nm_modem_profile() -> String {
    if backends::active_native().is_some() {
        "native backend: MM/NM profile initialization skipped".into()
    } else {
        mm::ensure_nm_modem_profile().await
    }
}

/// Shared AT entry for IMS context/SMSC/voicemail queries. Kept here so raw
/// mmcli invocations cannot hide inside SIP, APN or UI code in native mode.
pub async fn at_command(selector: &str, command: &str) -> Result<String, String> {
    if let Some(device) = route(selector).map_err(|e| e.to_string())? {
        return device.at(command).await.map_err(|e| e.to_string());
    }
    let output = tokio::process::Command::new("mmcli")
        .args(["-m", selector, &format!("--command={command}")])
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|_| "mm_at_command_spawn_failed".to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "mm_at_command_failed:{}",
            output.status.code().unwrap_or(-1)
        ))
    }
}

/// Temporary projection for the existing IMS profile resolver. These keys are
/// parser compatibility, not synthetic MM object paths or a native dependency
/// on a daemon. Delete the projection when the resolver accepts typed metadata.
pub async fn native_ims_properties(selector: &str, sim: bool) -> Result<String, String> {
    let device = backends::native_device(selector).map_err(|e| e.to_string())?;
    if sim {
        let identity = device.sim_identity().await.map_err(|e| e.to_string())?;
        return Ok(format!(
            "sim.properties.imsi : {}\nsim.properties.operator-code : {}\n",
            identity.imsi, identity.operator_id
        ));
    }
    let snapshot = device.refresh().await.map_err(|e| e.to_string())?;
    Ok(format!("modem.generic.sim : {}\nmodem.3gpp.operator-code : {}\nmodem.3gpp.registration-state : {}\n",
        selector, snapshot.network.plmn.unwrap_or_default(),
        match snapshot.network.registration {
            RegistrationState::Home => "home",
            RegistrationState::Roaming => "roaming",
            RegistrationState::Searching => "searching",
            _ => "unknown",
        }))
}

fn parse_operator_scan(output: &str) -> Result<OperatorListResponse, NativeError> {
    let payload = super::backends::protocol::at_payload(output, "+COPS:")
        .ok_or(NativeError::Protocol("native_operator_scan_invalid".into()))?;
    let mut operators = Vec::new();
    for row in payload
        .split('(')
        .skip(1)
        .filter_map(|s| s.split_once(')').map(|(row, _)| row))
    {
        let fields = backends::protocol::csv(row)?;
        if fields.len() < 4 || !backends::protocol::valid_plmn(&fields[3]) {
            continue;
        }
        let plmn = &fields[3];
        operators.push(OperatorInfo {
            path: plmn.clone(),
            name: fields[1].clone(),
            mcc: plmn[..3].into(),
            mnc: plmn[3..].into(),
            status: match fields[0].as_str() {
                "1" => "available",
                "2" => "current",
                "3" => "forbidden",
                _ => "unknown",
            }
            .into(),
            technologies: Vec::new(),
        });
    }
    Ok(OperatorListResponse { operators })
}
