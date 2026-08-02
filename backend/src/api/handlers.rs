//! API 处理器模块 (ModemManager 版)
//!
//! 包含所有 HTTP API 的处理函数
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::fs;
use std::process::{Command, Output};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};
use zbus::Connection;

use crate::{
    connectivity::modems::softstack::vowifi::diagnostics::{
        self as vowifi_diagnostics, VowifiDiagnosticsResponse, VowifiProfileMatchResponse,
        VowifiProfilesResponse, VowifiStatusResponse,
    },
    connectivity::modems::softstack::vowifi::restore::RestorePhase,
    connectivity::modems::softstack::vowifi::{
        live::{
            clear_all_live_runtime, clear_live_runtime_for_line, place_live_voice_call_for_line,
            send_live_sms_over_ims_for_line, verify_live_sim_auth_access_for_line,
        },
        sms::{MoSmsSipOutcome, MtSmsDeliver},
    },
    api::models::*,
    hardware::cellular::modem_manager,
    hardware::cellular::modem_manager::{
        answer_call, background_fetch_smsc, current_sim_identity, find_nm_modem_connection_pub,
        get_band_lock_status_for_modem, get_baseband_restart_progress, get_call_by_path,
        get_call_settings, get_cell_location, get_cells_data_for_modem, get_data_connection_status,
        get_device_info_data, get_is_roaming_for_modem, get_network_info_data,
        get_operators_list_for_modem, get_radio_mode_for_modem, get_signal_strength,
        get_sim_info_data_with_cache, get_sim_info_for_modem_with_cache, hangup_all_calls,
        hangup_call, list_apn_contexts_for_modem, list_current_calls, make_call_on_modem,
        nm_set_autoconnect_pub, power_cycle_sim_for_profile_switch, register_operator_for_modem,
        restart_baseband, scan_operators_for_modem, send_sms, send_sms_via_modem,
        set_apn_on_bearer, set_band_lock_for_modem, set_call_waiting, set_data_connection_with_apn,
        set_radio_mode_for_modem, start_cell_monitoring, stop_cell_monitoring,
    },
    platform::config::{
        AccessPathKind, ApnConfig, LineDataProxyConfig, LineProfileConfig, LineVowifiConfig,
        MidFlightDisablePolicy, SmsPathPolicy, StandaloneSimSlotConfig, TrunkProfileConfig,
        VilteConfig, VoicePathPolicy, VolteConfig, VowifiConfig,
    },
    platform::db::{
        NewVowifiSmsDelivery, NewVowifiSmsPart, SmsMessage, VowifiEsimRestoreEntry,
        VowifiRuntimeEventsResponse, VowifiSmsDeliveriesResponse, VowifiSoakRunsResponse,
    },
    platform::utils::{
        connection_addresses_from_interfaces, format_uptime, get_active_interfaces, read_cpu_info,
        read_cpu_load_sync, read_disk_info, read_interface_stats, read_memory_info,
        read_network_interfaces, read_system_info, read_uptime, sample_cpu_usage,
    },
    hardware::sim::esim::EsimApiError,
    state::AppState,
    services::system::system_event::{
        codes as system_event_codes, mask_identifier, severity as system_event_severity,
        status as system_event_status,
    },
};

const ESIM_SIM_IDENTITY_TIMEOUT_SECS: u64 = 3;
const ESIM_SIM_ENRICH_TIMEOUT_SECS: u64 = 12;
const VOWIFI_SIM_IDENTITY_TIMEOUT_SECS: u64 = 3;
const VOWIFI_STATUS_STAGE_TIMEOUT_SECS: u64 = 12;
const VOWIFI_LIVE_STAGE_TIMEOUT_SECS: u64 = 90;
const VOWIFI_MANUAL_CONNECT_ATTEMPTS: u8 = 3;
const VOWIFI_MANUAL_CONNECT_RETRY_DELAY_SECS: u64 = 1;
const VOWIFI_PROFILE_SWITCH_RESTORE_INITIAL_DELAY_SECS: u64 = 1;
const VOWIFI_PROFILE_SWITCH_RESTORE_ATTEMPTS: u8 = 3;
const VOWIFI_PROFILE_SWITCH_RESTORE_RETRY_DELAY_SECS: u64 = 3;
const VOWIFI_RESTORE_IDENTITY_GATE_ATTEMPTS: u8 = 5;
const VOLTE_RESTORE_MAX_ATTEMPTS: u32 = 5;
const VOLTE_MODEM_RESTART_MAX_ATTEMPTS: u32 = 3;
const VOLTE_MODEM_MISSING_POLLS: u32 = 6;
const VOLTE_MODEM_MISSING_POLL_DELAY_SECS: u64 = 5;
const VOWIFI_RESTORE_IDENTITY_GATE_DELAY_SECS: u64 = 2;
const VOWIFI_PROFILE_SWITCH_CONNECT_ATTEMPTS: u8 = 2;
const VOWIFI_PROFILE_SWITCH_CONNECT_RETRY_DELAY_SECS: u64 = 1;
const SMS_DB_MAINTENANCE_DELETE_THRESHOLD: usize = 100;
const SMS_DB_MAINTENANCE_DELAY_SECS: u64 = 60;

// ============ 基础接口 ============

/// 处理 OPTIONS 请求（CORS 预检）
pub async fn options_handler() -> impl IntoResponse {
    StatusCode::NO_CONTENT
}

/// Enumerate every physical modem together with its active SIM and independent
/// VoLTE runtime. Discovery is refreshed on demand so hotplug does not require
/// a service restart.
pub async fn get_modem_lines_handler(
    State(app): State<AppState>,
) -> (
    StatusCode,
    Json<ApiResponse<Vec<crate::services::line_registry::LineRuntimeStatus>>>,
) {
    match app.line_registry.refresh(app.dbus_conn.as_ref()).await {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Success",
                app.line_registry.statuses().await,
            )),
        ),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error(format!(
                "Failed to discover modems: {error}"
            ))),
        ),
    }
}

/// GET /api/health
pub async fn health_check() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "message": "Service is running",
            "platform": "linux-modem",
            "version": env!("CARGO_PKG_VERSION"),
        })),
    )
}

fn esim_error_response<T: Default>(error: EsimApiError) -> (StatusCode, Json<ApiResponse<T>>) {
    let status = match error {
        EsimApiError::Disabled => StatusCode::FORBIDDEN,
        EsimApiError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        EsimApiError::Command(_) => StatusCode::OK,
    };
    (status, Json(ApiResponse::<T>::error(error.message())))
}

/// Decide whether a line may run eSIM/lpac operations.
///
/// `Some(false)` on the profile forces the line to behave as a plain SIM, so no
/// lpac command is ever issued. `Some(true)` force-enables management even when
/// the SIM cannot advertise a eUICC (e.g. a bare reader). `None` is the auto
/// policy: management is offered only when the line's SIM reports a eUICC chip
/// through ModemManager (`sim_type`/`esim_status`), which is a cheap signal that
/// costs no extra lpac probe.
fn line_reports_euicc(binding: &crate::hardware::cellular::modem_manager::ModemBinding) -> bool {
    binding.sim_type == "esim"
        || matches!(binding.esim_status.as_str(), "no-profiles" | "with-profiles")
}

/// Returns `Ok(())` when the named line is allowed to run eSIM operations, or an
/// `EsimApiError::Disabled` describing why not. A missing `line_id` keeps the
/// legacy single-reader behaviour (allowed) so older clients still work.
async fn resolve_line_esim_gate(app: &AppState, line_id: Option<&str>) -> Result<(), EsimApiError> {
    let Some(line_id) = line_id.filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    match app.config_manager.get_line_profile(line_id).esim_control {
        Some(false) => Err(EsimApiError::Disabled),
        Some(true) => Ok(()),
        None => {
            // The registry is the only place that knows what the card reported.
            // Refresh when the line is not yet known (first call after boot, a
            // hotplug, or a newly configured standalone reader) so the automatic
            // policy does not deny a line that actually holds a eUICC.
            let mut line = app.line_registry.get(line_id).await;
            if line.is_none() {
                let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
                line = app.line_registry.get(line_id).await;
            }
            let reports_euicc = line
                .map(|line| line_reports_euicc(&line.binding()))
                .unwrap_or(false);
            if reports_euicc {
                Ok(())
            } else {
                Err(EsimApiError::Disabled)
            }
        }
    }
}

fn esim_command_succeeded(response: &EsimCommandResponse) -> bool {
    response.code == 0
        && (response.status.is_empty()
            || response.status.eq_ignore_ascii_case("success")
            || response.status.eq_ignore_ascii_case("ok"))
}

fn esim_profile_is_active(profile: &EsimProfile) -> bool {
    matches!(
        profile.state.trim().to_ascii_lowercase().as_str(),
        "enabled" | "active" | "1" | "true"
    )
}

fn enrich_profiles_with_current_sim(profiles: &mut [EsimProfile], sim: &SimInfoResponse) {
    if !sim.present {
        return;
    }
    let current_index = profiles
        .iter()
        .position(|profile| !sim.iccid.is_empty() && profile.iccid == sim.iccid)
        .or_else(|| profiles.iter().position(esim_profile_is_active));

    let Some(profile) = current_index.and_then(|index| profiles.get_mut(index)) else {
        return;
    };

    if profile.state == "unknown" || !sim.iccid.is_empty() && profile.iccid == sim.iccid {
        profile.state = "enabled".to_string();
    }
    if profile.imsi.is_none() && !sim.imsi.is_empty() {
        profile.imsi = Some(sim.imsi.clone());
    }
    if profile.msisdn.is_none() {
        if let Some(number) = sim
            .phone_numbers
            .iter()
            .find(|number| !number.trim().is_empty())
        {
            profile.msisdn = Some(number.clone());
        }
    }
    if profile.smsc.is_none() && !sim.sms_center.is_empty() {
        profile.smsc = Some(sim.sms_center.clone());
    }
    if profile.mcc.is_none() && !sim.mcc.is_empty() {
        profile.mcc = Some(sim.mcc.clone());
    }
    if profile.mnc.is_none() && !sim.mnc.is_empty() {
        profile.mnc = Some(sim.mnc.clone());
    }
}

fn split_profile_operator_code(code: &str) -> (String, String) {
    let digits: String = code.chars().filter(|ch| ch.is_ascii_digit()).collect();
    if digits.len() >= 6 {
        (digits[..3].to_string(), digits[3..6].to_string())
    } else if digits.len() >= 5 {
        (digits[..3].to_string(), digits[3..].to_string())
    } else {
        (String::new(), String::new())
    }
}

fn enrich_profiles_with_current_identity(
    profiles: &mut [EsimProfile],
    identity: &crate::hardware::cellular::modem_manager::SimIdentity,
) {
    let current_index = profiles
        .iter()
        .position(|profile| !identity.iccid.is_empty() && profile.iccid == identity.iccid)
        .or_else(|| profiles.iter().position(esim_profile_is_active));

    let Some(profile) = current_index.and_then(|index| profiles.get_mut(index)) else {
        return;
    };

    if profile.state == "unknown" || !identity.iccid.is_empty() && profile.iccid == identity.iccid {
        profile.state = "enabled".to_string();
    }
    if profile.imsi.is_none() && !identity.imsi.is_empty() {
        profile.imsi = Some(identity.imsi.clone());
    }
    let (mcc, mnc) = split_profile_operator_code(&identity.operator_id);
    if profile.mcc.is_none() && !mcc.is_empty() {
        profile.mcc = Some(mcc);
    }
    if profile.mnc.is_none() && !mnc.is_empty() {
        profile.mnc = Some(mnc);
    }
}

fn profile_cache_value(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn optional_profile_cache_value(value: &Option<String>) -> Option<String> {
    value.as_deref().and_then(profile_cache_value)
}

fn profile_cache_entry(profile: &EsimProfile) -> EsimProfileCacheEntry {
    EsimProfileCacheEntry {
        iccid: profile.iccid.trim().to_string(),
        name: profile_cache_value(&profile.name),
        provider: profile_cache_value(&profile.provider),
        profile_class: profile_cache_value(&profile.profile_class),
        imsi: optional_profile_cache_value(&profile.imsi),
        msisdn: optional_profile_cache_value(&profile.msisdn),
        smsc: optional_profile_cache_value(&profile.smsc),
        smdp: optional_profile_cache_value(&profile.smdp),
        matching_id: optional_profile_cache_value(&profile.matching_id),
        isdp_aid: optional_profile_cache_value(&profile.isdp_aid),
        mcc: optional_profile_cache_value(&profile.mcc),
        mnc: optional_profile_cache_value(&profile.mnc),
        updated_at: String::new(),
    }
}

fn fill_cached_string(target: &mut String, cached: Option<String>) {
    if target.trim().is_empty() {
        if let Some(value) = cached.and_then(|item| profile_cache_value(&item)) {
            *target = value;
        }
    }
}

fn fill_cached_option(target: &mut Option<String>, cached: Option<String>) {
    if target.as_deref().unwrap_or("").trim().is_empty() {
        if let Some(value) = cached.and_then(|item| profile_cache_value(&item)) {
            *target = Some(value);
        }
    }
}

fn hydrate_profile_from_cache(db: &Database, profile: &mut EsimProfile) {
    let cache = match db.get_esim_profile_cache(&profile.iccid) {
        Ok(Some(cache)) => cache,
        Ok(None) => return,
        Err(err) => {
            warn!(iccid = %profile.iccid, error = %err, "Failed to read eSIM profile cache");
            return;
        }
    };

    fill_cached_string(&mut profile.name, cache.name);
    fill_cached_string(&mut profile.provider, cache.provider);
    fill_cached_string(&mut profile.profile_class, cache.profile_class);
    fill_cached_option(&mut profile.imsi, cache.imsi);
    fill_cached_option(&mut profile.msisdn, cache.msisdn);
    fill_cached_option(&mut profile.smsc, cache.smsc);
    fill_cached_option(&mut profile.smdp, cache.smdp);
    fill_cached_option(&mut profile.matching_id, cache.matching_id);
    fill_cached_option(&mut profile.isdp_aid, cache.isdp_aid);
    fill_cached_option(&mut profile.mcc, cache.mcc);
    fill_cached_option(&mut profile.mnc, cache.mnc);
}

fn hydrate_profiles_from_cache(db: &Database, profiles: &mut [EsimProfile]) {
    for profile in profiles {
        hydrate_profile_from_cache(db, profile);
    }
}

fn cache_esim_profiles(db: &Database, profiles: &[EsimProfile]) {
    for profile in profiles {
        if let Err(err) = db.upsert_esim_profile_cache(&profile_cache_entry(profile)) {
            warn!(iccid = %profile.iccid, error = %err, "Failed to write eSIM profile cache");
        }
    }
}

fn profile_from_cache_entry(entry: EsimProfileCacheEntry) -> EsimProfile {
    EsimProfile {
        iccid: entry.iccid,
        name: entry.name.unwrap_or_default(),
        provider: entry.provider.unwrap_or_default(),
        state: "unknown".to_string(),
        profile_class: entry.profile_class.unwrap_or_default(),
        imsi: entry.imsi,
        msisdn: entry.msisdn,
        smsc: entry.smsc,
        smdp: entry.smdp,
        matching_id: entry.matching_id,
        isdp_aid: entry.isdp_aid,
        mcc: entry.mcc,
        mnc: entry.mnc,
        disable_allowed: Some(true),
        delete_allowed: Some(true),
        raw: json!({
            "source": "cache",
            "updated_at": entry.updated_at,
        }),
    }
}

fn cached_profiles_requested(query: &std::collections::HashMap<String, String>) -> bool {
    query
        .get("cached")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

// ============ eSIM ============

/// GET /api/esim/lpac/status
pub async fn get_esim_lpac_status_handler(State(app): State<AppState>) -> impl IntoResponse {
    match app.esim_supervisor.get_lpac_status().await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(err) => esim_error_response::<EsimLpacStatusResponse>(err),
    }
}

/// POST /api/esim/lpac/repair
pub async fn repair_esim_lpac_handler(
    State(app): State<AppState>,
    Json(payload): Json<EsimLpacRepairRequest>,
) -> impl IntoResponse {
    match app.esim_supervisor.repair_lpac(payload).await {
        Ok(data) => {
            app.system_event_emitter
                .emit_code(
                    system_event_codes::ESIM_LPAC_REPAIR_SUCCEEDED,
                    system_event_severity::INFO,
                    system_event_status::SUCCEEDED,
                    "lpac",
                    "lpac 修复成功",
                )
                .await;
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message("lpac repaired", data)),
            )
        }
        Err(err) => {
            let message = err.message();
            app.system_event_emitter
                .emit_code(
                    system_event_codes::ESIM_LPAC_REPAIR_FAILED,
                    system_event_severity::WARNING,
                    system_event_status::FAILED,
                    "lpac",
                    format!("lpac 修复失败: {message}"),
                )
                .await;
            esim_error_response::<EsimLpacRepairResponse>(err)
        }
    }
}

/// GET /api/esim/config
pub async fn get_esim_config_handler(State(app): State<AppState>) -> impl IntoResponse {
    let esim_config = app.config_manager.get_esim_config();
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", esim_config)),
    )
}

/// POST /api/esim/config
pub async fn set_esim_config_handler(
    State(app): State<AppState>,
    Json(payload): Json<crate::platform::config::EsimConfig>,
) -> impl IntoResponse {
    match app.config_manager.set_esim_config(payload) {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::<()>::success_with_message(
                "eSIM config updated successfully",
                (),
            )),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(err)),
        ),
    }
}

/// GET /api/esim/euicc
pub async fn get_esim_euicc_handler(
    State(app): State<AppState>,
    Query(query): Query<LineScopedQuery>,
) -> impl IntoResponse {
    if let Err(err) = resolve_line_esim_gate(&app, query.line_id.as_deref()).await {
        return esim_error_response::<EsimEuiccInfo>(err);
    }
    match app
        .esim_supervisor
        .get_euicc_info_for_line(query.line_id.as_deref())
        .await
    {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(err) => esim_error_response::<EsimEuiccInfo>(err),
    }
}

/// GET /api/esim/profiles
pub async fn get_esim_profiles_handler(
    State(app): State<AppState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    if cached_profiles_requested(&query) {
        return match app.database.list_esim_profile_cache() {
            Ok(entries) => (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Cached profiles",
                    EsimProfilesResponse {
                        profiles: entries.into_iter().map(profile_from_cache_entry).collect(),
                    },
                )),
            ),
            Err(err) => (
                StatusCode::OK,
                Json(ApiResponse::<EsimProfilesResponse>::error(format!(
                    "Failed to read cached profiles: {err}"
                ))),
            ),
        };
    }

    let line_id = query.get("line_id").map(String::as_str);
    if let Err(err) = resolve_line_esim_gate(&app, line_id).await {
        return esim_error_response::<EsimProfilesResponse>(err);
    }
    match app.esim_supervisor.get_profiles_for_line(line_id).await {
        Ok(mut data) => {
            hydrate_profiles_from_cache(&app.database, &mut data.profiles);
            match tokio::time::timeout(
                std::time::Duration::from_secs(ESIM_SIM_IDENTITY_TIMEOUT_SECS),
                current_sim_identity(&app.dbus_conn),
            )
            .await
            {
                Ok(Some(identity)) => {
                    enrich_profiles_with_current_identity(&mut data.profiles, &identity)
                }
                Ok(None) => {}
                Err(_) => warn!(
                    timeout_secs = ESIM_SIM_IDENTITY_TIMEOUT_SECS,
                    "Timed out enriching eSIM profiles with current SIM identity"
                ),
            }
            match tokio::time::timeout(
                std::time::Duration::from_secs(ESIM_SIM_ENRICH_TIMEOUT_SECS),
                get_sim_info_data_with_cache(&app.dbus_conn, Some(&app.database)),
            )
            .await
            {
                Ok(Ok(sim_info)) => enrich_profiles_with_current_sim(&mut data.profiles, &sim_info),
                Ok(Err(err)) => {
                    warn!(error = %err, "Failed to enrich eSIM profiles with current SIM")
                }
                Err(_) => warn!(
                    timeout_secs = ESIM_SIM_ENRICH_TIMEOUT_SECS,
                    "Timed out enriching eSIM profiles with current SIM"
                ),
            }
            cache_esim_profiles(&app.database, &data.profiles);
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message("Success", data)),
            )
        }
        Err(err) => esim_error_response::<EsimProfilesResponse>(err),
    }
}

/// POST /api/esim/profiles/{iccid}/enable
pub async fn enable_esim_profile_handler(
    State(app): State<AppState>,
    Path(iccid): Path<String>,
    Query(query): Query<LineScopedQuery>,
) -> impl IntoResponse {
    if let Err(err) = resolve_line_esim_gate(&app, query.line_id.as_deref()).await {
        return esim_error_response::<EsimCommandResponse>(err).into_response();
    }
    let event_entity = mask_identifier(&iccid);
    let bg_line_id = query.line_id.clone();
    let vowifi_before_switch = app.config_manager.get_vowifi_config();
    let switch_token = new_vowifi_switch_token("profile-switch");
    if vowifi_before_switch.feature_enabled {
        persist_vowifi_restore_phase(
            &app,
            &switch_token,
            RestorePhase::Snapshot.as_str(),
            Instant::now(),
            false,
            false,
            None,
            0,
        );
        let _ = reset_vowifi_runtime(&app, "vowifi_profile_switch_pre_teardown").await;
    }

    // Reset baseband restart steps progress
    modem_manager::reset_baseband_restart_progress();
    modem_manager::record_restart_step("启用 eSIM Profile", "running", None);

    let bg_app = app.clone();
    let bg_iccid = iccid.clone();
    let bg_event_entity = event_entity.clone();
    let bg_switch_token = switch_token.clone();

    tokio::spawn(async move {
        let _guard = modem_manager::BasebandRestartRunGuard;

        match bg_app
            .esim_supervisor
            .enable_profile(bg_line_id.as_deref(), bg_iccid.clone())
            .await
        {
            Ok(data) => {
                if esim_command_succeeded(&data) {
                    modem_manager::record_restart_step("启用 eSIM Profile", "ok", None);
                    let auto_connect_data = !bg_app.data_user_disabled.load(Ordering::SeqCst);
                    let allow_roaming = bg_app.config_manager.get_roaming_allowed();
                    let apn_config = bg_app.config_manager.get_apn_config();
                    match power_cycle_sim_for_profile_switch(
                        &bg_app.dbus_conn,
                        auto_connect_data,
                        allow_roaming,
                        Some(apn_config),
                    )
                    .await
                    {
                        Ok(_recovery) => {
                            if bg_app.sms_resync.request_scan("profile-switch") {
                                info!("Requested SMS resync after eSIM profile switch");
                            } else {
                                warn!("Failed to request SMS resync after eSIM profile switch");
                            }
                            spawn_vowifi_profile_switch_restore(bg_app.clone(), bg_switch_token);
                            bg_app
                                .system_event_emitter
                                .emit_code(
                                    system_event_codes::ESIM_PROFILE_ENABLE_SUCCEEDED,
                                    system_event_severity::INFO,
                                    system_event_status::SUCCEEDED,
                                    bg_event_entity,
                                    "Profile 启用成功，基带恢复完成",
                                )
                                .await;
                        }
                        Err(err) => {
                            bg_app
                                .system_event_emitter
                                .emit_code(
                                    system_event_codes::ESIM_PROFILE_SWITCH_BASEBAND_RECOVERY_FAILED,
                                    system_event_severity::CRITICAL,
                                    system_event_status::FAILED,
                                    bg_event_entity,
                                    format!("Profile 切换后基带恢复失败: {err}"),
                                )
                                .await;
                            if bg_app
                                .sms_resync
                                .request_scan("profile-switch-recovery-failed")
                            {
                                info!("Requested SMS resync after failed eSIM profile recovery");
                            } else {
                                warn!(
                                    "Failed to request SMS resync after failed eSIM profile recovery"
                                );
                            }
                        }
                    }
                } else {
                    modem_manager::record_restart_step(
                        "启用 eSIM Profile",
                        "error",
                        Some(data.msg.clone()),
                    );
                    bg_app
                        .system_event_emitter
                        .emit_code(
                            system_event_codes::ESIM_PROFILE_ENABLE_FAILED,
                            system_event_severity::WARNING,
                            system_event_status::FAILED,
                            bg_event_entity.clone(),
                            format!("Profile 启用失败: {}", data.msg),
                        )
                        .await;
                }
            }
            Err(err) => {
                let message = err.message();
                modem_manager::record_restart_step(
                    "启用 eSIM Profile",
                    "error",
                    Some(message.clone()),
                );
                bg_app
                    .system_event_emitter
                    .emit_code(
                        system_event_codes::ESIM_PROFILE_ENABLE_FAILED,
                        system_event_severity::WARNING,
                        system_event_status::FAILED,
                        bg_event_entity.clone(),
                        format!("Profile 启用失败: {message}"),
                    )
                    .await;
            }
        }
    });

    let success_resp = EsimCommandResponse {
        code: 0,
        status: "success".to_string(),
        action: "enable".to_string(),
        msg: "Profile enable task started in background".to_string(),
        data: None,
    };
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Profile enable requested",
            success_resp,
        )),
    )
        .into_response()
}

/// POST /api/esim/profiles/{iccid}/rename
pub async fn rename_esim_profile_handler(
    State(app): State<AppState>,
    Path(iccid): Path<String>,
    Query(query): Query<LineScopedQuery>,
    Json(payload): Json<EsimRenameRequest>,
) -> impl IntoResponse {
    if let Err(err) = resolve_line_esim_gate(&app, query.line_id.as_deref()).await {
        return esim_error_response::<EsimCommandResponse>(err);
    }
    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<EsimCommandResponse>::error(
                "Profile name cannot be empty",
            )),
        );
    }
    match app
        .esim_supervisor
        .rename_profile(query.line_id.as_deref(), iccid, name)
        .await
    {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Profile renamed", data)),
        ),
        Err(err) => esim_error_response::<EsimCommandResponse>(err),
    }
}

/// DELETE /api/esim/profiles/{iccid}
pub async fn delete_esim_profile_handler(
    State(app): State<AppState>,
    Path(iccid): Path<String>,
    Query(query): Query<LineScopedQuery>,
) -> impl IntoResponse {
    if let Err(err) = resolve_line_esim_gate(&app, query.line_id.as_deref()).await {
        return esim_error_response::<EsimCommandResponse>(err);
    }
    match app
        .esim_supervisor
        .delete_profile(query.line_id.as_deref(), iccid.clone())
        .await
    {
        Ok(data) => {
            if esim_command_succeeded(&data) {
                if let Err(err) = app.database.delete_esim_profile_cache(&iccid) {
                    warn!(iccid = %iccid, error = %err, "Failed to delete eSIM profile cache");
                }
                app.system_event_emitter
                    .emit_code(
                        system_event_codes::ESIM_PROFILE_DELETED,
                        system_event_severity::WARNING,
                        system_event_status::SUCCEEDED,
                        mask_identifier(&iccid),
                        "Profile 已删除",
                    )
                    .await;
            }
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message("Profile deleted", data)),
            )
        }
        Err(err) => esim_error_response::<EsimCommandResponse>(err),
    }
}

fn find_and_normalize_profile(value: &serde_json::Value) -> Option<EsimProfile> {
    if let Some(obj) = value.as_object() {
        if obj.contains_key("iccid") || obj.contains_key("ICCID") {
            return Some(crate::hardware::sim::esim::normalize_profile(value));
        }
        for (_, val) in obj {
            if let Some(p) = find_and_normalize_profile(val) {
                return Some(p);
            }
        }
    } else if let Some(arr) = value.as_array() {
        for val in arr {
            if let Some(p) = find_and_normalize_profile(val) {
                return Some(p);
            }
        }
    }
    None
}

/// POST /api/esim/profiles
pub async fn download_esim_profile_handler(
    State(app): State<AppState>,
    Query(query): Query<LineScopedQuery>,
    Json(payload): Json<EsimDownloadRequest>,
) -> impl IntoResponse {
    let line_id = query.line_id.as_deref();
    if let Err(err) = resolve_line_esim_gate(&app, line_id).await {
        return esim_error_response::<EsimCommandResponse>(err);
    }
    let smdp = payload.smdp.trim().to_string();
    let matching_id = payload.matching_id.trim().to_string();
    if smdp.is_empty() || matching_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<EsimCommandResponse>::error(
                "SM-DP+ server and Matching ID cannot be empty",
            )),
        );
    }

    // 在写卡前，先异步读取一次卡上的所有 profile ICCID 集合，用于后续新卡判断
    let initial_iccids_opt: Option<std::collections::HashSet<String>> = app
        .esim_supervisor
        .get_profiles_for_line(line_id)
        .await
        .ok()
        .map(|resp| {
            resp.profiles
                .into_iter()
                .map(|p| crate::platform::utils::normalize_iccid(&p.iccid))
                .collect()
        });

    match app
        .esim_supervisor
        .download_profile(line_id, payload.clone())
        .await
    {
        Ok(data) => {
            if esim_command_succeeded(&data) {
                // Attempt to recursively find the downloaded profile details in lpac's response
                let profile_val = data.data.clone().unwrap_or(serde_json::Value::Null);
                if let Some(mut profile) = find_and_normalize_profile(&profile_val) {
                    // Supplement SM-DP+ if not returned
                    if profile.smdp.as_deref().unwrap_or("").trim().is_empty() {
                        profile.smdp = Some(smdp.clone());
                    }
                    if profile
                        .matching_id
                        .as_deref()
                        .unwrap_or("")
                        .trim()
                        .is_empty()
                    {
                        profile.matching_id = Some(matching_id.clone());
                    }

                    let entry = EsimProfileCacheEntry {
                        iccid: profile.iccid.clone(),
                        name: Some(profile.name.clone()),
                        provider: Some(profile.provider.clone()),
                        profile_class: Some(profile.profile_class.clone()),
                        imsi: profile.imsi.clone(),
                        msisdn: profile.msisdn.clone(),
                        smsc: profile.smsc.clone(),
                        smdp: profile.smdp.clone(),
                        matching_id: profile.matching_id.clone(),
                        isdp_aid: profile.isdp_aid.clone(),
                        mcc: profile.mcc.clone(),
                        mnc: profile.mnc.clone(),
                        updated_at: chrono::Utc::now().to_rfc3339(),
                    };

                    if let Err(err) = app.database.upsert_esim_profile_cache(&entry) {
                        warn!(iccid = %entry.iccid, error = %err, "Failed to cache downloaded eSIM profile to database");
                    }

                    app.system_event_emitter
                        .emit_code(
                            system_event_codes::ESIM_PROFILE_DOWNLOAD_SUCCEEDED,
                            system_event_severity::INFO,
                            system_event_status::SUCCEEDED,
                            mask_identifier(&entry.iccid),
                            "Profile 写入并缓存成功",
                        )
                        .await;
                } else {
                    // Fallback if we couldn't parse the profile details from lpac.
                    // Query the profiles on the card to identify the new one(s) that lack smdp/matching_id in cache.
                    let mut cached_fallback_iccid = None;

                    // 1. 等待 1.5 秒，让 eUICC 卡片状态恢复稳定
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

                    // 2. 尝试读取最新列表，最多重试 4 次，每次间隔 1.5 秒
                    let mut profiles_resp = None;
                    for attempt in 1..=4 {
                        match app.esim_supervisor.get_profiles().await {
                            Ok(resp) => {
                                profiles_resp = Some(resp);
                                break;
                            }
                            Err(err) => {
                                warn!(attempt = attempt, error = ?err, "Failed to get profiles during fallback retry");
                                if attempt < 4 {
                                    tokio::time::sleep(std::time::Duration::from_millis(1500))
                                        .await;
                                }
                            }
                        }
                    }

                    if let Some(resp) = profiles_resp {
                        if let Some(ref init_iccids) = initial_iccids_opt {
                            for p in resp.profiles {
                                let norm_iccid = crate::platform::utils::normalize_iccid(&p.iccid);
                                let is_new_profile = !init_iccids.contains(&norm_iccid);

                                if is_new_profile {
                                    let needs_cache =
                                        match app.database.get_esim_profile_cache(&p.iccid) {
                                            Ok(Some(cached_entry)) => cached_entry
                                                .smdp
                                                .as_deref()
                                                .unwrap_or("")
                                                .trim()
                                                .is_empty(),
                                            _ => true,
                                        };
                                    if needs_cache {
                                        let entry = EsimProfileCacheEntry {
                                            iccid: p.iccid.clone(),
                                            name: Some(p.name.clone()),
                                            provider: Some(p.provider.clone()),
                                            profile_class: Some(p.profile_class.clone()),
                                            imsi: p.imsi.clone(),
                                            msisdn: p.msisdn.clone(),
                                            smsc: p.smsc.clone(),
                                            smdp: Some(smdp.clone()),
                                            matching_id: Some(matching_id.clone()),
                                            isdp_aid: p.isdp_aid.clone(),
                                            mcc: p.mcc.clone(),
                                            mnc: p.mnc.clone(),
                                            updated_at: chrono::Utc::now().to_rfc3339(),
                                        };
                                        if let Err(err) =
                                            app.database.upsert_esim_profile_cache(&entry)
                                        {
                                            warn!(iccid = %entry.iccid, error = %err, "Failed to cache fallback eSIM profile to database");
                                        } else {
                                            cached_fallback_iccid = Some(p.iccid.clone());
                                        }
                                    }
                                }
                            }
                        } else {
                            warn!("Initial ICCIDs list was unavailable before writing; fallback difference detection skipped to prevent profile mismatch");
                        }
                    } else {
                        error!("Failed to fetch profiles list after writing even with retries; fallback profile caching cannot proceed");
                    }

                    let event_entity = cached_fallback_iccid
                        .as_ref()
                        .map(|iccid| mask_identifier(iccid))
                        .unwrap_or_else(|| "esim".to_string());

                    app.system_event_emitter
                        .emit_code(
                            system_event_codes::ESIM_PROFILE_DOWNLOAD_SUCCEEDED,
                            system_event_severity::INFO,
                            system_event_status::SUCCEEDED,
                            event_entity,
                            "Profile 写入成功，已通过列表扫描更新缓存",
                        )
                        .await;
                }
            } else {
                let msg = data.msg.clone();
                let is_refused = msg.contains("MatchingID is refused")
                    || msg.contains("es9p_initiate_authentication")
                    || msg.contains("es10b_load_bound_profile_package")
                    || data
                        .data
                        .as_ref()
                        .map(|v| {
                            let s = v.to_string();
                            s.contains("MatchingID is refused")
                                || s.contains("es9p_initiate_authentication")
                                || s.contains("es10b_load_bound_profile_package")
                        })
                        .unwrap_or(false);

                if is_refused {
                    info!("MatchingID is refused, attempting to bind matching info to the profile if it exists");
                    let mut cached_fallback_iccid = None;
                    if let Ok(profiles_resp) = app.esim_supervisor.get_profiles().await {
                        for p in profiles_resp.profiles {
                            let needs_cache = match app.database.get_esim_profile_cache(&p.iccid) {
                                Ok(Some(cached_entry)) => {
                                    cached_entry.smdp.as_deref().unwrap_or("").trim().is_empty()
                                }
                                _ => true,
                            };
                            if needs_cache {
                                let entry = EsimProfileCacheEntry {
                                    iccid: p.iccid.clone(),
                                    name: Some(p.name.clone()),
                                    provider: Some(p.provider.clone()),
                                    profile_class: Some(p.profile_class.clone()),
                                    imsi: p.imsi.clone(),
                                    msisdn: p.msisdn.clone(),
                                    smsc: p.smsc.clone(),
                                    smdp: Some(smdp.clone()),
                                    matching_id: Some(matching_id.clone()),
                                    isdp_aid: p.isdp_aid.clone(),
                                    mcc: p.mcc.clone(),
                                    mnc: p.mnc.clone(),
                                    updated_at: chrono::Utc::now().to_rfc3339(),
                                };
                                // Keep the result alive through the branch so any future Drop
                                // side effects retain the established ordering.
                                #[allow(clippy::redundant_pattern_matching)]
                                if let Ok(_) = app.database.upsert_esim_profile_cache(&entry) {
                                    cached_fallback_iccid = Some(p.iccid.clone());
                                    break;
                                }
                            }
                        }
                    }
                    if let Some(ref iccid) = cached_fallback_iccid {
                        app.system_event_emitter
                            .emit_code(
                                system_event_codes::ESIM_PROFILE_DOWNLOAD_SUCCEEDED,
                                system_event_severity::INFO,
                                system_event_status::SUCCEEDED,
                                mask_identifier(iccid),
                                "Profile 已被使用，成功将 Matching ID 绑定至对应卡片",
                            )
                            .await;
                    }
                }

                app.system_event_emitter
                    .emit_code(
                        system_event_codes::ESIM_PROFILE_DOWNLOAD_FAILED,
                        system_event_severity::WARNING,
                        system_event_status::FAILED,
                        "esim",
                        format!("Profile 写入失败: {}", data.msg),
                    )
                    .await;
            }
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Profile downloaded",
                    data,
                )),
            )
        }
        Err(err) => {
            let message = err.message();
            app.system_event_emitter
                .emit_code(
                    system_event_codes::ESIM_PROFILE_DOWNLOAD_FAILED,
                    system_event_severity::WARNING,
                    system_event_status::FAILED,
                    "esim",
                    format!("Profile 写入失败: {message}"),
                )
                .await;
            esim_error_response::<EsimCommandResponse>(err)
        }
    }
}

// ============ 设备信息 ============

/// GET /api/device
pub async fn get_device_info(State(conn): State<Arc<Connection>>) -> impl IntoResponse {
    match get_device_info_data(&conn).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<DeviceInfoResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

// ============ SIM 卡 ============

/// GET /api/sim
pub async fn get_sim_info(
    State((conn, db)): State<(Arc<Connection>, Arc<Database>)>,
) -> impl IntoResponse {
    match get_sim_info_data_with_cache(&conn, Some(&db)).await {
        Ok(data) => {
            // 如果 SMSC 为空，后台异步通过 AT+CRSM 读取 EF_SMSP 并缓存
            if data.sms_center.is_empty() {
                let conn_bg = Arc::clone(&conn);
                let db_bg = Arc::clone(&db);
                tokio::spawn(async move {
                    background_fetch_smsc(&conn_bg, &db_bg).await;
                });
            }
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message("Success", data)),
            )
        }
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<SimInfoResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// POST /api/sim/cache
pub async fn update_sim_cache_handler(
    State(app): State<AppState>,
    Json(payload): Json<UpdateSimCacheRequest>,
) -> impl IntoResponse {
    let identity = match tokio::time::timeout(
        std::time::Duration::from_secs(ESIM_SIM_IDENTITY_TIMEOUT_SECS),
        current_sim_identity(&app.dbus_conn),
    )
    .await
    {
        Ok(Some(identity)) => identity,
        _ => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiResponse::<serde_json::Value>::error(
                    "Unable to get current SIM identity",
                )),
            );
        }
    };

    if let Some(sms_center) = &payload.sms_center {
        crate::hardware::cellular::modem_manager::cache_smsc_for_identity(
            &app.database,
            &identity,
            sms_center,
            "manual",
        );
    }

    if let Some(phone_number) = &payload.phone_number {
        crate::hardware::cellular::modem_manager::cache_own_numbers_for_identity(
            &app.database,
            &identity,
            std::slice::from_ref(phone_number),
            "manual",
        );
    }

    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "SIM cache updated",
            json!({}),
        )),
    )
}

// ============ 网络信息 ============

/// GET /api/network
pub async fn get_network_info(State(conn): State<Arc<Connection>>) -> impl IntoResponse {
    match get_network_info_data(&conn).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<NetworkInfoResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// Resolve which modem a cellular request targets.
///
/// Named `line_id` wins; omitting it means the primary line, which keeps the
/// single-line API contract intact. Returns `Err` when a line was named but is
/// unknown or absent, so a request never silently reconfigures a *different*
/// baseband than the caller asked for — that silent fallback was the bug where
/// the UI claimed to configure line N while writing to line 1.
async fn resolve_modem_path(app: &AppState, line_id: Option<&str>) -> Result<String, String> {
    let line = match line_id.map(str::trim).filter(|value| !value.is_empty()) {
        Some(line_id) => app
            .line_registry
            .get(line_id)
            .await
            .ok_or_else(|| "line_not_found".to_string())?,
        None => app
            .line_registry
            .primary()
            .await
            .ok_or_else(|| "no_modem_available".to_string())?,
    };
    let binding = line.binding();
    if !binding.present {
        return Err("line_not_present".to_string());
    }
    Ok(binding.modem_path)
}

#[derive(Debug, Default, Deserialize)]
pub struct LineScopedQuery {
    #[serde(default)]
    pub line_id: Option<String>,
}

/// GET /api/cells
pub async fn get_cells(
    State(app): State<AppState>,
    Query(query): Query<LineScopedQuery>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, query.line_id.as_deref()).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<CellsResponse>::error(reason)),
            )
        }
    };
    match get_cells_data_for_modem(&app.dbus_conn, &modem_path).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<CellsResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// POST /api/cell-monitor/start
pub async fn start_cell_monitor_handler(State(app): State<AppState>) -> impl IntoResponse {
    if app.cell_monitoring_active.swap(true, Ordering::SeqCst) {
        return (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Cell monitor already active",
                json!({}),
            )),
        );
    }

    match start_cell_monitoring().await {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Cell monitor activated",
                json!({}),
            )),
        ),
        Err(e) => {
            app.cell_monitoring_active.store(false, Ordering::SeqCst);
            (
                StatusCode::OK,
                Json(ApiResponse::<serde_json::Value>::error(format!(
                    "Failed: {}",
                    e
                ))),
            )
        }
    }
}

/// POST /api/cell-monitor/stop
pub async fn stop_cell_monitor_handler(State(app): State<AppState>) -> impl IntoResponse {
    if !app.cell_monitoring_active.swap(false, Ordering::SeqCst) {
        return (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Cell monitor already inactive",
                json!({}),
            )),
        );
    }

    match stop_cell_monitoring().await {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Cell monitor deactivated",
                json!({}),
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// GET /api/radio-mode
pub async fn get_radio_mode_handler(
    State(app): State<AppState>,
    Query(query): Query<LineScopedQuery>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, query.line_id.as_deref()).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<RadioModeResponse>::error(reason)),
            )
        }
    };
    match get_radio_mode_for_modem(&app.dbus_conn, &modem_path).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<RadioModeResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// POST /api/radio-mode
pub async fn set_radio_mode_handler(
    State(app): State<AppState>,
    Json(payload): Json<RadioModeRequest>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, payload.line_id.as_deref()).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            )
        }
    };
    match set_radio_mode_for_modem(&app.dbus_conn, &modem_path, payload.mode).await {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Radio mode updated",
                json!({}),
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// GET /api/band-lock
pub async fn get_band_lock_handler(
    State(app): State<AppState>,
    Query(query): Query<LineScopedQuery>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, query.line_id.as_deref()).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<BandLockStatus>::error(reason)),
            )
        }
    };
    match get_band_lock_status_for_modem(&app.dbus_conn, &modem_path).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<BandLockStatus>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// POST /api/band-lock
pub async fn set_band_lock_handler(
    State(app): State<AppState>,
    Json(payload): Json<BandLockRequest>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, payload.line_id.as_deref()).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            )
        }
    };
    match set_band_lock_for_modem(&app.dbus_conn, &modem_path, &payload).await {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Band selection updated",
                json!({}),
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// GET /api/location/cell-info
pub async fn get_cell_location_handler(State(conn): State<Arc<Connection>>) -> impl IntoResponse {
    match get_cell_location(&conn).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<CellLocationResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// GET /api/network/operators
pub async fn get_network_operators(
    State(app): State<AppState>,
    Query(query): Query<LineScopedQuery>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, query.line_id.as_deref()).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<OperatorListResponse>::error(reason)),
            )
        }
    };
    match get_operators_list_for_modem(&app.dbus_conn, &modem_path).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<OperatorListResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// GET /api/network/operators/scan
pub async fn scan_network_operators(
    State(app): State<AppState>,
    Query(query): Query<LineScopedQuery>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, query.line_id.as_deref()).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<OperatorListResponse>::error(reason)),
            )
        }
    };
    match scan_operators_for_modem(&app.dbus_conn, &modem_path).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<OperatorListResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// POST /api/network/register-manual
pub async fn register_network_manual(
    State(app): State<AppState>,
    Json(payload): Json<ManualRegisterRequest>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, payload.line_id.as_deref()).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            )
        }
    };
    match register_operator_for_modem(&app.dbus_conn, &modem_path, &payload.mccmnc).await {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Registration started",
                json!({}),
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// POST /api/network/register-auto
pub async fn register_network_auto(
    State(app): State<AppState>,
    Query(query): Query<LineScopedQuery>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, query.line_id.as_deref()).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            )
        }
    };
    match register_operator_for_modem(&app.dbus_conn, &modem_path, "").await {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Auto registration started",
                json!({}),
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// GET /api/apn
pub async fn get_apn_list_handler(
    State(app): State<AppState>,
    Query(query): Query<LineScopedQuery>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, query.line_id.as_deref()).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<ApnListResponse>::error(reason)),
            )
        }
    };
    let apn_config = match query.line_id.as_deref() {
        Some(line_id) => app.config_manager.get_line_apn_config(line_id),
        None => app.config_manager.get_apn_config(),
    };
    match list_apn_contexts_for_modem(&app.dbus_conn, &modem_path, Some(&apn_config)).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<ApnListResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// POST /api/apn
pub async fn set_apn_handler(
    State(app): State<AppState>,
    Json(payload): Json<SetApnRequest>,
) -> impl IntoResponse {
    let line_id = payload
        .line_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut apn_config = match line_id {
        Some(line_id) => app.config_manager.get_line_apn_config(line_id),
        None => app.config_manager.get_apn_config(),
    };
    if let Some(apn) = &payload.apn {
        apn_config.apn = apn.trim().to_string();
    }
    if let Some(protocol) = &payload.protocol {
        apn_config.protocol = protocol.trim().to_string();
    }
    if let Some(username) = &payload.username {
        apn_config.username = username.trim().to_string();
    }
    if let Some(password) = &payload.password {
        apn_config.password = password.clone();
    }
    if let Some(auth_method) = &payload.auth_method {
        apn_config.auth_method = auth_method.trim().to_string();
    }
    if apn_config.protocol.trim().is_empty() {
        apn_config.protocol = ApnConfig::default().protocol;
    }
    if apn_config.auth_method.trim().is_empty() {
        apn_config.auth_method = ApnConfig::default().auth_method;
    }

    let saved = match line_id {
        Some(line_id) => app
            .config_manager
            .set_line_apn_config(line_id, Some(apn_config))
            .map(|_| ()),
        None => app.config_manager.set_apn_config(apn_config),
    };
    if let Err(err) = saved {
        return (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed to save APN config: {}",
                err
            ))),
        );
    }

    let context_path = payload.context_path.trim();
    if context_path.is_empty() || context_path.ends_with("/bearer/default") {
        return (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "APN config saved",
                json!({}),
            )),
        );
    }

    match set_apn_on_bearer(&app.dbus_conn, &payload).await {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("APN updated", json!({}))),
        ),
        Err(e) => {
            warn!(error = %e, "APN config saved but bearer update failed");
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "APN config saved",
                    json!({ "bearer_update_error": e.to_string() }),
                )),
            )
        }
    }
}

/// GET /api/cell-lock
pub async fn get_cell_lock_status_handler(
    State(app): State<AppState>,
    Query(query): Query<LineScopedQuery>,
) -> impl IntoResponse {
    let line_id = match resolve_cell_lock_line(&app, query.line_id.as_deref()).await {
        Ok(line_id) => line_id,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<CellLockStatusResponse>::error(reason)),
            )
        }
    };
    let store = app.cell_lock.lock().await;
    let data = store.get(&line_id).cloned().unwrap_or_default().status();
    drop(store);
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", data)),
    )
}

/// The cell-lock store is keyed by line, so a request must name (or default to)
/// exactly one line before it can read or mutate that line's lock state.
async fn resolve_cell_lock_line(app: &AppState, line_id: Option<&str>) -> Result<String, String> {
    match line_id.map(str::trim).filter(|value| !value.is_empty()) {
        Some(line_id) => app
            .line_registry
            .get(line_id)
            .await
            .map(|line| line.binding().line_id)
            .ok_or_else(|| "line_not_found".to_string()),
        None => app
            .line_registry
            .primary()
            .await
            .map(|line| line.binding().line_id)
            .ok_or_else(|| "no_modem_available".to_string()),
    }
}

/// POST /api/cell-lock
pub async fn set_cell_lock_handler(
    State(app): State<AppState>,
    Json(payload): Json<CellLockRequest>,
) -> impl IntoResponse {
    let line_id = match resolve_cell_lock_line(&app, payload.line_id.as_deref()).await {
        Ok(line_id) => line_id,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<CellLockResult>::error(reason)),
            )
        }
    };
    let mut store = app.cell_lock.lock().await;
    match store.entry(line_id).or_default().apply(&payload) {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "OK",
                CellLockResult { success: true },
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<CellLockResult>::error(e)),
        ),
    }
}

/// POST /api/cell-lock/unlock-all
pub async fn unlock_all_cells_handler(
    State(app): State<AppState>,
    Query(query): Query<LineScopedQuery>,
) -> impl IntoResponse {
    let line_id = match resolve_cell_lock_line(&app, query.line_id.as_deref()).await {
        Ok(line_id) => line_id,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<CellLockResult>::error(reason)),
            )
        }
    };
    let mut store = app.cell_lock.lock().await;
    store.entry(line_id).or_default().unlock_all();
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Unlocked",
            CellLockResult { success: true },
        )),
    )
}

/// GET /api/network/interfaces
pub async fn get_network_interfaces_info(
    State(dbus_conn): State<Arc<Connection>>,
) -> impl IntoResponse {
    match read_network_interfaces(Some(&dbus_conn)).await {
        Ok(interfaces) => {
            let total_count = interfaces.len();
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Success",
                    NetworkInterfacesResponse {
                        interfaces,
                        total_count,
                    },
                )),
            )
        }
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<NetworkInterfacesResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// GET /api/network/connection-addresses
pub async fn get_network_connection_addresses(
    State(dbus_conn): State<Arc<Connection>>,
) -> impl IntoResponse {
    match read_network_interfaces(Some(&dbus_conn)).await {
        Ok(interfaces) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Success",
                connection_addresses_from_interfaces(&interfaces),
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<ConnectionAddressesResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// GET /api/device-network/ddns/config
pub async fn get_device_ddns_config_handler(State(app): State<AppState>) -> impl IntoResponse {
    let config = app.config_manager.get_ddns_config();
    let access_secret_set = !config.access_secret.trim().is_empty();
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            ddns_config_response(config, access_secret_set),
        )),
    )
}

/// POST /api/device-network/ddns/config
pub async fn set_device_ddns_config_handler(
    State(app): State<AppState>,
    Json(mut payload): Json<crate::platform::config::DdnsConfig>,
) -> impl IntoResponse {
    let current = app.config_manager.get_ddns_config();
    if is_masked_secret(&payload.access_id) {
        payload.access_id = current.access_id;
    }
    if payload.access_secret.trim().is_empty() || is_masked_secret(&payload.access_secret) {
        payload.access_secret = current.access_secret;
    }
    if payload.interval_seconds == 0 {
        payload.interval_seconds = 300;
    }
    if payload.ttl == 0 {
        payload.ttl = 600;
    }

    match app.config_manager.set_ddns_config(payload.clone()) {
        Ok(()) => {
            let access_secret_set = !payload.access_secret.trim().is_empty();
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "DDNS config updated",
                    ddns_config_response(payload, access_secret_set),
                )),
            )
        }
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed: {}",
                err
            ))),
        ),
    }
}

fn ddns_config_response(
    mut config: crate::platform::config::DdnsConfig,
    access_secret_set: bool,
) -> serde_json::Value {
    config.access_id = mask_secret(&config.access_id);
    config.access_secret = mask_secret(&config.access_secret);
    let mut value = serde_json::to_value(config).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert("access_secret_set".to_string(), json!(access_secret_set));
    }
    value
}

fn mask_secret(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let prefix: String = trimmed.chars().take(3).collect();
    format!("{prefix}******")
}

fn is_masked_secret(value: &str) -> bool {
    value.contains('*')
}

/// GET /api/device-network/ddns/status
pub async fn get_device_ddns_status_handler(State(app): State<AppState>) -> impl IntoResponse {
    let config = app.config_manager.get_ddns_config();
    let status = app.ddns_manager.status(&config).await;
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", status)),
    )
}

/// POST /api/device-network/ddns/sync
pub async fn sync_device_ddns_handler(State(app): State<AppState>) -> impl IntoResponse {
    match app
        .ddns_manager
        .sync_now(app.config_manager.clone(), app.notification_sender.clone())
        .await
    {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "DDNS sync completed",
                data,
            )),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<DdnsSyncResponse>::error(format!(
                "Failed: {}",
                err
            ))),
        ),
    }
}

/// GET /api/device-network/ddns/logs
pub async fn get_device_ddns_logs_handler(State(app): State<AppState>) -> impl IntoResponse {
    let logs = app.ddns_manager.logs().await;
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", logs)),
    )
}

/// POST /api/device-network/ddns/logs/clear
pub async fn clear_device_ddns_logs_handler(State(app): State<AppState>) -> impl IntoResponse {
    app.ddns_manager.clear_logs().await;
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "DDNS logs cleared",
            json!({}),
        )),
    )
}

/// GET /api/device-network/wlan/status
pub async fn get_device_wlan_status_handler() -> impl IntoResponse {
    match crate::services::network::device_network::wlan_status().await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<WlanStatusResponse>::error(format!(
                "Failed: {}",
                err
            ))),
        ),
    }
}

/// POST /api/device-network/wlan/enabled
pub async fn set_device_wlan_enabled_handler(
    Json(payload): Json<WlanEnabledRequest>,
) -> impl IntoResponse {
    match crate::services::network::device_network::wlan_set_enabled(payload).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "WLAN state updated",
                data,
            )),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<WlanStatusResponse>::error(format!(
                "Failed: {}",
                err
            ))),
        ),
    }
}

/// POST /api/device-network/wlan/scan
pub async fn scan_device_wlan_handler() -> impl IntoResponse {
    match crate::services::network::device_network::wlan_scan().await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<WlanScanResponse>::error(format!(
                "Failed: {}",
                err
            ))),
        ),
    }
}

/// GET /api/device-network/wlan/profiles
pub async fn get_device_wlan_profiles_handler() -> impl IntoResponse {
    match crate::services::network::device_network::wlan_profiles().await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<WlanProfilesResponse>::error(format!(
                "Failed: {}",
                err
            ))),
        ),
    }
}

/// POST /api/device-network/wlan/forget
pub async fn forget_device_wlan_handler(
    Json(payload): Json<WlanForgetRequest>,
) -> impl IntoResponse {
    match crate::services::network::device_network::wlan_forget(payload).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "WLAN profile forgotten",
                data,
            )),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<WlanProfilesResponse>::error(format!(
                "Failed: {}",
                err
            ))),
        ),
    }
}

/// POST /api/device-network/wlan/connect
pub async fn connect_device_wlan_handler(
    State(app): State<AppState>,
    Json(payload): Json<WlanConnectRequest>,
) -> impl IntoResponse {
    let target_ssid = payload.ssid.clone();
    let previous = crate::services::network::device_network::wlan_status().await.ok();
    match crate::services::network::device_network::wlan_connect(payload).await {
        Ok(data) => {
            if data.connected {
                app.system_event_emitter
                    .emit_code(
                        system_event_codes::DEVICE_NETWORK_WLAN_CONNECTED,
                        system_event_severity::INFO,
                        system_event_status::SUCCEEDED,
                        data.ssid.clone().unwrap_or_else(|| target_ssid.clone()),
                        "WLAN 已连接",
                    )
                    .await;
                let previous_ssid = previous.and_then(|status| status.ssid);
                if previous_ssid.is_some() && previous_ssid != data.ssid && data.ssid.is_some() {
                    app.system_event_emitter
                        .emit_code(
                            system_event_codes::DEVICE_NETWORK_WLAN_SSID_CHANGED,
                            system_event_severity::INFO,
                            system_event_status::CHANGED,
                            data.ssid.clone().unwrap_or_default(),
                            "WLAN SSID 已变化",
                        )
                        .await;
                }
            }
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message("WLAN connected", data)),
            )
        }
        Err(err) => {
            app.system_event_emitter
                .emit_code(
                    system_event_codes::DEVICE_NETWORK_WLAN_CONNECT_FAILED,
                    system_event_severity::WARNING,
                    system_event_status::FAILED,
                    target_ssid,
                    format!("WLAN 连接失败: {err}"),
                )
                .await;
            (
                StatusCode::OK,
                Json(ApiResponse::<WlanStatusResponse>::error(format!(
                    "Failed: {}",
                    err
                ))),
            )
        }
    }
}

/// POST /api/device-network/wlan/disconnect
pub async fn disconnect_device_wlan_handler(State(app): State<AppState>) -> impl IntoResponse {
    let previous = crate::services::network::device_network::wlan_status().await.ok();
    match crate::services::network::device_network::wlan_disconnect().await {
        Ok(data) => {
            if previous
                .as_ref()
                .map(|status| status.connected)
                .unwrap_or(false)
                && !data.connected
            {
                app.system_event_emitter
                    .emit_code(
                        system_event_codes::DEVICE_NETWORK_WLAN_DISCONNECTED,
                        system_event_severity::INFO,
                        system_event_status::CHANGED,
                        previous.and_then(|status| status.ssid).unwrap_or_default(),
                        "WLAN 已断开",
                    )
                    .await;
            }
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message("WLAN disconnected", data)),
            )
        }
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<WlanStatusResponse>::error(format!(
                "Failed: {}",
                err
            ))),
        ),
    }
}

/// POST /api/device-network/wlan/profile
pub async fn save_device_wlan_profile_handler(
    Json(payload): Json<WlanProfileRequest>,
) -> impl IntoResponse {
    match crate::services::network::device_network::wlan_save_profile(payload).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "WLAN profile updated",
                data,
            )),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<WlanStatusResponse>::error(format!(
                "Failed: {}",
                err
            ))),
        ),
    }
}

/// GET /api/network/signal-strength
pub async fn get_signal_strength_handler(State(conn): State<Arc<Connection>>) -> impl IntoResponse {
    match get_signal_strength(&conn).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<SignalStrengthResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

// ============ 数据连接 ============

/// GET /api/data
pub async fn get_data_status(State(app): State<AppState>) -> impl IntoResponse {
    if app.data_user_disabled.load(Ordering::SeqCst) {
        return (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Success",
                DataConnectionResponse { active: false },
            )),
        );
    }

    match get_data_connection_status(&app.dbus_conn).await {
        Ok(active) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Success",
                DataConnectionResponse { active },
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<DataConnectionResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// POST /api/data
pub async fn set_data_status(
    State(app): State<AppState>,
    Json(payload): Json<DataConnectionRequest>,
) -> impl IntoResponse {
    let previous_active = !app.data_user_disabled.load(Ordering::SeqCst);
    let allow_roaming = app.config_manager.get_roaming_allowed();
    let apn_config = app.config_manager.get_apn_config();
    match set_data_connection_with_apn(
        &app.dbus_conn,
        payload.active,
        allow_roaming,
        Some(&apn_config),
    )
    .await
    {
        Ok(_) => {
            if let Err(err) = app.config_manager.set_data_enabled(payload.active) {
                return (
                    StatusCode::OK,
                    Json(ApiResponse::<DataConnectionResponse>::error(format!(
                        "Failed to save data switch state: {}",
                        err
                    ))),
                );
            }
            app.data_user_disabled
                .store(!payload.active, Ordering::SeqCst);
            if previous_active != payload.active {
                app.system_event_emitter
                    .emit_code(
                        system_event_codes::CELLULAR_DATA_ENABLED_CHANGED,
                        system_event_severity::INFO,
                        system_event_status::CHANGED,
                        "cellular_data",
                        if payload.active {
                            "蜂窝数据开关已开启"
                        } else {
                            "蜂窝数据开关已关闭"
                        },
                    )
                    .await;
            }
            // 同步 NM autoconnect 状态，防止用户关闭数据后 NM 自动重连
            tokio::spawn(async move {
                if let Ok(profile) = find_nm_modem_connection_pub().await {
                    let _ = nm_set_autoconnect_pub(&profile, payload.active).await;
                }
            });
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Data connection updated",
                    DataConnectionResponse {
                        active: payload.active,
                    },
                )),
            )
        }
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<DataConnectionResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

pub async fn restart_baseband_handler(State(app): State<AppState>) -> impl IntoResponse {
    let auto_connect_data = !app.data_user_disabled.load(Ordering::SeqCst);
    let allow_roaming = app.config_manager.get_roaming_allowed();
    let apn_config = app.config_manager.get_apn_config();
    match restart_baseband(
        &app.dbus_conn,
        auto_connect_data,
        allow_roaming,
        Some(apn_config),
    )
    .await
    {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Baseband restarted",
                data,
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<BasebandRestartResponse>::error(format!(
                "重启基带失败：{e}",
            ))),
        ),
    }
}

pub async fn get_baseband_restart_status_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            get_baseband_restart_progress(),
        )),
    )
}

async fn build_line_network_controls(
    app: &AppState,
    line: &Arc<crate::services::line_registry::LineRuntime>,
) -> LineNetworkControlsResponse {
    let binding = line.binding();
    let profile = app.config_manager.get_line_profile(&binding.line_id);
    let connected = if binding.present {
        line.secondary_data.interface().await.is_some()
            || modem_manager::data_interface_for_modem(app.dbus_conn.as_ref(), &binding.modem_path)
                .await
                .unwrap_or(None)
                .is_some()
    } else {
        false
    };
    let observed_airplane = if binding.present {
        modem_manager::get_airplane_mode_for_modem(app.dbus_conn.as_ref(), &binding.modem_path)
            .await
            .unwrap_or(AirplaneModeResponse {
                enabled: false,
                powered: false,
                online: false,
            })
    } else {
        AirplaneModeResponse {
            enabled: false,
            powered: false,
            online: false,
        }
    };
    let airplane_mode_requested = profile.airplane_mode_enabled;
    let airplane_phase = match (airplane_mode_requested, observed_airplane.enabled) {
        (true, true) => "enabled",
        (true, false) => "enabling",
        (false, true) => "disabling",
        (false, false) => "disabled",
    };
    let airplane_stage = match airplane_phase {
        "enabled" => "移动射频已关闭",
        "enabling" => "正在关闭移动射频",
        "disabling" => "正在恢复移动射频",
        _ => "移动射频正常",
    };
    let mut airplane_mode = observed_airplane;
    airplane_mode.enabled |= airplane_mode_requested;
    let is_roaming = if binding.present {
        get_is_roaming_for_modem(app.dbus_conn.as_ref(), &binding.modem_path)
            .await
            .unwrap_or(false)
    } else {
        false
    };
    let mut proxy_status = line.data_proxy.status().await;
    if profile.data_connection_enabled && !proxy_status.running && proxy_status.phase == "disabled"
    {
        proxy_status.phase = "connecting".to_string();
        proxy_status.stage = if connected {
            "移动数据已连接，正在启动代理监听"
        } else {
            "正在建立移动数据连接"
        }
        .to_string();
    }
    LineNetworkControlsResponse {
        line_id: binding.line_id,
        slot_label: format!("{} · 卡槽 {}", binding.slot_label, binding.uim_slot),
        modem_path: binding.modem_path,
        present: binding.present,
        data: LineDataConnectionResponse {
            enabled: profile.data_connection_enabled,
            connected,
            password_set: !profile.data_proxy.password.is_empty(),
            config: profile.data_proxy.redacted(),
            proxy: proxy_status,
        },
        roaming: RoamingResponse {
            roaming_allowed: profile.roaming_allowed,
            is_roaming,
        },
        airplane_mode,
        airplane_mode_requested,
        airplane_phase: airplane_phase.to_string(),
        airplane_stage: airplane_stage.to_string(),
    }
}

pub async fn get_line_network_controls_handler(
    State(app): State<AppState>,
) -> (
    StatusCode,
    Json<ApiResponse<Vec<LineNetworkControlsResponse>>>,
) {
    if let Err(error) = app.line_registry.refresh(app.dbus_conn.as_ref()).await {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error(format!(
                "Failed to discover modems: {error}"
            ))),
        );
    }
    let lines = app.line_registry.all().await;
    let mut response = Vec::with_capacity(lines.len());
    for line in lines {
        response.push(build_line_network_controls(&app, &line).await);
    }
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", response)),
    )
}

/// POST /api/modem/lines/{line_id}/data/traffic/reset
///
/// Zero one line's proxied-traffic counters, in memory and on disk. Useful at
/// the start of a billing cycle; other lines are untouched.
pub async fn reset_line_data_traffic_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<LineNetworkControlsResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    app.line_registry.reset_data_traffic(&line_id).await;
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            build_line_network_controls(&app, &line).await,
        )),
    )
}

pub(crate) async fn start_line_data_runtime(
    app: &AppState,
    line: &Arc<crate::services::line_registry::LineRuntime>,
    profile: &LineProfileConfig,
) -> Result<(), String> {
    let _guard = line.bearer_operation_lock.lock().await;
    start_line_data_runtime_locked(app, line, profile).await
}

async fn start_line_data_runtime_locked(
    app: &AppState,
    line: &Arc<crate::services::line_registry::LineRuntime>,
    profile: &LineProfileConfig,
) -> Result<(), String> {
    let binding = line.binding();
    if get_is_roaming_for_modem(app.dbus_conn.as_ref(), &binding.modem_path)
        .await
        .unwrap_or(false)
        && !profile.roaming_allowed
    {
        return Err("cellular_data_roaming_forbidden".to_string());
    }

    // A DATA6 session is the dedicated user-data bearer. Prefer it whenever it
    // is already alive, especially when IMS was restored first and qmi0 now
    // exposes an IMS bearer to ModemManager. Reusing this interface keeps the
    // proxy on the data path and avoids allocating a duplicate WDS session.
    if let Some(interface) = line.secondary_data.interface().await {
        line.data_proxy
            .start(&interface, &profile.data_proxy)
            .await?;
        return Ok(());
    }

    // Preserve an ordinary-data bearer that is already active on qmi0. Beta8
    // moves IMS to DATA6 in this case; replacing the data bearer would discard
    // the observed slot state before the VoLTE allocator can act on it.
    if let Some(interface) =
        modem_manager::data_interface_for_modem(app.dbus_conn.as_ref(), &binding.modem_path)
            .await
            .map_err(|error| error.to_string())?
    {
        line.data_proxy
            .start(&interface, &profile.data_proxy)
            .await?;
        return Ok(());
    }

    let configured_apn = app.config_manager.get_line_apn_config(&binding.line_id);
    let apn = modem_manager::resolve_data_apn_config(
        app.dbus_conn.as_ref(),
        &binding.modem_path,
        Some(&configured_apn),
    )
    .await;

    if let Some(qmi_device) = binding.qmi_device.as_deref() {
        match line
            .secondary_data
            .start(&binding.modem_id, qmi_device, &apn)
            .await
        {
            Ok(interface) => {
                line.data_proxy
                    .start(&interface, &profile.data_proxy)
                    .await?;
                return Ok(());
            }
            Err(error) if profile.volte_connection_enabled => return Err(error),
            Err(error) => warn!(
                line_id = %binding.line_id,
                error = %error,
                "Secondary DATA unavailable; VoLTE is disabled so ModemManager fallback is allowed"
            ),
        }
    } else if profile.volte_connection_enabled {
        return Err("cellular_secondary_qmi_device_unavailable".to_string());
    }

    // Single-slot fallback. This is intentionally forbidden above while VoLTE
    // is enabled because a normal MM bearer deactivates the IMS bearer on this
    // firmware.
    modem_manager::connect_data_via_modem(
        app.dbus_conn.as_ref(),
        &binding.modem_path,
        profile.roaming_allowed,
        Some(&apn),
    )
    .await?;
    let mut interface = None;
    for _ in 0..15 {
        interface =
            modem_manager::data_interface_for_modem(app.dbus_conn.as_ref(), &binding.modem_path)
                .await
                .unwrap_or(None);
        if interface.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let interface = interface.ok_or_else(|| "cellular_data_interface_unavailable".to_string())?;
    line.data_proxy
        .start(&interface, &profile.data_proxy)
        .await?;
    Ok(())
}

pub(crate) async fn stop_line_data_runtime(
    app: &AppState,
    line: &Arc<crate::services::line_registry::LineRuntime>,
) {
    let _guard = line.bearer_operation_lock.lock().await;
    stop_line_data_runtime_locked(app, line).await;
}

async fn stop_line_data_runtime_locked(
    app: &AppState,
    line: &Arc<crate::services::line_registry::LineRuntime>,
) {
    let binding = line.binding();
    line.data_proxy.stop().await;
    let had_secondary = line.secondary_data.interface().await.is_some();
    line.secondary_data.stop().await;
    if !had_secondary {
        if let Err(error) =
            modem_manager::disconnect_data_via_modem(app.dbus_conn.as_ref(), &binding.modem_path)
                .await
        {
            warn!(line_id = %binding.line_id, error = %error, "Per-line data disconnect failed");
        }
    }
}

/// Prepare beta8's dynamic allocation. Existing qmi0 data is preserved and IMS
/// moves to DATA6; otherwise ordinary data is prepared on DATA6 and IMS stays on
/// qmi0. The caller must hold `bearer_operation_lock` through IMS activation.
async fn prepare_line_data_slot_for_volte(
    app: &AppState,
    line: &Arc<crate::services::line_registry::LineRuntime>,
    profile: &LineProfileConfig,
) -> Result<
    crate::connectivity::modems::softstack::volte::data_slot::DataSlotMode,
    crate::connectivity::modems::softstack::volte::VolteError,
> {
    use crate::connectivity::modems::softstack::volte::data_slot::{
        select_data_slot_mode, DataSlotInputs, DataSlotMode,
    };

    if !profile.data_connection_enabled {
        return Ok(DataSlotMode::PrimaryImsOnly);
    }

    let binding = line.binding();
    let primary_data_interface =
        modem_manager::data_interface_for_modem(app.dbus_conn.as_ref(), &binding.modem_path)
            .await
            .unwrap_or(None);
    let primary_data_active = primary_data_interface.is_some();
    let mut secondary_data_active = line.secondary_data.interface().await.is_some();

    if !primary_data_active {
        let data_start_error = start_line_data_runtime_locked(app, line, profile)
            .await
            .err();
        secondary_data_active = line.secondary_data.interface().await.is_some();
        if let Some(error) = data_start_error {
            line.data_proxy.record_error(error.clone()).await;
            if secondary_data_active {
                // The DATA6 bearer can be healthy even when the local proxy
                // listener fails. Keep the real allocation in that case.
                warn!(line_id = %binding.line_id, error = %error, "DATA6 is active but its local proxy is unavailable");
            } else {
                warn!(line_id = %binding.line_id, error = %error, "DATA6 preparation failed; VoLTE allocation will use the observed slot state");
            }
        }
    } else if let Some(interface) = primary_data_interface.as_deref() {
        if let Err(error) = line.data_proxy.start(interface, &profile.data_proxy).await {
            line.data_proxy.record_error(error.clone()).await;
            warn!(line_id = %binding.line_id, error = %error, "Primary data is active but its local proxy is unavailable");
        }
    }

    let inputs = DataSlotInputs {
        data_requested: true,
        primary_data_active,
        secondary_data_active,
        secondary_endpoint_available: secondary_data_active || binding.qmi_device.is_some(),
    };
    match select_data_slot_mode(inputs) {
        Ok(mode) => {
            info!(line_id = %binding.line_id, mode = mode.as_str(), allocation = mode.allocation_message(), "VoLTE/data slot allocation selected");
            Ok(mode)
        }
        Err(error) => {
            line.data_proxy.record_error(error.to_string()).await;
            warn!(line_id = %binding.line_id, error = %error, "VoLTE/data slot allocation failed");
            Err(error)
        }
    }
}

pub async fn set_line_data_connection_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<LineNetworkToggleRequest>,
) -> (StatusCode, Json<ApiResponse<LineNetworkControlsResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    let binding = line.binding();
    if !binding.present {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("line_not_present")),
        );
    }
    let profile = app.config_manager.get_line_profile(&line_id);
    if payload.enabled && profile.airplane_mode_enabled {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("line_airplane_mode_enabled")),
        );
    }

    if payload.enabled {
        if !profile.data_connection_enabled {
            // Each explicit disabled -> enabled transition starts a new usage
            // session. Clear both the in-memory counters and persisted baseline
            // before accepting clients on the new listener.
            app.line_registry.reset_data_traffic(&line_id).await;
        }
        if let Err(error) = start_line_data_runtime(&app, &line, &profile).await {
            line.data_proxy.record_error(error.clone()).await;
            return (
                StatusCode::OK,
                Json(ApiResponse::error(format!("Failed: {error}"))),
            );
        }
    } else {
        stop_line_data_runtime(&app, &line).await;
    }
    if let Err(error) = app
        .config_manager
        .set_line_data_connection_enabled(&line_id, payload.enabled)
    {
        return (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {error}"))),
        );
    }
    let is_primary = app
        .line_registry
        .primary()
        .await
        .is_some_and(|primary| primary.binding().line_id == line_id);
    if is_primary {
        app.data_user_disabled
            .store(!payload.enabled, Ordering::SeqCst);
        let _ = app.config_manager.set_data_enabled(payload.enabled);
    }
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            build_line_network_controls(&app, &line).await,
        )),
    )
}

pub async fn set_line_data_proxy_config_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(mut payload): Json<LineDataProxyConfig>,
) -> (StatusCode, Json<ApiResponse<LineNetworkControlsResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    let current = app.config_manager.get_line_profile(&line_id).data_proxy;
    payload.username = payload.username.trim().to_string();
    if !payload.username.is_empty()
        && payload.password.is_empty()
        && payload.username == current.username
        && !current.password.is_empty()
    {
        // An empty password from the redacted edit form means “keep saved”.
        payload.password = current.password;
    }
    let profile = match app
        .config_manager
        .set_line_data_proxy_config(&line_id, payload)
    {
        Ok(profile) => profile,
        Err(error) => {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse::error(format!("Failed: {error}"))),
            )
        }
    };
    if profile.data_connection_enabled {
        let binding = line.binding();
        if !binding.present {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse::error("line_not_present")),
            );
        }
        if let Err(error) = start_line_data_runtime(&app, &line, &profile).await {
            line.data_proxy.record_error(error.clone()).await;
            return (
                StatusCode::OK,
                Json(ApiResponse::error(format!("Failed: {error}"))),
            );
        }
    }
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            build_line_network_controls(&app, &line).await,
        )),
    )
}

pub async fn set_line_roaming_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<RoamingRequest>,
) -> (StatusCode, Json<ApiResponse<LineNetworkControlsResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    let profile = match app
        .config_manager
        .set_line_roaming_allowed(&line_id, payload.allowed)
    {
        Ok(profile) => profile,
        Err(error) => {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse::error(format!("Failed: {error}"))),
            )
        }
    };
    let mut data_error = None;
    if profile.data_connection_enabled {
        let binding = line.binding();
        if !binding.present {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse::error("line_not_present")),
            );
        }
        stop_line_data_runtime(&app, &line).await;
        if let Err(error) = start_line_data_runtime(&app, &line, &profile).await {
            line.data_proxy.record_error(error.clone()).await;
            data_error = Some(error);
        }
    }
    if profile.enabled && profile.volte_connection_enabled {
        let status = line.volte.status().await;
        if status.registered {
            let _bearer_guard = line.bearer_operation_lock.lock().await;
            let _guard = line.volte_connect_lock.lock().await;
            crate::connectivity::modems::softstack::volte::live::disconnect_live_for_line(
                &line.volte_live,
                &line.volte,
                "line_roaming_policy_changed",
            )
            .await;
        }
        if !line.volte_retry_in_progress() {
            start_line_volte_restore(app.clone(), Arc::clone(&line), "roaming_policy_changed")
                .await;
        }
    }
    if let Some(error) = data_error {
        return (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {error}"))),
        );
    }
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            build_line_network_controls(&app, &line).await,
        )),
    )
}

pub async fn set_line_airplane_mode_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<LineNetworkToggleRequest>,
) -> (StatusCode, Json<ApiResponse<LineNetworkControlsResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    let binding = line.binding();
    if !binding.present && payload.enabled {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("line_not_present")),
        );
    }
    let is_primary = app
        .line_registry
        .all()
        .await
        .first()
        .is_some_and(|primary| primary.binding().line_id == line_id);

    if payload.enabled {
        match app.config_manager.set_line_airplane_mode(&line_id, true) {
            Ok(_) => {}
            Err(error) => {
                return (
                    StatusCode::OK,
                    Json(ApiResponse::error(format!("Failed: {error}"))),
                )
            }
        };
        let _bearer_guard = line.bearer_operation_lock.lock().await;
        stop_line_data_runtime_locked(&app, &line).await;
        let _volte_guard = line.volte_connect_lock.lock().await;
        crate::connectivity::modems::softstack::volte::live::disconnect_live_for_line(
            &line.volte_live,
            &line.volte,
            "line_airplane_mode_enabled",
        )
        .await;
        if is_primary {
            let _ = app.config_manager.set_data_enabled(false);
            app.data_user_disabled.store(true, Ordering::SeqCst);
        }
    } else if let Err(error) = app.config_manager.set_line_airplane_mode(&line_id, false) {
        return (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {error}"))),
        );
    }
    if let Err(error) = modem_manager::set_airplane_mode_for_modem(
        app.dbus_conn.as_ref(),
        &binding.modem_path,
        payload.enabled,
    )
    .await
    {
        return (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {error}"))),
        );
    }
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            build_line_network_controls(&app, &line).await,
        )),
    )
}

// ============ 短信功能 ============

use crate::platform::db::{Database, EsimProfileCacheEntry};

fn schedule_sms_db_maintenance(app: &AppState, deleted: usize) {
    if deleted < SMS_DB_MAINTENANCE_DELETE_THRESHOLD {
        return;
    }

    if app.sms_db_maintenance_pending.swap(true, Ordering::SeqCst) {
        info!(
            deleted,
            threshold = SMS_DB_MAINTENANCE_DELETE_THRESHOLD,
            "SMS database maintenance already scheduled"
        );
        return;
    }

    let db = Arc::clone(&app.database);
    let pending = Arc::clone(&app.sms_db_maintenance_pending);
    tokio::spawn(async move {
        info!(
            deleted,
            delay_secs = SMS_DB_MAINTENANCE_DELAY_SECS,
            "SMS database maintenance scheduled"
        );
        tokio::time::sleep(tokio::time::Duration::from_secs(
            SMS_DB_MAINTENANCE_DELAY_SECS,
        ))
        .await;

        let result = tokio::task::spawn_blocking(move || db.vacuum()).await;
        match result {
            Ok(Ok(())) => info!("SMS database maintenance completed"),
            Ok(Err(err)) => warn!(error = %err, "SMS database maintenance failed"),
            Err(err) => warn!(error = %err, "SMS database maintenance task failed"),
        }
        pending.store(false, Ordering::SeqCst);
    });
}

fn persist_vowifi_mt_deliveries(db: &Database, outcome: &MoSmsSipOutcome) -> Vec<SmsMessage> {
    if outcome.mt_deliveries.is_empty() {
        return Vec::new();
    }

    let mut groups: std::collections::BTreeMap<String, Vec<&MtSmsDeliver>> =
        std::collections::BTreeMap::new();
    for deliver in &outcome.mt_deliveries {
        groups
            .entry(vowifi_mt_delivery_group_key(deliver))
            .or_default()
            .push(deliver);
    }

    let mut inserted_messages = Vec::new();
    for (group_key, mut parts) in groups {
        parts.sort_by_key(|part| part.segment_sequence);
        let originator = parts
            .first()
            .map(|part| part.originator.as_str())
            .unwrap_or_default();
        let reference = parts
            .first()
            .and_then(|part| part.segment_reference)
            .or_else(|| {
                parts
                    .first()
                    .map(|part| u16::from(part.rp_message_reference))
            })
            .unwrap_or_default();
        let total = parts
            .iter()
            .map(|part| part.segment_total)
            .max()
            .unwrap_or(1)
            .max(1);
        let complete = (1..=total).all(|sequence| {
            parts
                .iter()
                .any(|part| part.segment_sequence == sequence && !part.text.is_empty())
        });
        let mut api_sms_id = None;
        let mut storage_key = group_key.clone();

        if complete {
            let mut text = String::new();
            for sequence in 1..=total {
                if let Some(part) = parts.iter().find(|part| part.segment_sequence == sequence) {
                    text.push_str(&part.text);
                }
            }
            storage_key = vowifi_mt_storage_key(outcome, originator, &text);
            let storage_marker = format!("vowifi-mt:{storage_key}");
            api_sms_id = db.sms_id_by_pdu(&storage_marker).unwrap_or(None);
            if api_sms_id.is_none() {
                let timestamp = crate::platform::db::beijing_sms_now_string();
                api_sms_id = db
                    .insert_sms_at_with_transport(
                        "incoming",
                        originator,
                        &text,
                        &timestamp,
                        "received",
                        Some(&storage_marker),
                        "vowifi_ims",
                    )
                    .ok();
                if let Some(id) = api_sms_id {
                    inserted_messages.push(SmsMessage {
                        id,
                        direction: "incoming".to_string(),
                        phone_number: originator.to_string(),
                        content: text.clone(),
                        timestamp,
                        status: "received".to_string(),
                        pdu: Some(storage_marker.clone()),
                        transport: "vowifi_ims".to_string(),
                        line_id: None,
                    });
                }
            }
        }
        let short_key = &storage_key[..std::cmp::min(16, storage_key.len())];
        let mt_message_id = format!("vowifi-mt-{short_key}");
        let mt_trace_id = format!("{}-mt-{short_key}", outcome.trace_id);

        let _ = db.upsert_vowifi_sms_delivery(NewVowifiSmsDelivery {
            message_id: &mt_message_id,
            trace_id: &mt_trace_id,
            direction: "mobile_terminated",
            state: if complete { "received" } else { "submitted" },
            sip_state: "accepted",
            rpdu_ack: "acked",
            delivery_reported: complete,
            failure_cause: None,
            retry_count: 0,
            api_sms_id,
        });

        for part in parts {
            let _ = db.upsert_vowifi_sms_part(NewVowifiSmsPart {
                message_id: &mt_message_id,
                reference: i64::from(reference),
                sequence: i64::from(part.segment_sequence),
                total: i64::from(total),
                received: true,
            });
        }
    }

    inserted_messages
}

fn spawn_vowifi_sms_followup_persist(
    app: AppState,
    mut followup: tokio::sync::mpsc::UnboundedReceiver<
        crate::connectivity::modems::softstack::vowifi::live::LiveSmsFollowupFrame,
    >,
) {
    tokio::spawn(async move {
        while let Some(frame) = followup.recv().await {
            let mt_messages = persist_vowifi_mt_deliveries(&app.database, &frame.outcome);
            let mt_complete_count = vowifi_mt_complete_group_count(&frame.outcome);
            if !frame.outcome.mt_deliveries.is_empty() || mt_complete_count > 0 {
                info!(
                    trace_id = frame.outcome.trace_id.as_str(),
                    message_id = frame.outcome.message_id.as_str(),
                    mt_received_count = frame.outcome.mt_deliveries.len(),
                    mt_complete_count,
                    mt_inserted_count = mt_messages.len(),
                    "VoWiFi SMS follow-up deliveries persisted"
                );
            }
            for sms in mt_messages {
                let notification_sender = Arc::clone(&app.notification_sender);
                tokio::spawn(async move {
                    let _ = notification_sender.forward_sms(&sms).await;
                });
            }
        }
    });
}
fn vowifi_mt_complete_group_count(outcome: &MoSmsSipOutcome) -> usize {
    let mut groups: std::collections::BTreeMap<String, Vec<&MtSmsDeliver>> =
        std::collections::BTreeMap::new();
    for deliver in &outcome.mt_deliveries {
        groups
            .entry(vowifi_mt_delivery_group_key(deliver))
            .or_default()
            .push(deliver);
    }

    groups
        .values()
        .filter(|parts| {
            let total = parts
                .iter()
                .map(|part| part.segment_total)
                .max()
                .unwrap_or(1)
                .max(1);
            (1..=total).all(|sequence| {
                parts
                    .iter()
                    .any(|part| part.segment_sequence == sequence && !part.text.is_empty())
            })
        })
        .count()
}

fn vowifi_mt_delivery_group_key(deliver: &MtSmsDeliver) -> String {
    let logical_part = if let Some(reference) = deliver.segment_reference {
        format!("segment:{reference:04x}:{}", deliver.segment_total)
    } else {
        let text_hash = format!("{:x}", md5::compute(deliver.text.as_bytes()));
        format!("single:{}:{text_hash}", deliver.service_center_timestamp)
    };
    let material = format!("{}|{}", deliver.originator, logical_part);
    format!("{:x}", md5::compute(material.as_bytes()))
}

fn vowifi_mt_storage_key(outcome: &MoSmsSipOutcome, originator: &str, text: &str) -> String {
    let text_hash = format!("{:x}", md5::compute(text.as_bytes()));
    let material = format!("{}|{originator}|{text_hash}", outcome.message_id);
    format!("{:x}", md5::compute(material.as_bytes()))
}

fn new_vowifi_switch_token(reason: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("{reason}-{millis:x}")
}

// This is a persistence boundary whose fields intentionally mirror one restore
// snapshot. Named arguments are supplied from a small set of local call sites.
#[allow(clippy::too_many_arguments)]
fn persist_vowifi_restore_phase(
    app: &AppState,
    switch_token: &str,
    switch_phase: &'static str,
    phase_started_at: Instant,
    identity_ready: bool,
    sim_auth_ready: bool,
    degraded_reason: Option<&str>,
    retry_count: u8,
) {
    if let Err(err) =
        app.database
            .upsert_vowifi_esim_restore(crate::platform::db::NewVowifiEsimRestore {
                switch_token: Some(switch_token),
                switch_phase: Some(switch_phase),
                phase_ms: Some(phase_started_at.elapsed().as_millis().min(i64::MAX as u128) as i64),
                identity_ready,
                sim_auth_ready,
                degraded_reason,
                retry_count: i64::from(retry_count),
            })
    {
        warn!(error = %err, "Failed to persist VoWiFi eSIM restore phase");
    }
}

/// POST /api/sms/send
pub async fn send_sms_handler(
    State(app): State<AppState>,
    Json(payload): Json<SendSmsRequest>,
) -> impl IntoResponse {
    // Each line routes by its own policy (falling back to the global one), so a
    // VoLTE-only SIM and a VoWiFi-only SIM can coexist in one device.
    let scope = VowifiScope::resolve(&app, payload.line_id.as_deref()).await;
    let policy = if scope.is_bound() {
        app.config_manager.get_line_sms_path_policy(scope.line_id())
    } else {
        app.config_manager.get_sms_path_policy().normalized()
    };
    let stop_on_failure = policy.mid_flight_disable == MidFlightDisablePolicy::Fail;
    let mut failures = Vec::new();
    for path in policy.enabled_layers() {
        let result = match path {
            AccessPathKind::Vowifi => send_sms_over_vowifi_path(&app, &scope, &payload).await,
            AccessPathKind::Volte => send_sms_over_volte_path(&app, &payload).await,
            AccessPathKind::Cs => send_sms_over_cs_path(&app, &payload).await,
        };
        match result {
            Ok(data) => {
                return (
                    StatusCode::OK,
                    Json(ApiResponse::success_with_message("SMS sent", data)),
                );
            }
            Err(reason) => {
                failures.push(format!("{}:{reason}", path.as_str()));
                if stop_on_failure {
                    break;
                }
                warn!(path = path.as_str(), reason = %reason, "SMS path failed; trying next path");
            }
        }
    }
    let detail = if failures.is_empty() {
        "no enabled SMS path".to_string()
    } else {
        failures.join("; ")
    };
    (
        StatusCode::OK,
        Json(ApiResponse::<serde_json::Value>::error(format!(
            "Failed to send SMS: {}",
            detail
        ))),
    )
}

async fn send_sms_over_vowifi_path(
    app: &AppState,
    scope: &VowifiScope,
    payload: &SendSmsRequest,
) -> Result<serde_json::Value, String> {
    let control = app.config_manager.get_vowifi_config();
    if !control.feature_enabled {
        return Err("disabled".to_string());
    }
    if !scope.is_bound() {
        return Err("vowifi_line_not_available".to_string());
    }
    if !app
        .config_manager
        .get_line_profile(scope.line_id())
        .vowifi
        .enabled
    {
        return Err("disabled".to_string());
    }
    let mut ready = scope.runtime().snapshot().await.readiness().sms_ready;
    if !ready {
        let airplane_enabled = scope.airplane_mode_enabled(app).await;
        if airplane_enabled {
            scope
                .runtime()
                .refresh_identity_with_timeout(
                    &app.dbus_conn,
                    std::time::Duration::from_secs(VOWIFI_SIM_IDENTITY_TIMEOUT_SECS),
                )
                .await;
            ready = scope
                .runtime()
                .connect_live_with_stage_timeout(
                    Some(&app.database),
                    std::time::Duration::from_secs(VOWIFI_LIVE_STAGE_TIMEOUT_SECS),
                )
                .await
                .readiness()
                .sms_ready;
        } else {
            ready = connect_vowifi_on_line(
                app,
                scope,
                VOWIFI_MANUAL_CONNECT_ATTEMPTS,
                std::time::Duration::from_secs(VOWIFI_MANUAL_CONNECT_RETRY_DELAY_SECS),
                false,
            )
            .await
            .readiness
            .sms_ready;
        }
    }
    if !ready {
        return Err("sms_ready_not_reached".to_string());
    }
    let send_result =
        send_live_sms_over_ims_for_line(scope.line_id(), &payload.phone_number, &payload.content)
            .await
            .map_err(|error| error.reason)?;
    let outcome = send_result.outcome;
    let api_sms_id = app
        .database
        .insert_sms_with_transport(
            "outgoing",
            &payload.phone_number,
            &payload.content,
            outcome.api_status(),
            None,
            "vowifi_ims",
        )
        .ok();
    let _ = app
        .database
        .upsert_vowifi_sms_delivery(NewVowifiSmsDelivery {
            message_id: &outcome.message_id,
            trace_id: &outcome.trace_id,
            direction: "mobile_originated",
            state: outcome.delivery_state.as_str(),
            sip_state: if (200..300).contains(&outcome.sip_status) {
                "accepted"
            } else {
                "rejected"
            },
            rpdu_ack: outcome.rpdu_ack.as_str(),
            delivery_reported: false,
            failure_cause: outcome.failure_cause.as_deref(),
            retry_count: 0,
            api_sms_id,
        });
    spawn_vowifi_sms_followup_persist(app.clone(), send_result.followup);
    Ok(json!({
        "path": "vowifi_ims",
        "transport": "vowifi_ims",
        "message_id": outcome.message_id,
        "trace_id": outcome.trace_id,
        "delivery_state": outcome.delivery_state.as_str(),
        "rpdu_ack": outcome.rpdu_ack.as_str(),
        "mt_followup": "background",
    }))
}

async fn send_sms_over_volte_path(
    app: &AppState,
    payload: &SendSmsRequest,
) -> Result<serde_json::Value, String> {
    let config = app.config_manager.get_volte_config();
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let selected_line_id = match payload.line_id.as_deref() {
        Some(line_id) => line_id.to_string(),
        None => {
            let enabled = app
                .line_registry
                .all()
                .await
                .into_iter()
                .filter(|line| {
                    let binding = line.binding();
                    let profile = app.config_manager.get_line_profile(&binding.line_id);
                    binding.present && profile.enabled && profile.volte_connection_enabled
                })
                .collect::<Vec<_>>();
            if enabled.len() != 1 {
                return Err("volte_line_id_required".to_string());
            }
            enabled[0].binding().line_id
        }
    };
    let line_id = selected_line_id.as_str();
    let line = app
        .line_registry
        .get(line_id)
        .await
        .ok_or_else(|| "line_not_found".to_string())?;
    let binding = line.binding();
    if !binding.present {
        return Err("line_not_present".to_string());
    }
    let profile = app.config_manager.get_line_profile(line_id);
    if !profile.enabled || !profile.volte_connection_enabled {
        return Err("line_volte_connection_disabled".to_string());
    }
    if !line.volte.status().await.registered {
        let device = crate::connectivity::modems::softstack::volte::live::VolteDeviceBinding::from_modem(&binding)
            .map_err(|error| error.to_string())?;
        let _bearer_guard = line.bearer_operation_lock.lock().await;
        let data_slot_mode = prepare_line_data_slot_for_volte(app, &line, &profile)
            .await
            .map_err(|error| error.to_string())?;
        let _guard = line.volte_connect_lock.lock().await;
        if !line.volte.status().await.registered {
            crate::connectivity::modems::softstack::volte::live::connect_live_for_line(
                &line.volte_live,
                &device,
                &line.volte,
                &config,
                profile.volte_ip_families.as_deref(),
                profile.roaming_allowed,
                data_slot_mode,
                app.config_manager.get_sms_path_policy().dedupe_enabled,
                Arc::clone(&app.database),
                Arc::clone(&app.notification_sender),
            )
            .await
            .map_err(|error| error.to_string())?;
        }
    }
    let sim =
        get_sim_info_for_modem_with_cache(&app.dbus_conn, &binding.modem_path, Some(&app.database))
            .await
            .map_err(|error| error.to_string())?;
    let result = crate::connectivity::modems::softstack::volte::live::send_live_sms_for_line(
        &line.volte_live,
        &line.volte,
        &payload.phone_number,
        &payload.content,
        &sim.sms_center,
    )
    .await
    .map_err(|error| error.to_string())?;
    app.database
        .insert_sms_with_transport_for_line(
            "outgoing",
            &payload.phone_number,
            &payload.content,
            "sent",
            None,
            "volte_ims",
            Some(line_id),
        )
        .map_err(|error| error.to_string())?;
    Ok(json!({
            "path": "volte_ims",
            "transport": "volte_ims",
            "line_id": line_id,
            "message_id": result.message_id,
            "trace_id": result.trace_id,
            "part_count": result.part_count,
            "sip_statuses": result.sip_statuses,
    }))
}

async fn send_sms_over_cs_path(
    app: &AppState,
    payload: &SendSmsRequest,
) -> Result<serde_json::Value, String> {
    let (path, line_id) = if let Some(line_id) = payload.line_id.as_deref() {
        let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
        let line = app
            .line_registry
            .get(line_id)
            .await
            .ok_or_else(|| "line_not_found".to_string())?;
        let binding = line.binding();
        if !binding.present {
            return Err("line_not_present".to_string());
        }
        (
            send_sms_via_modem(
                &app.dbus_conn,
                &binding.modem_path,
                &payload.phone_number,
                &payload.content,
            )
            .await
            .map_err(|error| error.to_string())?,
            Some(line_id),
        )
    } else {
        (
            send_sms(&app.dbus_conn, &payload.phone_number, &payload.content)
                .await
                .map_err(|error| error.to_string())?,
            None,
        )
    };
    app.database
        .insert_sms_with_transport_for_line(
            "outgoing",
            &payload.phone_number,
            &payload.content,
            "sent",
            None,
            "modem",
            line_id,
        )
        .map_err(|error| error.to_string())?;
    Ok(json!({ "path": path, "transport": "modem", "line_id": line_id }))
}

/// Drain the voice call follow-up channel, logging dialog/media progress
/// (ringing, answer, hangup). Mirrors `spawn_vowifi_sms_followup_persist`.
fn spawn_vowifi_call_followup(
    mut followup: tokio::sync::mpsc::UnboundedReceiver<
        crate::connectivity::modems::softstack::vowifi::live::LiveCallFollowupFrame,
    >,
) {
    tokio::spawn(async move {
        while let Some(frame) = followup.recv().await {
            info!(
                trace_id = frame.outcome.trace_id.as_str(),
                call_id = frame.outcome.call_id.as_str(),
                call_state = frame.outcome.call_state.as_str(),
                invite_state = frame.outcome.invite_state.as_str(),
                negotiated_codec = frame
                    .outcome
                    .negotiated_codec
                    .map(|codec| codec.as_str())
                    .unwrap_or("none"),
                "VoWiFi voice call follow-up progress"
            );
        }
    });
}

/// POST /api/voice/call
///
/// Places a VoWiFi voice call over IMS. Requires the VoWiFi feature to be
/// enabled and the runtime to have reached `voice_ready`. Unlike SMS, there is
/// no modem fallback wired here yet: the carrier (AT + USB-Audio) leg is a
/// reserved interface and is not exposed until a media backend is attached.
pub async fn place_call_handler(
    State(app): State<AppState>,
    Json(payload): Json<PlaceCallRequest>,
) -> impl IntoResponse {
    let vowifi_control = app.config_manager.get_vowifi_config();
    let scope = VowifiScope::resolve(&app, payload.line_id.as_deref()).await;
    if !scope.is_bound() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<serde_json::Value>::error(
                "line_not_found".to_string(),
            )),
        );
    }
    // The global flag is only a kill switch; whether this SIM may call is the
    // line's own VoWiFi setting.
    let line_vowifi_enabled = app
        .config_manager
        .get_line_profile(scope.line_id())
        .vowifi
        .enabled;
    if !vowifi_control.feature_enabled || !line_vowifi_enabled {
        return (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(
                "VoWiFi is not enabled; voice calling is unavailable".to_string(),
            )),
        );
    }

    let mut voice_ready = scope.runtime().snapshot().await.readiness().voice_ready;
    if !voice_ready {
        // Attempt to bring the runtime up to voice readiness once.
        let snapshot = scope
            .runtime()
            .connect_live_with_stage_timeout(
                Some(&app.database),
                std::time::Duration::from_secs(VOWIFI_LIVE_STAGE_TIMEOUT_SECS),
            )
            .await;
        voice_ready = snapshot.readiness().voice_ready;
        if !voice_ready {
            return (
                StatusCode::OK,
                Json(ApiResponse::<serde_json::Value>::error(format!(
                    "Failed to place call over VoWiFi: {}",
                    snapshot
                        .degraded_reason
                        .as_deref()
                        .unwrap_or("voice_ready_not_reached")
                ))),
            );
        }
    }

    match place_live_voice_call_for_line(scope.line_id(), &payload.phone_number).await {
        Ok(call_result) => {
            let outcome = call_result.outcome;
            spawn_vowifi_call_followup(call_result.followup);
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Call placed",
                    json!({
                        "path": "vowifi_ims",
                        "transport": "vowifi_ims",
                        "call_id": outcome.call_id,
                        "trace_id": outcome.trace_id,
                        "call_state": outcome.call_state.as_str(),
                        "invite_state": outcome.invite_state.as_str(),
                        "negotiated_codec": outcome
                            .negotiated_codec
                            .map(|codec| codec.as_str()),
                        "sip_status": outcome.sip_status,
                        "media_followup": "background",
                    }),
                )),
            )
        }
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed to place call over VoWiFi: {}",
                err.reason
            ))),
        ),
    }
}

/// GET /api/sms/list
pub async fn get_sms_channels_handler(
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<Vec<SmsChannelResponse>>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let mut channels = Vec::new();
    for line in app.line_registry.all().await {
        let modem = line.binding();
        channels.push(SmsChannelResponse {
            id: modem.line_id.clone(),
            kind: "modem_line".to_string(),
            label: format!("{} · 卡槽 {}", modem.slot_label, modem.uim_slot),
            available: modem.present,
            uim_slot: modem.uim_slot,
            line_id: Some(modem.line_id),
            slot_id: None,
            iccid: (!modem.sim_iccid.trim().is_empty()).then_some(modem.sim_iccid),
            operator_id: (!modem.operator_id.trim().is_empty()).then_some(modem.operator_id),
        });
    }
    for slot in app.config_manager.get_standalone_sim_slots() {
        channels.push(SmsChannelResponse {
            id: format!("reader:{}", slot.id),
            kind: "standalone_sim_slot".to_string(),
            label: if slot.label.trim().is_empty() {
                format!("读卡器 · 卡槽 {}", slot.uim_slot)
            } else {
                format!("{} · 卡槽 {}", slot.label, slot.uim_slot)
            },
            available: slot.enabled,
            uim_slot: slot.uim_slot,
            line_id: None,
            slot_id: Some(slot.id),
            iccid: None,
            operator_id: None,
        });
    }
    if app.database.has_unassigned_sms().unwrap_or(false) {
        channels.push(SmsChannelResponse {
            id: "unassigned".to_string(),
            kind: "unassigned".to_string(),
            label: "历史未归属短信".to_string(),
            available: true,
            ..Default::default()
        });
    }
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", channels)),
    )
}

fn sms_channel_filter(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

pub async fn get_sms_list_handler(
    State(db): State<Arc<Database>>,
    Query(params): Query<SmsListRequest>,
) -> (StatusCode, Json<ApiResponse<SmsListResponse>>) {
    let limit = if params.limit > 0 { params.limit } else { 50 };
    let offset = if params.offset >= 0 { params.offset } else { 0 };
    let direction = params
        .direction
        .as_deref()
        .filter(|value| matches!(*value, "incoming" | "outgoing"));

    let channel_id = sms_channel_filter(params.channel_id.as_deref());
    match db.get_sms_messages_for_channel(limit, offset, direction, channel_id) {
        Ok(messages) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Success",
                SmsListResponse { messages },
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<SmsListResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// GET /api/sms/conversation
pub async fn get_sms_conversation_handler(
    State(db): State<Arc<Database>>,
    Query(params): Query<SmsConversationRequest>,
) -> (StatusCode, Json<ApiResponse<SmsListResponse>>) {
    let limit = if params.limit > 0 { params.limit } else { 50 };
    let channel_id = sms_channel_filter(params.channel_id.as_deref());
    match db.get_sms_conversation_for_channel(&params.phone_number, limit, channel_id) {
        Ok(messages) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Success",
                SmsListResponse { messages },
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<SmsListResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// GET /api/sms/stats
pub async fn get_sms_stats_handler(
    State(db): State<Arc<Database>>,
    Query(params): Query<SmsStatsRequest>,
) -> (StatusCode, Json<ApiResponse<SmsStatsResponse>>) {
    match db.get_sms_stats_for_channel(sms_channel_filter(params.channel_id.as_deref())) {
        Ok(stats) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", stats)),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<SmsStatsResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// POST /api/sms/clear
pub async fn clear_sms_handler(
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let deleted = app
        .database
        .get_sms_stats()
        .map(|stats| stats.total.max(0) as usize)
        .unwrap_or(SMS_DB_MAINTENANCE_DELETE_THRESHOLD);

    match app.database.clear_all_sms() {
        Ok(_) => {
            schedule_sms_db_maintenance(&app, deleted);
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "All SMS cleared",
                    json!({ "deleted": deleted }),
                )),
            )
        }
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", e))),
        ),
    }
}

/// DELETE /api/sms/message/{id}
pub async fn delete_sms_message_handler(
    State(db): State<Arc<Database>>,
    Path(id): Path<i64>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    match db.delete_sms(id) {
        Ok(deleted) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "SMS deleted",
                json!({ "deleted": deleted }),
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", e))),
        ),
    }
}

/// DELETE /api/sms/conversation/{phone_number}
pub async fn delete_sms_conversation_handler(
    State(app): State<AppState>,
    Path(phone_number): Path<String>,
    Query(params): Query<SmsStatsRequest>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    match app.database.delete_sms_conversation_for_channel(
        &phone_number,
        sms_channel_filter(params.channel_id.as_deref()),
    ) {
        Ok(deleted) => {
            schedule_sms_db_maintenance(&app, deleted);
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "SMS conversation deleted",
                    json!({ "deleted": deleted }),
                )),
            )
        }
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", e))),
        ),
    }
}

/// POST /api/sms/batch-delete
pub async fn delete_sms_batch_handler(
    State(app): State<AppState>,
    Json(payload): Json<SmsBatchDeleteRequest>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    if payload.ids.is_empty() && payload.phone_numbers.is_empty() {
        return (StatusCode::OK, Json(ApiResponse::error("No SMS selected")));
    }

    match app.database.delete_sms_batch_for_channel(
        &payload.ids,
        &payload.phone_numbers,
        sms_channel_filter(payload.channel_id.as_deref()),
    ) {
        Ok(deleted) => {
            schedule_sms_db_maintenance(&app, deleted);
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "SMS batch deleted",
                    json!({ "deleted": deleted }),
                )),
            )
        }
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", e))),
        ),
    }
}

// ============ 系统信息 ============

// 读取温度传感器数据
// ============ 电话功能 ============

async fn track_call_start(
    app: &AppState,
    path: &str,
    direction: &str,
    phone_number: &str,
    answered: bool,
) {
    if let Ok(id) = app.database.insert_call(direction, phone_number, answered) {
        let mut active = app.active_calls.lock().await;
        active.insert(
            path.to_string(),
            crate::state::ActiveCallRecord {
                id,
                answered_at: answered.then(std::time::Instant::now),
                answered,
            },
        );
    }
}

async fn mark_tracked_call_answered(app: &AppState, path: &str) {
    let mut active = app.active_calls.lock().await;
    if let Some(record) = active.get_mut(path) {
        record.answered = true;
        if record.answered_at.is_none() {
            record.answered_at = Some(std::time::Instant::now());
        }
    }
}

async fn finish_tracked_call(app: &AppState, path: &str, answered_now: bool) {
    let mut record = {
        let mut active = app.active_calls.lock().await;
        active.remove(path)
    };
    if let Some(ref mut record) = record {
        if answered_now && record.answered_at.is_none() {
            record.answered_at = Some(std::time::Instant::now());
        }
        let duration = record
            .answered_at
            .map(|at| at.elapsed().as_secs() as i64)
            .unwrap_or(0);
        let _ = app
            .database
            .update_call_end(record.id, duration, record.answered || answered_now);
    }
}

pub async fn get_calls_handler(State(app): State<AppState>) -> impl IntoResponse {
    match list_current_calls(&app.dbus_conn).await {
        Ok(data) => {
            for call in &data.calls {
                if matches!(call.state.as_str(), "active" | "held") {
                    mark_tracked_call_answered(&app, &call.path).await;
                }
            }
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message("Success", data)),
            )
        }
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<CallListResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

pub async fn dial_call_handler(
    State(app): State<AppState>,
    Json(payload): Json<MakeCallRequest>,
) -> impl IntoResponse {
    let phone_number = payload.phone_number.trim().to_string();
    if phone_number.is_empty() {
        return (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(
                "Phone number is required",
            )),
        );
    }
    let modem_path = match resolve_modem_path(&app, payload.line_id.as_deref()).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            )
        }
    };
    match make_call_on_modem(&app.dbus_conn, &modem_path, &phone_number).await {
        Ok(path) => {
            track_call_start(&app, &path, "outgoing", &phone_number, false).await;
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Call started",
                    json!({ "path": path }),
                )),
            )
        }
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed to dial: {}",
                e
            ))),
        ),
    }
}

pub async fn hangup_call_handler(
    State(app): State<AppState>,
    Json(payload): Json<HangupCallRequest>,
) -> impl IntoResponse {
    let before = get_call_by_path(&app.dbus_conn, &payload.path).await.ok();
    match hangup_call(&app.dbus_conn, &payload.path).await {
        Ok(()) => {
            let answered = before
                .as_ref()
                .map(|call| call.state == "active" || call.state == "held")
                .unwrap_or(false);
            finish_tracked_call(&app, &payload.path, answered).await;
            if let Some(call) = before {
                if call.direction == "incoming"
                    && matches!(call.state.as_str(), "incoming" | "waiting")
                {
                    let _ = app
                        .database
                        .insert_call("missed", &call.phone_number, false);
                }
            }
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message("Call hung up", json!({}))),
            )
        }
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed to hang up: {}",
                e
            ))),
        ),
    }
}

pub async fn hangup_all_calls_handler(State(app): State<AppState>) -> impl IntoResponse {
    let before = list_current_calls(&app.dbus_conn).await.ok();
    match hangup_all_calls(&app.dbus_conn).await {
        Ok(()) => {
            if let Some(list) = before {
                for call in list.calls {
                    let answered = call.state == "active" || call.state == "held";
                    finish_tracked_call(&app, &call.path, answered).await;
                    if call.direction == "incoming"
                        && matches!(call.state.as_str(), "incoming" | "waiting")
                    {
                        let _ = app
                            .database
                            .insert_call("missed", &call.phone_number, false);
                    }
                }
            }
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "All calls hung up",
                    json!({}),
                )),
            )
        }
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed to hang up calls: {}",
                e
            ))),
        ),
    }
}

pub async fn answer_call_handler(
    State(app): State<AppState>,
    Json(payload): Json<HangupCallRequest>,
) -> impl IntoResponse {
    let before = get_call_by_path(&app.dbus_conn, &payload.path).await.ok();
    match answer_call(&app.dbus_conn, &payload.path).await {
        Ok(()) => {
            if let Some(call) = before {
                track_call_start(&app, &payload.path, "incoming", &call.phone_number, true).await;
                mark_tracked_call_answered(&app, &payload.path).await;
            }
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Call answered",
                    json!({}),
                )),
            )
        }
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed to answer call: {}",
                e
            ))),
        ),
    }
}

pub async fn get_call_history_handler(
    State(db): State<Arc<Database>>,
    Query(params): Query<CallHistoryRequest>,
) -> (StatusCode, Json<ApiResponse<CallHistoryResponse>>) {
    let limit = if params.limit > 0 { params.limit } else { 50 };
    let offset = if params.offset >= 0 { params.offset } else { 0 };
    match (db.get_call_history(limit, offset), db.get_call_stats()) {
        (Ok(records), Ok(stats)) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Success",
                CallHistoryResponse { records, stats },
            )),
        ),
        (Err(e), _) | (_, Err(e)) => (
            StatusCode::OK,
            Json(ApiResponse::<CallHistoryResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

pub async fn delete_call_history_handler(
    State(db): State<Arc<Database>>,
    Path(id): Path<i64>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    match db.delete_call(id) {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Call record deleted",
                json!({}),
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", e))),
        ),
    }
}

pub async fn clear_call_history_handler(
    State(db): State<Arc<Database>>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    match db.clear_all_calls() {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Call history cleared",
                json!({}),
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", e))),
        ),
    }
}

pub async fn get_call_settings_handler(State(conn): State<Arc<Connection>>) -> impl IntoResponse {
    match get_call_settings(&conn).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<CallSettingsResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

pub async fn set_call_settings_handler(
    State(conn): State<Arc<Connection>>,
    Json(payload): Json<SetCallSettingRequest>,
) -> impl IntoResponse {
    if payload.property != "VoiceCallWaiting" {
        return (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(
                "Only VoiceCallWaiting is supported by ModemManager",
            )),
        );
    }
    let enabled = matches!(payload.value.as_str(), "enabled" | "on" | "true" | "1");
    match set_call_waiting(&conn, enabled).await {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Call setting updated",
                json!({}),
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed to update call setting: {}",
                e
            ))),
        ),
    }
}

pub async fn get_call_volume_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(ApiResponse::<CallVolumeResponse>::error(
            "Call volume control is not exposed by ModemManager on this backend",
        )),
    )
}

pub async fn set_call_volume_handler(
    Json(payload): Json<SetCallVolumeRequest>,
) -> impl IntoResponse {
    let _ = (
        payload.speaker_volume,
        payload.microphone_volume,
        payload.muted,
    );
    (
        StatusCode::OK,
        Json(ApiResponse::<CallVolumeResponse>::error(
            "Call volume control is not exposed by ModemManager on this backend",
        )),
    )
}

pub async fn get_call_forwarding_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(ApiResponse::<CallForwardingResponse>::error(
            "Call forwarding is not exposed by ModemManager on this backend",
        )),
    )
}

pub async fn set_call_forwarding_handler(
    Json(payload): Json<SetCallForwardingRequest>,
) -> impl IntoResponse {
    let _ = (payload.forward_type, payload.number, payload.timeout);
    (
        StatusCode::OK,
        Json(ApiResponse::<CallForwardingResponse>::error(
            "Call forwarding is not exposed by ModemManager on this backend",
        )),
    )
}

pub async fn get_ims_status_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(ApiResponse::<ImsStatusResponse>::error(
            "IMS status is not exposed by ModemManager on this backend",
        )),
    )
}

async fn current_vowifi_profile_match(app: &AppState) -> VowifiProfileMatchResponse {
    if !app.config_manager.get_vowifi_config().feature_enabled {
        return VowifiProfileMatchResponse::default();
    }
    let snapshot = app
        .vowifi_runtime
        .refresh_identity_with_timeout(
            &app.dbus_conn,
            std::time::Duration::from_secs(VOWIFI_SIM_IDENTITY_TIMEOUT_SECS),
        )
        .await;
    let mut response = snapshot.profile;
    if let (Some(profile), Some(epdg)) = (response.profile.as_ref(), response.epdg.as_mut()) {
        if let Some(external) = profile_store(&app)
            .list_as_external()
            .unwrap_or_default()
            .into_iter()
            .find(|item| {
                item.profile_id == profile.profile_id
                    || item.mcc == profile.mcc && item.mnc == profile.mnc
            })
        {
            epdg.host = external.epdg_host;
            epdg.port = external.epdg_port;
            epdg.ip_stack = external.ip_stack;
            epdg.apn = external.apn;
            epdg.dns_server = external.dns_server;
            epdg.source = "carrier_profile_database".to_string();
        }
    }
    if let Some(primary) = app.line_registry.primary().await {
        let line_config = app
            .config_manager
            .get_line_profile(&primary.binding().line_id)
            .vowifi;
        if let Some(epdg) = response.epdg.as_mut() {
            if !line_config.dns_server.is_empty() {
                epdg.dns_server = Some(line_config.dns_server);
            }
            epdg.route_kind = match line_config.proxy_mode {
                crate::platform::config::VowifiProxyMode::Direct => "direct",
                crate::platform::config::VowifiProxyMode::Socks5UdpAssociate => "socks5_udp_associate",
                crate::platform::config::VowifiProxyMode::UdpRelay => "udp_relay",
            }
            .to_string();
            epdg.proxy_endpoint_configured = !line_config.proxy_endpoint.is_empty();
        }
    }
    response
}

fn disabled_vowifi_status(reason: &str) -> VowifiStatusResponse {
    VowifiStatusResponse {
        degraded_reason: Some(reason.to_string()),
        ..Default::default()
    }
}
fn vowifi_restore_reason_is_soft_retry(reason: Option<&str>) -> bool {
    matches!(
        reason,
        Some("vowifi_connect_already_running" | "live_connect_already_running")
    )
}

/// Which line a VoWiFi request acts on.
///
/// Every line owns its own `VowifiRuntime`, TUN device, ePDG/DNS/proxy
/// overrides and IMS session caches, so connecting line B must never touch
/// line A. The legacy single-line endpoints (`/api/vowifi/...`) keep working by
/// resolving to the primary line. `AppState::vowifi_runtime` survives only as
/// the answer for "no line present at all", which keeps status endpoints
/// responsive during boot and hotplug instead of 404-ing.
#[derive(Clone)]
pub struct VowifiScope {
    line: Option<Arc<crate::services::line_registry::LineRuntime>>,
    runtime: Arc<crate::connectivity::modems::softstack::vowifi::runtime::VowifiRuntime>,
    line_id: String,
}

impl VowifiScope {
    async fn resolve(app: &AppState, line_id: Option<&str>) -> Self {
        let line = match line_id {
            Some(line_id) => app.line_registry.get(line_id).await,
            None => app.line_registry.primary().await,
        };
        match line {
            Some(line) => {
                let line_id = line.binding().line_id;
                let runtime = Arc::clone(&line.vowifi);
                Self {
                    line: Some(line),
                    runtime,
                    line_id,
                }
            }
            None => Self {
                line: None,
                runtime: Arc::clone(&app.vowifi_runtime),
                line_id: line_id.unwrap_or_default().to_string(),
            },
        }
    }

    async fn primary(app: &AppState) -> Self {
        Self::resolve(app, None).await
    }

    fn line_id(&self) -> &str {
        &self.line_id
    }

    fn runtime(&self) -> &Arc<crate::connectivity::modems::softstack::vowifi::runtime::VowifiRuntime> {
        &self.runtime
    }

    fn is_bound(&self) -> bool {
        self.line.is_some()
    }

    /// The modem this line owns, when the line is actually present. `None` means
    /// there is nothing to act on, which callers treat as a no-op rather than
    /// falling back to some other baseband.
    fn modem_path(&self) -> Option<String> {
        self.line.as_ref().map(|line| line.binding()).and_then(|b| {
            if b.present {
                Some(b.modem_path)
            } else {
                None
            }
        })
    }

    /// The connect guard is per line, so two lines can dial their ePDGs at the
    /// same time while each line still rejects a second concurrent connect.
    fn try_connect_lock(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.line
            .as_ref()
            .and_then(|line| line.vowifi_connect_lock.try_lock().ok())
    }

    async fn status(&self) -> VowifiStatusResponse {
        self.runtime.snapshot().await.status_response()
    }

    /// Whether *this line's* radio is off. Airplane mode is per line now, so a
    /// second SIM being powered down must not change how this line behaves.
    async fn airplane_mode_enabled(&self, app: &AppState) -> bool {
        let Some(modem_path) = self.modem_path() else {
            return false;
        };
        modem_manager::get_airplane_mode_for_modem(app.dbus_conn.as_ref(), &modem_path)
            .await
            .map(|state| state.enabled)
            .unwrap_or(false)
    }
}

async fn reset_vowifi_runtime(app: &AppState, reason: &str) -> VowifiStatusResponse {
    let scope = VowifiScope::primary(app).await;
    reset_vowifi_runtime_for_scope(app, &scope, reason).await
}

async fn reset_vowifi_runtime_for_scope(
    app: &AppState,
    scope: &VowifiScope,
    reason: &str,
) -> VowifiStatusResponse {
    if scope.is_bound() {
        clear_live_runtime_for_line(scope.line_id()).await;
    } else {
        clear_all_live_runtime().await;
    }
    // let _ = app.database.clear_vowifi_runtime_events();
    let snapshot = scope.runtime().reset_runtime(reason).await;
    let status = snapshot.status_response();
    persist_vowifi_runtime_snapshot(app, &status);
    status
}

async fn restore_cellular_and_reset_vowifi(
    app: &AppState,
    scope: &VowifiScope,
    reason: &str,
) -> VowifiStatusResponse {
    let current = scope.status().await;
    let profile_meta = current.profile.profile.as_ref();
    let profile_id = profile_meta.map(|p| p.profile_id);

    // 1. IPSEC Event: 发送 IKEv2 INFORMATIONAL 报文，拆除全部 ESP 安全关联并注销会话
    let _ = app
        .database
        .insert_vowifi_runtime_event(crate::platform::db::NewVowifiRuntimeEvent {
            trace_id: Some("runtime-stop"),
            level: "info",
            phase: "connection_stop",
            profile_id,
            event_type: "ike_teardown",
            detail_json: "{}",
        });

    if let Err(err) = ensure_line_radio_enabled(app, scope).await {
        warn!(error = %err, "Failed to re-enable the line radio while stopping WiFi Calling");
    }

    restore_cellular_data_after_vowifi(app, scope).await;

    // 2. SMS Event: 短信路径已释放，成功退回到蜂窝基站数据链路。
    let _ = app
        .database
        .insert_vowifi_runtime_event(crate::platform::db::NewVowifiRuntimeEvent {
            trace_id: Some("runtime-stop"),
            level: "info",
            phase: "connection_stop",
            profile_id,
            event_type: "sms_path_released",
            detail_json: "{}",
        });

    let status = reset_vowifi_runtime_for_scope(app, scope, reason).await;

    // 3. SYS Event: WiFi Calling 核心服务运行时已停止
    let _ = app
        .database
        .insert_vowifi_runtime_event(crate::platform::db::NewVowifiRuntimeEvent {
            trace_id: Some("runtime-stop"),
            level: "info",
            phase: "connection_stop",
            profile_id,
            event_type: "runtime_stop",
            detail_json: "{}",
        });

    status
}

async fn fallback_to_cellular_and_disable_vowifi_connection(
    app: &AppState,
    scope: &VowifiScope,
    reason: &str,
) -> VowifiStatusResponse {
    if let Err(err) = disable_vowifi_connection_for_scope(app, scope) {
        warn!(error = %err, "Failed to persist WiFi Calling connection disable after fallback");
    }
    restore_cellular_and_reset_vowifi(app, scope, reason).await
}

async fn stop_vowifi_and_restore_cellular(
    app: &AppState,
    scope: &VowifiScope,
    reason: &str,
) -> VowifiStatusResponse {
    let _ = disable_vowifi_connection_for_scope(app, scope);
    restore_cellular_and_reset_vowifi(app, scope, reason).await
}

/// Turn off the VoWiFi connection intent for one line. The legacy global flag is
/// only touched for the primary line so the old single-line UI keeps agreeing
/// with reality; secondary lines are purely per-line.
fn disable_vowifi_connection_for_scope(app: &AppState, scope: &VowifiScope) -> Result<(), String> {
    if scope.is_bound() {
        app.config_manager
            .set_line_vowifi_connection_enabled(scope.line_id(), false)
            .map(|_| ())
    } else {
        app.config_manager
            .set_vowifi_connection_enabled(false)
            .map(|_| ())
    }
}

fn spawn_vowifi_profile_switch_restore(app: AppState, switch_token: String) {
    let config = app.config_manager.get_vowifi_config();
    if !config.feature_enabled {
        return;
    }
    tokio::spawn(async move {
        run_vowifi_restore_workflow(app, VowifiRestoreWorkflow::profile_switch(switch_token)).await;
    });
}

#[derive(Clone)]
struct VowifiRestoreWorkflow {
    trigger: VowifiRestoreTrigger,
    initial_delay: Duration,
    attempts: u8,
    retry_delay: Duration,
    connect_attempts: u8,
    connect_retry_delay: Duration,
    start_reason: &'static str,
    disabled_reason: &'static str,
    fallback_reason: &'static str,
}

#[derive(Clone)]
enum VowifiRestoreTrigger {
    ProfileSwitch { switch_token: String },
    BootAutoRestore,
}

impl VowifiRestoreWorkflow {
    fn profile_switch(switch_token: String) -> Self {
        Self {
            trigger: VowifiRestoreTrigger::ProfileSwitch { switch_token },
            initial_delay: Duration::from_secs(VOWIFI_PROFILE_SWITCH_RESTORE_INITIAL_DELAY_SECS),
            attempts: VOWIFI_PROFILE_SWITCH_RESTORE_ATTEMPTS,
            retry_delay: Duration::from_secs(VOWIFI_PROFILE_SWITCH_RESTORE_RETRY_DELAY_SECS),
            connect_attempts: VOWIFI_PROFILE_SWITCH_CONNECT_ATTEMPTS,
            connect_retry_delay: Duration::from_secs(
                VOWIFI_PROFILE_SWITCH_CONNECT_RETRY_DELAY_SECS,
            ),
            start_reason: "vowifi_profile_switch_teardown",
            disabled_reason: "vowifi_profile_switch_connection_disabled",
            fallback_reason: "vowifi_profile_switch_restore_failed_cellular_fallback",
        }
    }

    fn boot_auto_restore(config: &VowifiConfig) -> Self {
        Self {
            trigger: VowifiRestoreTrigger::BootAutoRestore,
            initial_delay: Duration::from_secs(
                config.auto_restore_initial_delay_secs.clamp(30, 300),
            ),
            attempts: config.auto_restore_attempts.clamp(1, 5),
            retry_delay: Duration::from_secs(config.auto_restore_retry_delay_secs.clamp(10, 180)),
            connect_attempts: config.auto_restore_attempts.clamp(1, 5),
            connect_retry_delay: Duration::from_secs(
                config.auto_restore_retry_delay_secs.clamp(10, 180),
            ),
            start_reason: "vowifi_auto_restore_start",
            disabled_reason: "vowifi_auto_restore_connection_disabled",
            fallback_reason: "vowifi_auto_restore_failed_cellular_fallback",
        }
    }

    fn switch_token(&self) -> Option<&str> {
        match &self.trigger {
            VowifiRestoreTrigger::ProfileSwitch { switch_token } => Some(switch_token.as_str()),
            VowifiRestoreTrigger::BootAutoRestore => None,
        }
    }

    fn is_profile_switch(&self) -> bool {
        matches!(self.trigger, VowifiRestoreTrigger::ProfileSwitch { .. })
    }

    fn label(&self) -> &'static str {
        match self.trigger {
            VowifiRestoreTrigger::ProfileSwitch { .. } => "profile_switch",
            VowifiRestoreTrigger::BootAutoRestore => "boot_auto_restore",
        }
    }
}

async fn run_vowifi_restore_workflow(app: AppState, workflow: VowifiRestoreWorkflow) {
    // The restore workflow is a boot/profile-switch recovery for the primary
    // line; other lines are restored by their own per-line intent.
    let scope = VowifiScope::primary(&app).await;
    if workflow.is_profile_switch() {
        persist_optional_vowifi_restore_phase(
            &app,
            &workflow,
            RestorePhase::TeardownVowifi,
            Instant::now(),
            false,
            false,
            None,
            0,
        );
        let _ = reset_vowifi_runtime_for_scope(&app, &scope, workflow.start_reason).await;
    }

    let config = app.config_manager.get_vowifi_config();
    if !config.feature_enabled || !config.connection_enabled {
        persist_optional_vowifi_restore_phase(
            &app,
            &workflow,
            RestorePhase::Failed,
            Instant::now(),
            false,
            false,
            Some("vowifi_connection_disabled"),
            0,
        );
        let _ = stop_vowifi_and_restore_cellular(&app, &scope, workflow.disabled_reason).await;
        return;
    }

    persist_optional_vowifi_restore_phase(
        &app,
        &workflow,
        RestorePhase::CardResetSettling,
        Instant::now(),
        false,
        false,
        None,
        0,
    );
    tokio::time::sleep(workflow.initial_delay).await;

    let mut last_status = disabled_vowifi_status("vowifi_restore_not_attempted");
    let attempts = workflow.attempts.max(1);
    for attempt in 1..=attempts {
        let retry_count = attempt.saturating_sub(1);
        let identity_status =
            wait_for_vowifi_identity_gate(&app, &scope, Some(&workflow), retry_count).await;
        if !identity_status.readiness.identity_ready || !identity_status.readiness.profile_matched {
            last_status = identity_status;
            if attempt < attempts {
                schedule_vowifi_restore_retry(&app, &workflow, &last_status, attempt).await;
                continue;
            }
            break;
        }

        if let Err(status) =
            wait_for_vowifi_sim_auth_gate(&app, &scope, Some(&workflow), retry_count).await
        {
            last_status = status;
            if attempt < attempts {
                schedule_vowifi_restore_retry(&app, &workflow, &last_status, attempt).await;
                continue;
            }
            break;
        }

        let runtime_started_at = Instant::now();
        persist_optional_vowifi_restore_phase(
            &app,
            &workflow,
            RestorePhase::RuntimeRestore,
            runtime_started_at,
            true,
            true,
            None,
            retry_count,
        );
        last_status = connect_vowifi_with_attempts(
            &app,
            workflow.connect_attempts,
            workflow.connect_retry_delay,
            false,
        )
        .await;
        let readiness = &last_status.readiness;
        if readiness.sms_ready {
            persist_optional_vowifi_restore_phase(
                &app,
                &workflow,
                RestorePhase::SmsReady,
                runtime_started_at,
                readiness.identity_ready,
                readiness.sim_auth_ready,
                None,
                retry_count,
            );
            info!(
                trigger = workflow.label(),
                "WiFi Calling restore workflow completed"
            );
            return;
        }
        if attempt < attempts {
            schedule_vowifi_restore_retry(&app, &workflow, &last_status, attempt).await;
        }
    }

    if vowifi_restore_reason_is_soft_retry(last_status.degraded_reason.as_deref()) {
        info!(
            trigger = workflow.label(),
            reason = last_status.degraded_reason.as_deref().unwrap_or("unknown"),
            "WiFi Calling restore workflow left active connection attempt in charge"
        );
        return;
    }
    let readiness = &last_status.readiness;
    persist_optional_vowifi_restore_phase(
        &app,
        &workflow,
        RestorePhase::Failed,
        Instant::now(),
        readiness.identity_ready,
        readiness.sim_auth_ready,
        last_status.degraded_reason.as_deref(),
        attempts,
    );
    warn!(
        trigger = workflow.label(),
        reason = last_status.degraded_reason.as_deref().unwrap_or("unknown"),
        "WiFi Calling restore workflow failed after retries"
    );
    let _ =
        fallback_to_cellular_and_disable_vowifi_connection(&app, &scope, workflow.fallback_reason)
            .await;
}

async fn schedule_vowifi_restore_retry(
    app: &AppState,
    workflow: &VowifiRestoreWorkflow,
    last_status: &VowifiStatusResponse,
    retry_count: u8,
) {
    persist_optional_vowifi_restore_phase(
        app,
        workflow,
        RestorePhase::RetryScheduled,
        Instant::now(),
        last_status.readiness.identity_ready,
        last_status.readiness.sim_auth_ready,
        last_status.degraded_reason.as_deref(),
        retry_count,
    );
    tokio::time::sleep(workflow.retry_delay).await;
}

async fn wait_for_vowifi_identity_gate(
    app: &AppState,
    scope: &VowifiScope,
    workflow: Option<&VowifiRestoreWorkflow>,
    retry_count: u8,
) -> VowifiStatusResponse {
    let mut last_status = disabled_vowifi_status("identity_refresh_not_attempted");
    for gate_attempt in 1..=VOWIFI_RESTORE_IDENTITY_GATE_ATTEMPTS.max(1) {
        let phase_started_at = Instant::now();
        let snapshot = scope
            .runtime()
            .refresh_identity_with_timeout(
                &app.dbus_conn,
                Duration::from_secs(VOWIFI_SIM_IDENTITY_TIMEOUT_SECS),
            )
            .await;
        last_status = snapshot.status_response();
        persist_vowifi_runtime_snapshot(app, &last_status);
        let identity_ready = last_status.readiness.identity_ready;
        let profile_matched = last_status.readiness.profile_matched;
        let degraded_reason = if identity_ready && profile_matched {
            None
        } else if !identity_ready {
            Some("identity_refresh_not_ready")
        } else {
            Some("profile_not_matched")
        };
        if let Some(workflow) = workflow {
            persist_optional_vowifi_restore_phase(
                app,
                workflow,
                RestorePhase::IdentityRefresh,
                phase_started_at,
                identity_ready,
                false,
                degraded_reason,
                retry_count,
            );
        }
        if identity_ready && profile_matched {
            return last_status;
        }
        if gate_attempt < VOWIFI_RESTORE_IDENTITY_GATE_ATTEMPTS {
            tokio::time::sleep(Duration::from_secs(VOWIFI_RESTORE_IDENTITY_GATE_DELAY_SECS)).await;
        }
    }

    if last_status.degraded_reason.is_none() {
        last_status.degraded_reason = Some(if !last_status.readiness.identity_ready {
            "identity_refresh_not_ready".to_string()
        } else {
            "profile_not_matched".to_string()
        });
    }
    persist_vowifi_runtime_snapshot(app, &last_status);
    last_status
}

async fn wait_for_vowifi_sim_auth_gate(
    app: &AppState,
    scope: &VowifiScope,
    workflow: Option<&VowifiRestoreWorkflow>,
    retry_count: u8,
) -> Result<(), VowifiStatusResponse> {
    let sim_auth_started_at = Instant::now();
    if let Some(workflow) = workflow {
        persist_optional_vowifi_restore_phase(
            app,
            workflow,
            RestorePhase::SimAuthGate,
            sim_auth_started_at,
            true,
            false,
            None,
            retry_count,
        );
    }

    if let Err(err) = verify_live_sim_auth_access_for_line(scope.line_id()).await {
        let mut status = scope.status().await;
        status.degraded_reason = Some(err.reason);
        persist_vowifi_runtime_snapshot(app, &status);
        if let Some(workflow) = workflow {
            persist_optional_vowifi_restore_phase(
                app,
                workflow,
                RestorePhase::SimAuthGate,
                sim_auth_started_at,
                status.readiness.identity_ready,
                false,
                status.degraded_reason.as_deref(),
                retry_count,
            );
        }
        return Err(status);
    }

    if let Some(workflow) = workflow {
        persist_optional_vowifi_restore_phase(
            app,
            workflow,
            RestorePhase::SimAuthGate,
            sim_auth_started_at,
            true,
            true,
            None,
            retry_count,
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn persist_optional_vowifi_restore_phase(
    app: &AppState,
    workflow: &VowifiRestoreWorkflow,
    switch_phase: RestorePhase,
    phase_started_at: Instant,
    identity_ready: bool,
    sim_auth_ready: bool,
    degraded_reason: Option<&str>,
    retry_count: u8,
) {
    if let Some(switch_token) = workflow.switch_token() {
        persist_vowifi_restore_phase(
            app,
            switch_token,
            switch_phase.as_str(),
            phase_started_at,
            identity_ready,
            sim_auth_ready,
            degraded_reason,
            retry_count,
        );
    }
}

/// VoWiFi needs this line's modem powered on for SIM/AKA access, so make sure
/// the line is not sitting in airplane mode. Only ever turns airplane mode off,
/// and only for the line that is connecting — a second SIM keeps its own state.
async fn ensure_line_radio_enabled(app: &AppState, scope: &VowifiScope) -> Result<(), String> {
    let Some(modem_path) = scope.modem_path() else {
        return Ok(());
    };
    modem_manager::set_airplane_mode_for_modem(app.dbus_conn.as_ref(), &modem_path, false).await
}

async fn pause_cellular_data_for_vowifi(app: &AppState, scope: &VowifiScope) -> Result<(), String> {
    if let Err(err) = ensure_line_radio_enabled(app, scope).await {
        warn!(error = %err, "Failed to keep modem enabled for WiFi Calling SIM access");
    }
    if let Ok(profile) = find_nm_modem_connection_pub().await {
        if let Err(err) = nm_set_autoconnect_pub(&profile, false).await {
            warn!(error = %err, profile = %profile, "Failed to disable NM autoconnect before WiFi Calling");
        }
    }
    let Some(modem_path) = scope.modem_path() else {
        return Ok(());
    };
    if let Some(line) = app.line_registry.get(scope.line_id()).await {
        stop_line_data_runtime(app, &line).await;
        return Ok(());
    }
    modem_manager::disconnect_data_via_modem(app.dbus_conn.as_ref(), &modem_path)
        .await
        .map_err(|err| err.to_string())
}

async fn restore_cellular_data_after_vowifi(app: &AppState, scope: &VowifiScope) {
    // Whether cellular data comes back is this line's own setting, not a global
    // one: a second SIM that never had data enabled must not get it here.
    let line_profile = app.config_manager.get_line_profile(scope.line_id());
    let should_restore_data = line_profile.data_connection_enabled
        && !line_profile.airplane_mode_enabled
        && !app.data_user_disabled.load(Ordering::SeqCst);
    if let Ok(profile) = find_nm_modem_connection_pub().await {
        if let Err(err) = nm_set_autoconnect_pub(&profile, should_restore_data).await {
            warn!(error = %err, profile = %profile, "Failed to restore NM autoconnect after WiFi Calling");
        }
    }
    if !should_restore_data {
        return;
    }
    if let Some(line) = app.line_registry.get(scope.line_id()).await {
        if let Err(err) = start_line_data_runtime(app, &line, &line_profile).await {
            warn!(error = %err, "Failed to restore cellular data after WiFi Calling");
        }
    }
}

async fn attempt_vowifi_connect_once(
    app: &AppState,
    scope: &VowifiScope,
    refresh_identity: bool,
) -> VowifiStatusResponse {
    if refresh_identity {
        scope
            .runtime()
            .refresh_identity_with_timeout(
                &app.dbus_conn,
                std::time::Duration::from_secs(VOWIFI_SIM_IDENTITY_TIMEOUT_SECS),
            )
            .await;
    }
    let snapshot = scope
        .runtime()
        .connect_live_with_stage_timeout(
            Some(&app.database),
            std::time::Duration::from_secs(VOWIFI_LIVE_STAGE_TIMEOUT_SECS),
        )
        .await;
    let status = snapshot.status_response();
    persist_vowifi_runtime_snapshot(app, &status);
    status
}

async fn connect_vowifi_with_attempts(
    app: &AppState,
    attempts: u8,
    retry_delay: std::time::Duration,
    fallback_to_cellular_on_failure: bool,
) -> VowifiStatusResponse {
    let scope = VowifiScope::primary(app).await;
    connect_vowifi_on_line(
        app,
        &scope,
        attempts,
        retry_delay,
        fallback_to_cellular_on_failure,
    )
    .await
}

async fn connect_vowifi_on_line(
    app: &AppState,
    scope: &VowifiScope,
    attempts: u8,
    retry_delay: std::time::Duration,
    fallback_to_cellular_on_failure: bool,
) -> VowifiStatusResponse {
    let control = app.config_manager.get_vowifi_config();
    if !control.feature_enabled {
        return disabled_vowifi_status("vowifi_feature_disabled");
    }
    if !scope.is_bound() {
        return disabled_vowifi_status("vowifi_line_not_available");
    }
    {
        let line_id = scope.line_id().to_string();
        // The ePDG endpoint now comes solely from the resolved carrier profile
        // (auto-matched or pinned via `profile_id`); the line only carries DNS and
        // proxy overrides, so this is applied as-is.
        let line_config = app.config_manager.get_line_profile(&line_id).vowifi;
        if let Err(error) =
            crate::connectivity::modems::softstack::vowifi::live::configure_live_network_overrides(&line_id, &line_config)
        {
            let mut status = disabled_vowifi_status("vowifi_line_network_config_invalid");
            status.degraded_reason = Some(error);
            persist_vowifi_runtime_snapshot(app, &status);
            return status;
        }
    }

    let current = scope.status().await;
    if current.readiness.sms_ready {
        persist_vowifi_runtime_snapshot(app, &current);
        return current;
    }

    let Some(_connect_guard) = scope.try_connect_lock() else {
        let mut status = scope.status().await;
        if !status.readiness.sms_ready {
            status.degraded_reason = Some("vowifi_connect_already_running".to_string());
        }
        persist_vowifi_runtime_snapshot(app, &status);
        return status;
    };

    let current = scope.status().await;
    if current.readiness.sms_ready {
        persist_vowifi_runtime_snapshot(app, &current);
        return current;
    }

    // let _ = app.database.clear_vowifi_runtime_events();
    let profile_meta = current.profile.profile.as_ref();
    let profile_id = profile_meta.map(|p| p.profile_id);
    let _ = app
        .database
        .insert_vowifi_runtime_event(crate::platform::db::NewVowifiRuntimeEvent {
            trace_id: Some("runtime-connect"),
            level: "info",
            phase: "connect_start",
            profile_id,
            event_type: "connect_start",
            detail_json: "{}",
        });

    let attempts = attempts.max(1);
    if let Err(err) = pause_cellular_data_for_vowifi(app, scope).await {
        let mut status = disabled_vowifi_status("vowifi_cellular_data_pause_failed");
        status.degraded_reason = Some(format!("vowifi_cellular_data_pause_failed:{err}"));
        persist_vowifi_runtime_snapshot(app, &status);
        return status;
    }

    let prepared = wait_for_vowifi_identity_gate(app, scope, None, 0).await;
    if !prepared.readiness.identity_ready || !prepared.readiness.profile_matched {
        persist_vowifi_runtime_snapshot(app, &prepared);
        return prepared;
    }

    if let Err(status) = wait_for_vowifi_sim_auth_gate(app, scope, None, 0).await {
        persist_vowifi_runtime_snapshot(app, &status);
        return status;
    }

    let mut last_status = disabled_vowifi_status("vowifi_connect_not_attempted");
    for attempt in 1..=attempts {
        info!(
            attempt = attempt,
            attempts = attempts,
            "WiFi Calling connection attempt started"
        );
        last_status = attempt_vowifi_connect_once(app, scope, false).await;
        if last_status.readiness.sms_ready {
            return last_status;
        }
        if attempt < attempts {
            tokio::time::sleep(retry_delay).await;
        }
    }

    if fallback_to_cellular_on_failure {
        let fallback_reason = last_status
            .degraded_reason
            .as_deref()
            .map(|reason| format!("vowifi_connect_failed_cellular_fallback:{reason}"))
            .unwrap_or_else(|| "vowifi_connect_failed_cellular_fallback".to_string());
        warn!(
            reason = fallback_reason.as_str(),
            "WiFi Calling connection attempts exhausted; falling back to cellular"
        );
        last_status =
            fallback_to_cellular_and_disable_vowifi_connection(app, scope, &fallback_reason).await;
    }
    last_status
}

#[derive(Deserialize)]
pub struct VowifiListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub trace_id: Option<String>,
    pub live: Option<bool>,
}

#[derive(Deserialize, Default)]
pub struct VowifiStatusQuery {
    #[serde(default)]
    pub live: Option<bool>,
}

#[derive(Deserialize)]
pub struct VowifiControlToggleRequest {
    pub enabled: bool,
}

// ===================== VoLTE handlers (stage 1 skeleton) =====================

#[derive(Deserialize)]
pub struct VolteControlToggleRequest {
    pub enabled: bool,
}

/// Combined VoLTE control response: persisted config + live runtime snapshot.
/// The `runtime` field carries the `volteStatus.js`-contract fields; `enabled`
/// mirrors the frontend `m()` helper (`feature_enabled && sms_enabled`).
#[derive(Debug, serde::Serialize, Default)]
pub struct VolteControlResponse {
    pub enabled: bool,
    pub feature_enabled: bool,
    pub sms_enabled: bool,
    pub connection_enabled: bool,
    pub runtime: crate::connectivity::modems::softstack::volte::VolteRuntimeStatus,
}

#[derive(Debug, serde::Serialize, Default)]
pub struct VolteLineControlResponse {
    pub modem: crate::hardware::cellular::modem_manager::ModemBinding,
    pub profile: LineProfileConfig,
    pub runtime: crate::connectivity::modems::softstack::volte::VolteRuntimeStatus,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct VowifiLineConfigResponse {
    pub line_id: String,
    pub modem: crate::hardware::cellular::modem_manager::ModemBinding,
    pub config: LineVowifiConfig,
    pub is_primary: bool,
    /// The current live VoWiFi executor is still shared by the legacy primary
    /// line. Per-line network settings are persisted now so the runtime can be
    /// split without another configuration migration.
    pub runtime_scope: &'static str,
    pub runtime_phase: String,
    pub runtime_stage: String,
    pub runtime_registered: bool,
    pub runtime_error: Option<String>,
    pub matched_profile_id: Option<String>,
}

async fn build_vowifi_line_response(
    app: &AppState,
    line: &crate::services::line_registry::LineRuntime,
    is_primary: bool,
) -> VowifiLineConfigResponse {
    let modem = line.binding();
    let config = app.config_manager.get_line_profile(&modem.line_id).vowifi;
    // Every line has a real runtime now, so report that line's own phase instead
    // of the old "only the primary line has a runtime" placeholder.
    let status = line.vowifi.snapshot().await.status_response();
    let (runtime_phase, runtime_stage, runtime_registered, runtime_error, matched_profile_id) = {
        let stage = if config.enabled && status.phase == "not_started" {
            "starting".to_string()
        } else {
            status.phase.to_string()
        };
        (
            status.phase.to_string(),
            stage,
            status.readiness.ims_registered,
            status.degraded_reason,
            status
                .profile
                .profile
                .map(|profile| profile.profile_id.to_string()),
        )
    };
    VowifiLineConfigResponse {
        line_id: modem.line_id.clone(),
        modem,
        config,
        is_primary,
        runtime_scope: "per_line_runtime",
        runtime_phase,
        runtime_stage,
        runtime_registered,
        runtime_error,
        matched_profile_id,
    }
}

pub async fn get_vowifi_lines_handler(
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<Vec<VowifiLineConfigResponse>>>) {
    if let Err(error) = app.line_registry.refresh(app.dbus_conn.as_ref()).await {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error(format!(
                "Failed to discover modems: {error}"
            ))),
        );
    }
    let lines = app.line_registry.all().await;
    let primary_line_id = app
        .line_registry
        .primary()
        .await
        .map(|line| line.binding().line_id);
    let mut response = Vec::with_capacity(lines.len());
    for line in &lines {
        let is_primary = primary_line_id
            .as_ref()
            .is_some_and(|line_id| line.binding().line_id == *line_id);
        response.push(build_vowifi_line_response(&app, line, is_primary).await);
    }
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", response)),
    )
}

pub async fn set_vowifi_line_config_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<LineVowifiConfig>,
) -> (StatusCode, Json<ApiResponse<VowifiLineConfigResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    if let Err(error) = app.config_manager.set_line_vowifi_config(&line_id, payload) {
        return (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {error}"))),
        );
    }
    let is_primary = app
        .line_registry
        .primary()
        .await
        .is_some_and(|primary| primary.binding().line_id == line_id);
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            build_vowifi_line_response(&app, &line, is_primary).await,
        )),
    )
}

pub async fn set_vowifi_line_connection_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<VowifiControlToggleRequest>,
) -> (StatusCode, Json<ApiResponse<VowifiLineConfigResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    if payload.enabled && !line.binding().present {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("line_not_present")),
        );
    }
    let is_primary = app
        .line_registry
        .primary()
        .await
        .is_some_and(|primary| primary.binding().line_id == line_id);
    // Validate and publish this line's network overrides regardless of whether it
    // is the primary line: overrides are keyed per line, so a bad proxy or DNS
    // value must be rejected for the line it belongs to rather than only when it
    // happens to be primary.
    if payload.enabled {
        let mut next = app.config_manager.get_line_profile(&line_id).vowifi;
        next.enabled = true;
        if let Err(error) =
            crate::connectivity::modems::softstack::vowifi::live::configure_live_network_overrides(&line_id, &next)
        {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse::error(format!("Failed: {error}"))),
            );
        }
    } else {
        // Drop the overrides so a disabled line stops influencing anything.
        crate::connectivity::modems::softstack::vowifi::live::forget_live_network_overrides(&line_id);
    }
    if let Err(error) = app
        .config_manager
        .set_line_vowifi_connection_enabled(&line_id, payload.enabled)
    {
        return (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {error}"))),
        );
    }
    // Connect/disconnect the line the request names. Lines no longer share a
    // runtime, so a secondary SIM can hold its own VoWiFi registration while the
    // primary one is up (or down).
    let scope = VowifiScope::resolve(&app, Some(&line_id)).await;
    if payload.enabled {
        // `feature_enabled` stays a process-level kill switch; the per-line
        // `connection_enabled` flag is what decides whether this SIM dials.
        let _ = app.config_manager.set_vowifi_feature_enabled(true);
        if is_primary {
            let _ = app.config_manager.set_vowifi_connection_enabled(true);
        }
        let connect_app = app.clone();
        tokio::spawn(async move {
            let _ = connect_vowifi_on_line(
                &connect_app,
                &scope,
                VOWIFI_MANUAL_CONNECT_ATTEMPTS,
                Duration::from_secs(VOWIFI_MANUAL_CONNECT_RETRY_DELAY_SECS),
                true,
            )
            .await;
        });
    } else {
        if is_primary {
            let _ = app.config_manager.set_vowifi_connection_enabled(false);
        }
        let _ = reset_vowifi_runtime_for_scope(&app, &scope, "vowifi_line_disabled").await;
    }
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            build_vowifi_line_response(&app, &line, is_primary).await,
        )),
    )
}

pub async fn get_standalone_sim_slots_handler(
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<Vec<StandaloneSimSlotConfig>>>) {
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            app.config_manager.get_standalone_sim_slots(),
        )),
    )
}

pub async fn set_standalone_sim_slots_handler(
    State(app): State<AppState>,
    Json(payload): Json<Vec<StandaloneSimSlotConfig>>,
) -> (StatusCode, Json<ApiResponse<Vec<StandaloneSimSlotConfig>>>) {
    match app.config_manager.set_standalone_sim_slots(payload) {
        Ok(slots) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", slots)),
        ),
        Err(error) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {error}"))),
        ),
    }
}

/// One line's eSIM management state: the persisted override plus whether the
/// hardware actually reports a eUICC, so the UI can render the "auto" case
/// without paying for an lpac probe on every refresh.
#[derive(Debug, Default, serde::Serialize)]
pub struct LineEsimControlResponse {
    pub line_id: String,
    /// `None` = auto (follow detection), `Some(true/false)` = explicit override.
    pub esim_control: Option<bool>,
    /// ModemManager's view of the card: "physical" / "esim" / "unknown".
    pub sim_type: String,
    /// "none" / "no-profiles" / "with-profiles" / "unknown".
    pub esim_status: String,
    /// Whether the discovered SIM advertises a eUICC chip.
    pub euicc_detected: bool,
    /// Effective result: may this line run lpac eSIM operations right now?
    pub esim_enabled: bool,
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct LineEsimControlRequest {
    /// Omit or send `null` to return the line to automatic detection.
    #[serde(default)]
    pub esim_control: Option<bool>,
}

fn build_line_esim_control_response(
    line_id: &str,
    esim_control: Option<bool>,
    binding: &crate::hardware::cellular::modem_manager::ModemBinding,
) -> LineEsimControlResponse {
    let euicc_detected = line_reports_euicc(binding);
    LineEsimControlResponse {
        line_id: line_id.to_string(),
        esim_control,
        sim_type: binding.sim_type.clone(),
        esim_status: binding.esim_status.clone(),
        euicc_detected,
        esim_enabled: esim_control.unwrap_or(euicc_detected),
    }
}

/// GET /api/modem/lines/{line_id}/esim-control
pub async fn get_line_esim_control_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<LineEsimControlResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    let esim_control = app.config_manager.get_line_profile(&line_id).esim_control;
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            build_line_esim_control_response(&line_id, esim_control, &line.binding()),
        )),
    )
}

/// POST /api/modem/lines/{line_id}/esim-control
pub async fn set_line_esim_control_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<LineEsimControlRequest>,
) -> (StatusCode, Json<ApiResponse<LineEsimControlResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    let profile = match app
        .config_manager
        .set_line_esim_control(&line_id, payload.esim_control)
    {
        Ok(profile) => profile,
        Err(error) => {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse::error(format!("Failed: {error}"))),
            )
        }
    };
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "eSIM control updated",
            build_line_esim_control_response(&line_id, profile.esim_control, &line.binding()),
        )),
    )
}

fn build_volte_line_response(
    app: &AppState,
    status: crate::services::line_registry::LineRuntimeStatus,
) -> VolteLineControlResponse {
    VolteLineControlResponse {
        // Redacted: the embedded trunk settings carry a Digest secret that must
        // never cross the API boundary.
        profile: app
            .config_manager
            .get_line_profile(&status.modem.line_id)
            .redacted(),
        modem: status.modem,
        runtime: status.volte,
    }
}

pub async fn get_volte_lines_handler(
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<Vec<VolteLineControlResponse>>>) {
    if let Err(error) = app.line_registry.refresh(app.dbus_conn.as_ref()).await {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error(format!(
                "Failed to discover modems: {error}"
            ))),
        );
    }
    let lines = app
        .line_registry
        .statuses()
        .await
        .into_iter()
        .map(|status| build_volte_line_response(&app, status))
        .collect();
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", lines)),
    )
}

pub async fn get_volte_line_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<VolteLineControlResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            build_volte_line_response(&app, line.status().await),
        )),
    )
}

pub async fn set_volte_line_connection_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<VolteControlToggleRequest>,
) -> (StatusCode, Json<ApiResponse<VolteLineControlResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    let binding = line.binding();
    if payload.enabled && !binding.present {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("line_not_present")),
        );
    }
    if payload.enabled
        && app
            .config_manager
            .get_line_profile(&line_id)
            .airplane_mode_enabled
    {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("line_airplane_mode_enabled")),
        );
    }
    let profile = match app
        .config_manager
        .set_line_volte_connection_enabled(&line_id, payload.enabled)
    {
        Ok(profile) => profile,
        Err(error) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::error(format!("Failed: {error}"))),
            )
        }
    };
    let result: Result<crate::connectivity::modems::softstack::volte::VolteRuntimeStatus, crate::connectivity::modems::softstack::volte::VolteError> =
        if payload.enabled {
            start_line_volte_restore(app.clone(), Arc::clone(&line), "connection_enabled").await;
            Ok(line.volte.status().await)
        } else {
            let _bearer_guard = line.bearer_operation_lock.lock().await;
            let _guard = line.volte_connect_lock.lock().await;
            Ok(crate::connectivity::modems::softstack::volte::live::disconnect_live_for_line(
                &line.volte_live,
                &line.volte,
                "volte_line_connection_disabled",
            )
            .await)
        };
    let response = VolteLineControlResponse {
        modem: line.binding(),
        profile: profile.redacted(),
        runtime: line.volte.status().await,
    };
    match result {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", response)),
        ),
        Err(error) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {error}"))),
        ),
    }
}

/// Body for `PUT /api/volte/lines/{line_id}/ip-families`.
///
/// `families` is the ordered attempt list (`["ipv4","ipv6"]`, `["ipv6"]`, …).
/// `null` (or an absent field) clears the per-line override so the line follows
/// the global `ip_family_preference` default again.
#[derive(Debug, serde::Deserialize)]
pub struct SetVolteIpFamiliesRequest {
    #[serde(default)]
    pub families: Option<Vec<crate::platform::config::VolteIpFamily>>,
}

/// PUT /api/volte/lines/{line_id}/ip-families
///
/// Persist this line's ordered IMS address-family list. If the line is currently
/// connected, restart it so the new order takes effect immediately.
pub async fn set_volte_line_ip_families_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<SetVolteIpFamiliesRequest>,
) -> (StatusCode, Json<ApiResponse<VolteLineControlResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    let profile = match app
        .config_manager
        .set_line_volte_ip_families(&line_id, payload.families)
    {
        Ok(profile) => profile,
        Err(error) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::error(format!("Failed: {error}"))),
            )
        }
    };
    // The family order is only consulted when a session is (re)established, so an
    // already-registered line has to be restarted for the change to take effect.
    if profile.volte_connection_enabled && line.volte.status().await.registered {
        {
            let _bearer_guard = line.bearer_operation_lock.lock().await;
            let _guard = line.volte_connect_lock.lock().await;
            crate::connectivity::modems::softstack::volte::live::disconnect_live_for_line(
                &line.volte_live,
                &line.volte,
                "volte_ip_families_changed",
            )
            .await;
        }
        start_line_volte_restore(app.clone(), Arc::clone(&line), "ip_families_changed").await;
    }
    let response = VolteLineControlResponse {
        modem: line.binding(),
        profile: profile.redacted(),
        runtime: line.volte.status().await,
    };
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", response)),
    )
}

/// Start a fresh five-attempt recovery batch without changing the persisted
/// VoLTE switch.  The response is immediate; progress is returned by the normal
/// line status endpoint and automatic polling.
pub async fn retry_volte_line_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<VolteLineControlResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    let profile = app.config_manager.get_line_profile(&line_id);
    if !profile.enabled || !profile.volte_connection_enabled {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("volte_line_connection_disabled")),
        );
    }
    if line.volte.status().await.registered {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("volte_line_already_registered")),
        );
    }
    if !start_line_volte_restore(app.clone(), Arc::clone(&line), "manual").await {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("volte_retry_already_running")),
        );
    }
    (
        StatusCode::ACCEPTED,
        Json(ApiResponse::success_with_message(
            "VoLTE retry started",
            build_volte_line_response(&app, line.status().await),
        )),
    )
}

// ===================== Trunk handlers (stage D3b: config only) =====================

/// Response for the per-line trunk config endpoints. The trunk `secret` is
/// always redacted; `secret_set` tells the UI whether one is stored so it can
/// show a "configured" hint without leaking the value.
#[derive(Debug, Default, serde::Serialize)]
pub struct TrunkProfileResponse {
    pub line_id: String,
    pub modem: crate::hardware::cellular::modem_manager::ModemBinding,
    pub trunk: TrunkProfileConfig,
    pub secret_set: bool,
    pub runtime: crate::services::trunk::runtime::TrunkRuntimeStatus,
}

impl TrunkProfileResponse {
    async fn from_line(
        profile: &LineProfileConfig,
        line: &crate::services::line_registry::LineRuntime,
    ) -> Self {
        Self {
            line_id: profile.line_id.clone(),
            modem: line.binding(),
            secret_set: profile.trunk.secret_set(),
            trunk: profile.trunk.redacted(),
            runtime: line.trunk.status().await,
        }
    }
}

pub async fn get_trunk_lines_handler(
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<Vec<TrunkProfileResponse>>>) {
    if let Err(error) = app.line_registry.refresh(app.dbus_conn.as_ref()).await {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error(format!(
                "Failed to discover modems: {error}"
            ))),
        );
    }
    let lines = app.line_registry.all().await;
    let mut responses = Vec::with_capacity(lines.len());
    for line in lines {
        let profile = app.config_manager.get_line_profile(&line.binding().line_id);
        line.trunk.reconcile_profile(&profile.trunk).await;
        responses.push(TrunkProfileResponse::from_line(&profile, &line).await);
    }
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", responses)),
    )
}

/// Read one line's trunk settings (secret redacted). Returns the inert default
/// for a line that has never been configured, so the UI always has a shape.
pub async fn get_line_trunk_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<TrunkProfileResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    let profile = app.config_manager.get_line_profile(&line_id);
    line.trunk.reconcile_profile(&profile.trunk).await;
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            TrunkProfileResponse::from_line(&profile, &line).await,
        )),
    )
}

/// Replace one line's trunk settings. An empty `secret` in the payload keeps the
/// stored secret (redacted round-trip). Validation/gating lives in the config
/// layer; its error strings are surfaced verbatim for the UI to map.
pub async fn set_line_trunk_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<TrunkProfileConfig>,
) -> (StatusCode, Json<ApiResponse<TrunkProfileResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    match app.config_manager.set_line_trunk_profile(&line_id, payload) {
        Ok(profile) => {
            line.trunk.activate_profile(&profile.trunk).await;
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Success",
                    TrunkProfileResponse::from_line(&profile, &line).await,
                )),
            )
        }
        Err(error) => (
            StatusCode::OK,
            Json(ApiResponse::<TrunkProfileResponse>::error(format!(
                "Failed: {error}"
            ))),
        ),
    }
}

/// Toggle one line's trunk on/off without resubmitting the full settings.
pub async fn set_line_trunk_enabled_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<VolteControlToggleRequest>,
) -> (StatusCode, Json<ApiResponse<TrunkProfileResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    match app
        .config_manager
        .set_line_trunk_enabled(&line_id, payload.enabled)
    {
        Ok(profile) => {
            line.trunk.activate_profile(&profile.trunk).await;
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Success",
                    TrunkProfileResponse::from_line(&profile, &line).await,
                )),
            )
        }
        Err(error) => (
            StatusCode::OK,
            Json(ApiResponse::<TrunkProfileResponse>::error(format!(
                "Failed: {error}"
            ))),
        ),
    }
}

impl VolteControlResponse {
    fn build(
        _config: &VolteConfig,
        runtime: crate::connectivity::modems::softstack::volte::VolteRuntimeStatus,
        line_enabled: bool,
    ) -> Self {
        Self {
            enabled: line_enabled,
            feature_enabled: line_enabled,
            sms_enabled: line_enabled,
            connection_enabled: line_enabled,
            runtime,
        }
    }
}

fn any_volte_line_enabled(app: &AppState) -> bool {
    app.config_manager
        .get_line_profiles()
        .iter()
        .any(|profile| profile.enabled && profile.volte_connection_enabled)
}

pub async fn get_volte_control_handler(
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VolteControlResponse>>) {
    let config = app.config_manager.get_volte_config();
    let runtime = app.volte_runtime.status().await;
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            VolteControlResponse::build(&config, runtime, any_volte_line_enabled(&app)),
        )),
    )
}

pub async fn set_volte_feature_handler(
    State(app): State<AppState>,
    Json(payload): Json<VolteControlToggleRequest>,
) -> (StatusCode, Json<ApiResponse<VolteControlResponse>>) {
    match app
        .config_manager
        .set_volte_feature_enabled(payload.enabled)
    {
        Ok(config) => {
            if !payload.enabled {
                crate::connectivity::modems::softstack::volte::live::disconnect_live(&app.volte_runtime, "volte_disabled")
                    .await;
            }
            let runtime = app.volte_runtime.status().await;
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Success",
                    VolteControlResponse::build(&config, runtime, any_volte_line_enabled(&app)),
                )),
            )
        }
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<VolteControlResponse>::error(format!(
                "Failed: {err}"
            ))),
        ),
    }
}

/// Legacy global endpoint. Physical IMS sessions must be controlled through a
/// line endpoint so multi-modem systems never select an implicit modem.
pub async fn set_volte_connection_handler(
    State(app): State<AppState>,
    Json(payload): Json<VolteControlToggleRequest>,
) -> (StatusCode, Json<ApiResponse<VolteControlResponse>>) {
    if payload.enabled {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("volte_line_endpoint_required")),
        );
    }
    let _guard = app.volte_connect_lock.lock().await;
    match app
        .config_manager
        .set_volte_connection_enabled(payload.enabled)
    {
        Ok(config) => {
            crate::connectivity::modems::softstack::volte::live::disconnect_live(
                &app.volte_runtime,
                "volte_connection_disabled",
            )
            .await;
            let runtime = app.volte_runtime.status().await;
            let response =
                VolteControlResponse::build(&config, runtime, any_volte_line_enabled(&app));
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message("Success", response)),
            )
        }
        Err(error) => (
            StatusCode::OK,
            Json(ApiResponse::<VolteControlResponse>::error(format!(
                "Failed: {error}"
            ))),
        ),
    }
}

/// VoLTE voice (gateway-mode) status. Voice is a forward-looking extension: the
/// target device relays RTP between the operator IMS leg and an internal SIP UA;
/// it never plays audio locally. `enabled` reflects `feature_enabled &&
/// voice_enabled`; `gateway_mode` is always true on this hardware class.
#[derive(Debug, serde::Serialize, Default)]
pub struct VolteVoiceStatusResponse {
    pub enabled: bool,
    pub feature_enabled: bool,
    pub voice_enabled: bool,
    pub gateway_mode: bool,
    pub local_audio_capable: bool,
}

impl VolteVoiceStatusResponse {
    fn build(config: &VolteConfig, line_enabled: bool) -> Self {
        Self {
            enabled: line_enabled && config.voice_enabled,
            feature_enabled: line_enabled,
            voice_enabled: config.voice_enabled,
            // Qualcomm 410 pocket-WiFi has no mic/speaker/PCM: relay only.
            gateway_mode: true,
            local_audio_capable: false,
        }
    }
}

pub async fn get_volte_call_status_handler(
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VolteVoiceStatusResponse>>) {
    let config = app.config_manager.get_volte_config();
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            VolteVoiceStatusResponse::build(&config, any_volte_line_enabled(&app)),
        )),
    )
}

pub async fn set_volte_voice_handler(
    State(app): State<AppState>,
    Json(payload): Json<VolteControlToggleRequest>,
) -> (StatusCode, Json<ApiResponse<VolteVoiceStatusResponse>>) {
    match app.config_manager.set_volte_voice_enabled(payload.enabled) {
        Ok(config) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Success",
                VolteVoiceStatusResponse::build(&config, any_volte_line_enabled(&app)),
            )),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<VolteVoiceStatusResponse>::error(format!(
                "Failed: {err}"
            ))),
        ),
    }
}

// ============ SMS multi-path orchestration policy (phase C) ============

/// Read the current SMS multi-path routing policy. The returned policy is
/// normalized so the priority list always contains every path kind exactly
/// once (missing kinds appended in canonical order), which keeps the UI's
/// reorder/enable controls well-defined.
pub async fn get_sms_path_policy_handler(
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<SmsPathPolicy>>) {
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            app.config_manager.get_sms_path_policy(),
        )),
    )
}

/// Replace the SMS multi-path routing policy. The incoming policy is normalized
/// before persisting, so a partial or duplicated priority list from the UI can
/// never leave the config in an invalid state.
pub async fn set_sms_path_policy_handler(
    State(app): State<AppState>,
    Json(payload): Json<SmsPathPolicy>,
) -> (StatusCode, Json<ApiResponse<SmsPathPolicy>>) {
    match app.config_manager.set_sms_path_policy(payload) {
        Ok(policy) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", policy)),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<SmsPathPolicy>::error(format!(
                "Failed: {err}"
            ))),
        ),
    }
}

// ==================== ViLTE (video over LTE) policy (phase F) ====================

/// ViLTE status: persisted config + a derived `enabled` that reflects the full
/// gating chain (`volte.feature_enabled && volte.voice_enabled &&
/// vilte.feature_enabled`). `gateway_mode`/`local_video_capable` mirror the
/// VoLTE voice response: the device is a pure video relay, never a video
/// endpoint.
#[derive(Debug, serde::Serialize, Default)]
pub struct VilteStatusResponse {
    pub enabled: bool,
    pub feature_enabled: bool,
    pub gateway_mode: bool,
    pub local_video_capable: bool,
    pub config: VilteConfig,
}

impl VilteStatusResponse {
    fn build(app: &AppState) -> Self {
        let volte = app.config_manager.get_volte_config();
        let vilte = app.config_manager.get_vilte_config();
        let voice_ready = any_volte_line_enabled(app) && volte.voice_enabled;
        Self {
            enabled: voice_ready && vilte.feature_enabled,
            feature_enabled: vilte.feature_enabled,
            // Qualcomm 410 pocket-WiFi has no camera/display/codec: relay only.
            gateway_mode: true,
            local_video_capable: false,
            config: vilte,
        }
    }
}

pub async fn get_vilte_control_handler(
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VilteStatusResponse>>) {
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            VilteStatusResponse::build(&app),
        )),
    )
}

pub async fn set_vilte_feature_handler(
    State(app): State<AppState>,
    Json(payload): Json<VolteControlToggleRequest>,
) -> (StatusCode, Json<ApiResponse<VilteStatusResponse>>) {
    match app
        .config_manager
        .set_vilte_feature_enabled(payload.enabled)
    {
        Ok(_) => {
            let status = VilteStatusResponse::build(&app);
            for line in app.line_registry.all().await {
                line.volte_live
                    .operator_link()
                    .set_video_enabled(status.enabled);
            }
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message("Success", status)),
            )
        }
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<VilteStatusResponse>::error(format!(
                "Failed: {err}"
            ))),
        ),
    }
}

/// Replace the full ViLTE config (codec / payload type / fmtp). The
/// `feature_enabled` field is honored only when VoLTE voice is enabled;
/// otherwise it is forced off by the config layer.
pub async fn set_vilte_config_handler(
    State(app): State<AppState>,
    Json(payload): Json<VilteConfig>,
) -> (StatusCode, Json<ApiResponse<VilteStatusResponse>>) {
    match app.config_manager.set_vilte_config(payload) {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Success",
                VilteStatusResponse::build(&app),
            )),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<VilteStatusResponse>::error(format!(
                "Failed: {err}"
            ))),
        ),
    }
}

pub async fn get_vowifi_profiles_handler() -> (StatusCode, Json<ApiResponse<VowifiProfilesResponse>>)
{
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            vowifi_diagnostics::list_profiles(),
        )),
    )
}

// ===================== VoWiFi carrier profile database =====================

fn profile_store(app: &AppState) -> crate::connectivity::modems::softstack::vowifi::profile_store::ProfileStore {
    crate::connectivity::modems::softstack::vowifi::profile_store::ProfileStore::new(Arc::clone(&app.database))
}

/// GET /api/vowifi/carrier-profiles
pub async fn list_vowifi_carrier_profiles_handler(
    State(app): State<AppState>,
) -> (
    StatusCode,
    Json<ApiResponse<Vec<crate::connectivity::modems::softstack::vowifi::profile_store::StoredProfile>>>,
) {
    match profile_store(&app).list() {
        Ok(profiles) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", profiles)),
        ),
        Err(error) => (StatusCode::OK, Json(ApiResponse::error(error))),
    }
}

/// PUT /api/vowifi/carrier-profiles
///
/// Insert or replace one profile. The body is the full profile document; the
/// record is validated before it is stored so a bad edit is rejected here
/// rather than surfacing later as a failed IKE or REGISTER exchange.
pub async fn save_vowifi_carrier_profile_handler(
    State(app): State<AppState>,
    Json(record): Json<crate::connectivity::modems::softstack::vowifi::profile_record::CarrierProfileRecord>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    match profile_store(&app).save(&record, "manual") {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Success",
                json!({
                    "profile_id": record.meta.profile_id,
                    "plmn": record.meta.plmn,
                    // Lets the UI prompt for emergency setup where it matters
                    // without blocking carriers that do not need it.
                    "e911_expected": record.e911_expected(),
                }),
            )),
        ),
        Err(error) => (StatusCode::OK, Json(ApiResponse::error(error))),
    }
}

/// DELETE /api/vowifi/carrier-profiles/{profile_id}
///
/// Removing a row falls back to the built-in or derived profile for that PLMN.
pub async fn delete_vowifi_carrier_profile_handler(
    State(app): State<AppState>,
    Path(profile_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    match profile_store(&app).delete(&profile_id) {
        Ok(true) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Success",
                json!({ "deleted": true }),
            )),
        ),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("profile_not_found")),
        ),
        Err(error) => (StatusCode::OK, Json(ApiResponse::error(error))),
    }
}

/// GET /api/vowifi/carrier-profiles/resolve?plmn=23433
///
/// Show which profile a PLMN actually resolves to and where it came from
/// (database / builtin / derived).
pub async fn resolve_vowifi_carrier_profile_handler(
    State(app): State<AppState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let plmn = query
        .get("plmn")
        .map(|value| {
            value
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>()
        })
        .unwrap_or_default();
    if plmn.len() < 5 || plmn.len() > 6 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("plmn_must_be_five_or_six_digits")),
        );
    }
    let (mcc, mnc) = plmn.split_at(3);
    match profile_store(&app).resolve_by_plmn(mcc, mnc) {
        Some(resolved) => {
            let record = crate::connectivity::modems::softstack::vowifi::profile_record::CarrierProfileRecord::from_profile(
                resolved.profile,
            );
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Success",
                    json!({
                        "origin": resolved.origin.as_str(),
                        "e911_expected": record.e911_expected(),
                        "record": record,
                    }),
                )),
            )
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("profile_not_resolvable")),
        ),
    }
}

#[derive(Deserialize)]
pub struct VowifiProfileImportRequest {
    /// `aosp_apns`, `aosp_carrier_config` or `ipcc`.
    pub format: String,
    /// The raw document (XML for all three formats).
    pub content: String,
    /// Required for `aosp_carrier_config` and `ipcc`, which do not carry the
    /// PLMN inside the document.
    #[serde(default)]
    pub mcc: Option<String>,
    #[serde(default)]
    pub mnc: Option<String>,
    /// Preview only; nothing is written unless this is false.
    #[serde(default)]
    pub dry_run: bool,
}

/// POST /api/vowifi/carrier-profiles/import
///
/// Import carrier facts from a public source. Imported values are overlaid on
/// the 3GPP-derived defaults, so an import never invents an ePDG hostname or
/// silently blanks a field the source does not cover.
pub async fn import_vowifi_carrier_profiles_handler(
    State(app): State<AppState>,
    Json(payload): Json<VowifiProfileImportRequest>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    use crate::connectivity::modems::softstack::vowifi::profile_import;

    let facts = match payload.format.as_str() {
        "aosp_apns" => profile_import::parse_aosp_apns(&payload.content),
        "aosp_carrier_config" | "ipcc" => {
            let (Some(mcc), Some(mnc)) = (payload.mcc.as_deref(), payload.mnc.as_deref()) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse::error("mcc_and_mnc_required_for_this_format")),
                );
            };
            let parsed = if payload.format == "ipcc" {
                profile_import::parse_ipcc_carrier_plist(&payload.content, mcc, mnc)
            } else {
                profile_import::parse_aosp_carrier_config(&payload.content, mcc, mnc)
            };
            vec![parsed]
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error("unsupported_import_format")),
            )
        }
    };

    let store = profile_store(&app);
    let mut imported = Vec::new();
    let mut skipped = Vec::new();
    for fact in &facts {
        let Some(record) = fact.to_record() else {
            skipped.push(json!({ "plmn": fact.plmn(), "reason": "invalid_plmn" }));
            continue;
        };
        if let Err(error) = record.validate() {
            skipped.push(json!({ "plmn": fact.plmn(), "reason": error }));
            continue;
        }
        if !payload.dry_run {
            if let Err(error) = store.save(&record, &payload.format) {
                skipped.push(json!({ "plmn": fact.plmn(), "reason": error }));
                continue;
            }
        }
        imported.push(json!({
            "profile_id": record.meta.profile_id,
            "plmn": record.meta.plmn,
            "brand": record.meta.brand,
            "ims_apn": record.epdg.apn,
            "vowifi_supported": fact.vowifi_supported,
            "e911_expected": record.e911_expected(),
        }));
    }

    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            json!({
                "dry_run": payload.dry_run,
                "imported": imported,
                "skipped": skipped,
            }),
        )),
    )
}

/// GET /api/vowifi/external-profiles
///
/// Legacy view of the carrier profile database, kept so the per-line dialog's
/// "apply a saved ePDG" shortcut keeps working. Custom profiles are no longer a
/// separate `vowifi-profiles.conf` file — they are rows in the database.
pub async fn get_external_vowifi_profiles_handler(
    State(app): State<AppState>,
) -> (
    StatusCode,
    Json<ApiResponse<Vec<crate::platform::config::ExternalVowifiProfile>>>,
) {
    match profile_store(&app).list_as_external() {
        Ok(profiles) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", profiles)),
        ),
        Err(error) => (StatusCode::OK, Json(ApiResponse::error(error))),
    }
}

/// POST /api/vowifi/external-profiles
///
/// Apply an ePDG override. It is expanded into a full profile row: an existing
/// row is edited in place so any REGISTER policy already tuned there survives.
pub async fn set_external_vowifi_profile_handler(
    State(app): State<AppState>,
    Json(mut payload): Json<crate::platform::config::ExternalVowifiProfile>,
) -> (
    StatusCode,
    Json<ApiResponse<Vec<crate::platform::config::ExternalVowifiProfile>>>,
) {
    payload.profile_id = payload.profile_id.trim().to_string();
    payload.mcc = payload.mcc.trim().to_string();
    payload.mnc = payload.mnc.trim().to_string();
    payload.epdg_host = payload.epdg_host.trim().trim_end_matches('.').to_string();
    if payload.profile_id.is_empty()
        || payload.mcc.len() != 3
        || payload.mnc.len() < 2
        || payload.mnc.len() > 3
        || payload.epdg_host.is_empty()
        || payload.epdg_port == 0
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("vowifi_external_profile_invalid")),
        );
    }
    let store = profile_store(&app);
    match store
        .save_external(&payload)
        .and_then(|_| store.list_as_external())
    {
        Ok(profiles) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", profiles)),
        ),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error(format!("Failed: {error}"))),
        ),
    }
}

pub async fn get_vowifi_profile_handler(
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VowifiProfileMatchResponse>>) {
    let profile = current_vowifi_profile_match(&app).await;
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", profile)),
    )
}

async fn current_vowifi_status(app: &AppState, live_probe: bool) -> VowifiStatusResponse {
    let control = app.config_manager.get_vowifi_config();
    if !control.feature_enabled {
        return disabled_vowifi_status("vowifi_feature_disabled");
    }
    app.vowifi_runtime
        .refresh_identity_with_timeout(
            &app.dbus_conn,
            std::time::Duration::from_secs(VOWIFI_SIM_IDENTITY_TIMEOUT_SECS),
        )
        .await;
    let snapshot = if live_probe && control.connection_enabled {
        app.vowifi_runtime
            .refresh_status_readiness_with_stage_timeout(
                Some(&app.database),
                std::time::Duration::from_secs(VOWIFI_STATUS_STAGE_TIMEOUT_SECS),
            )
            .await
    } else {
        app.vowifi_runtime.snapshot().await
    };
    let mut status = snapshot.status_response();
    if !control.connection_enabled {
        status.phase = "not_started";
    }
    persist_vowifi_runtime_snapshot(app, &status);
    status
}

pub async fn get_vowifi_control_handler(
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VowifiConfig>>) {
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            app.config_manager.get_vowifi_config(),
        )),
    )
}

pub async fn set_vowifi_feature_handler(
    State(app): State<AppState>,
    Json(payload): Json<VowifiControlToggleRequest>,
) -> (StatusCode, Json<ApiResponse<VowifiConfig>>) {
    match app
        .config_manager
        .set_vowifi_feature_enabled(payload.enabled)
    {
        Ok(config) => {
            if !payload.enabled {
                // This is the process-wide kill switch, so every line's runtime
                // has to come down — not just the primary one.
                for line in app.line_registry.all().await {
                    let line_id = line.binding().line_id;
                    let scope = VowifiScope::resolve(&app, Some(&line_id)).await;
                    if let Err(err) = ensure_line_radio_enabled(&app, &scope).await {
                        warn!(line_id = %line_id, error = %err, "Failed to re-enable the line radio after WiFi Calling feature disable");
                    }
                    let _ = reset_vowifi_runtime_for_scope(&app, &scope, "vowifi_feature_disabled")
                        .await;
                }
            }
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message("Success", config)),
            )
        }
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::<VowifiConfig>::error(format!("Failed: {err}"))),
        ),
    }
}

pub async fn set_vowifi_connection_handler(
    State(app): State<AppState>,
    Json(payload): Json<VowifiControlToggleRequest>,
) -> (StatusCode, Json<ApiResponse<VowifiStatusResponse>>) {
    if !app.config_manager.get_vowifi_config().feature_enabled {
        return (
            StatusCode::OK,
            Json(ApiResponse::<VowifiStatusResponse>::error(
                "Failed: vowifi_feature_disabled",
            )),
        );
    }

    if !payload.enabled {
        let scope = VowifiScope::primary(&app).await;
        let status =
            stop_vowifi_and_restore_cellular(&app, &scope, "vowifi_connection_disabled").await;
        return (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", status)),
        );
    }

    if let Err(err) = app.config_manager.set_vowifi_connection_enabled(true) {
        return (
            StatusCode::OK,
            Json(ApiResponse::<VowifiStatusResponse>::error(format!(
                "Failed: {err}"
            ))),
        );
    }
    let status = connect_vowifi_with_attempts(
        &app,
        VOWIFI_MANUAL_CONNECT_ATTEMPTS,
        std::time::Duration::from_secs(VOWIFI_MANUAL_CONNECT_RETRY_DELAY_SECS),
        true,
    )
    .await;
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", status)),
    )
}

pub fn spawn_vowifi_auto_restore(app: AppState) {
    tokio::spawn(async move {
        let config = app.config_manager.get_vowifi_config();
        if !config.feature_enabled || !config.connection_enabled {
            return;
        }
        let workflow = VowifiRestoreWorkflow::boot_auto_restore(&config);
        info!(
            initial_delay_secs = workflow.initial_delay.as_secs(),
            attempts = workflow.attempts,
            "WiFi Calling auto-restore scheduled"
        );
        run_vowifi_restore_workflow(app, workflow).await;
    });
}

fn volte_next_retry_at(delay_secs: u64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(delay_secs as i64)).to_rfc3339()
}

async fn start_line_volte_restore(
    app: AppState,
    line: Arc<crate::services::line_registry::LineRuntime>,
    source: &'static str,
) -> bool {
    if !line.begin_volte_retry() {
        return false;
    }
    let volte = app.config_manager.get_volte_config();
    let vilte = app.config_manager.get_vilte_config();
    line.volte_live
        .operator_link()
        .set_video_enabled(volte.voice_enabled && vilte.feature_enabled);
    line.volte
        .update(|state| {
            state.recovery_state = crate::connectivity::modems::softstack::volte::runtime::VolteRecoveryState::Connecting;
            state.recovery_source = Some(source.to_string());
            state.retry_attempt = 0;
            state.retry_max = VOLTE_RESTORE_MAX_ATTEMPTS;
            state.modem_restart_attempt = 0;
            state.modem_restart_max = VOLTE_MODEM_RESTART_MAX_ATTEMPTS;
            state.manual_retry_available = false;
            state.next_retry_at = None;
            state.last_error = None;
        })
        .await;
    tokio::spawn(async move {
        run_line_volte_restore_batch(&app, &line, source).await;
        line.finish_volte_retry();
    });
    true
}

enum LineModemWait {
    Ready,
    Cancelled,
    Exhausted,
}

fn line_volte_restore_enabled(
    app: &AppState,
    line: &crate::services::line_registry::LineRuntime,
) -> bool {
    let profile = app.config_manager.get_line_profile(&line.binding().line_id);
    profile.enabled && profile.volte_connection_enabled
}

async fn wait_for_line_modem(
    app: &AppState,
    line: &Arc<crate::services::line_registry::LineRuntime>,
) -> LineModemWait {
    for poll in 0..VOLTE_MODEM_MISSING_POLLS {
        if !line_volte_restore_enabled(app, line) {
            return LineModemWait::Cancelled;
        }
        let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
        if line.binding().present {
            return LineModemWait::Ready;
        }
        line.volte
            .update(|state| {
                state.phase = crate::connectivity::modems::softstack::volte::runtime::VoltePhase::Degraded;
                state.stage = crate::connectivity::modems::softstack::volte::runtime::VolteStage::Modem;
                state.recovery_state =
                    crate::connectivity::modems::softstack::volte::runtime::VolteRecoveryState::WaitingModem;
                state.last_error = Some(format!(
                    "volte_modem_missing_wait:{}/{}",
                    poll + 1,
                    VOLTE_MODEM_MISSING_POLLS
                ));
                state.next_retry_at =
                    Some(volte_next_retry_at(VOLTE_MODEM_MISSING_POLL_DELAY_SECS));
            })
            .await;
        if poll + 1 < VOLTE_MODEM_MISSING_POLLS {
            tokio::time::sleep(Duration::from_secs(VOLTE_MODEM_MISSING_POLL_DELAY_SECS)).await;
        }
    }

    for restart_attempt in 1..=VOLTE_MODEM_RESTART_MAX_ATTEMPTS {
        if !line_volte_restore_enabled(app, line) {
            return LineModemWait::Cancelled;
        }
        line.volte
            .update(|state| {
                state.recovery_state =
                    crate::connectivity::modems::softstack::volte::runtime::VolteRecoveryState::RestartingBaseband;
                state.modem_restart_attempt = restart_attempt;
                state.next_retry_at = None;
                state.last_error = Some(format!(
                    "volte_modem_missing_recovery:{restart_attempt}/{VOLTE_MODEM_RESTART_MAX_ATTEMPTS}"
                ));
            })
            .await;
        match modem_manager::recover_missing_baseband(app.dbus_conn.as_ref()).await {
            Ok(_) => {
                let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
                if line.binding().present {
                    return LineModemWait::Ready;
                }
            }
            Err(error) => {
                warn!(restart_attempt, error = %error, "VoLTE missing-baseband recovery failed");
                line.volte
                    .update(|state| {
                        state.last_error = Some(format!("volte_baseband_recovery_failed:{error}"))
                    })
                    .await;
            }
        }
    }

    line.volte
        .update(|state| {
            state.phase = crate::connectivity::modems::softstack::volte::runtime::VoltePhase::Degraded;
            state.recovery_state = crate::connectivity::modems::softstack::volte::runtime::VolteRecoveryState::Exhausted;
            state.manual_retry_available = true;
            state.next_retry_at = None;
            state.last_error = Some("volte_baseband_recovery_exhausted".to_string());
            state.last_failure_at = Some(chrono::Utc::now().to_rfc3339());
        })
        .await;
    LineModemWait::Exhausted
}

async fn run_line_volte_restore_batch(
    app: &AppState,
    line: &Arc<crate::services::line_registry::LineRuntime>,
    source: &'static str,
) {
    match wait_for_line_modem(app, line).await {
        LineModemWait::Ready => {}
        LineModemWait::Cancelled => {
            line.volte
                .update(|state| {
                    state.recovery_state = crate::connectivity::modems::softstack::volte::runtime::VolteRecoveryState::Idle;
                    state.recovery_source = None;
                    state.next_retry_at = None;
                    state.manual_retry_available = false;
                })
                .await;
            return;
        }
        LineModemWait::Exhausted => return,
    }

    for attempt in 1..=VOLTE_RESTORE_MAX_ATTEMPTS {
        let config = app.config_manager.get_volte_config();
        let profile = app.config_manager.get_line_profile(&line.binding().line_id);
        if !profile.enabled || !profile.volte_connection_enabled {
            line.volte
                .update(|state| {
                    state.recovery_state = crate::connectivity::modems::softstack::volte::runtime::VolteRecoveryState::Idle;
                    state.recovery_source = None;
                    state.next_retry_at = None;
                    state.manual_retry_available = false;
                })
                .await;
            return;
        }
        let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
        if !line.binding().present {
            match wait_for_line_modem(app, line).await {
                LineModemWait::Ready => {}
                LineModemWait::Cancelled | LineModemWait::Exhausted => return,
            }
        }
        let binding = line.binding();
        let device = match crate::connectivity::modems::softstack::volte::live::VolteDeviceBinding::from_modem(&binding) {
            Ok(device) => device,
            Err(error) => {
                line.volte
                    .update(|state| state.last_error = Some(error.to_string()))
                    .await;
                continue;
            }
        };
        line.volte
            .update(|state| {
                state.recovery_state =
                    crate::connectivity::modems::softstack::volte::runtime::VolteRecoveryState::Connecting;
                state.recovery_source = Some(source.to_string());
                state.retry_attempt = attempt;
                state.next_retry_at = None;
                state.manual_retry_available = false;
                state.reconnect_count = state.reconnect_count.saturating_add(1);
            })
            .await;

        let result = {
            let _bearer_guard = line.bearer_operation_lock.lock().await;
            match prepare_line_data_slot_for_volte(app, line, &profile).await {
                Ok(data_slot_mode) => {
                    let _guard = line.volte_connect_lock.lock().await;
                    if line.volte.status().await.registered {
                        return;
                    }
                    crate::connectivity::modems::softstack::volte::live::connect_live_for_line(
                        &line.volte_live,
                        &device,
                        &line.volte,
                        &config,
                        profile.volte_ip_families.as_deref(),
                        profile.roaming_allowed,
                        data_slot_mode,
                        app.config_manager.get_sms_path_policy().dedupe_enabled,
                        Arc::clone(&app.database),
                        Arc::clone(&app.notification_sender),
                    )
                    .await
                }
                Err(error) => Err(error),
            }
        };
        match result {
            Ok(_) => {
                line.volte
                    .update(|state| {
                        state.recovery_state =
                            crate::connectivity::modems::softstack::volte::runtime::VolteRecoveryState::Registered;
                        state.manual_retry_available = false;
                        state.next_retry_at = None;
                    })
                    .await;
                info!(line_id = %binding.line_id, attempt, source, "VoLTE IMS restore registered");
                return;
            }
            Err(error) => {
                warn!(line_id = %binding.line_id, attempt, source, error = %error, "VoLTE IMS restore attempt failed");
                // A wedged baseband must not be retried. Re-issuing IMS PDP
                // activation against it can escalate to a modem subsystem
                // restart and take the whole device down, so stop the batch and
                // wait for an explicit operator retry instead.
                if crate::connectivity::modems::softstack::volte::plan::FailureClass::from_details(
                    error.detail().unwrap_or_default(),
                ) == crate::connectivity::modems::softstack::volte::plan::FailureClass::BasebandWedged
                {
                    warn!(
                        line_id = %binding.line_id,
                        error = %error,
                        "VoLTE IMS restore aborted: the baseband refused the session in a way that is unsafe to retry"
                    );
                    line.volte
                        .update(|state| {
                            state.phase = crate::connectivity::modems::softstack::volte::runtime::VoltePhase::Degraded;
                            state.recovery_state =
                                crate::connectivity::modems::softstack::volte::runtime::VolteRecoveryState::Exhausted;
                            state.manual_retry_available = true;
                            state.next_retry_at = None;
                            state.last_error = Some(format!("volte_baseband_wedged:{error}"));
                            state.last_failure_at = Some(chrono::Utc::now().to_rfc3339());
                        })
                        .await;
                    return;
                }
                if attempt < VOLTE_RESTORE_MAX_ATTEMPTS {
                    let delay = config.auto_restore_retry_delay_secs.clamp(5, 180);
                    line.volte
                        .update(|state| state.next_retry_at = Some(volte_next_retry_at(delay)))
                        .await;
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                }
            }
        }
    }

    line.volte
        .update(|state| {
            state.phase = crate::connectivity::modems::softstack::volte::runtime::VoltePhase::Degraded;
            state.recovery_state = crate::connectivity::modems::softstack::volte::runtime::VolteRecoveryState::Exhausted;
            state.manual_retry_available = true;
            state.next_retry_at = None;
            if state.last_error.is_none() {
                state.last_error = Some("volte_restore_attempts_exhausted".to_string());
            }
        })
        .await;
}

/// Keep explicitly enabled VoLTE lines alive, but stop after one bounded batch.
/// A later registered-session failure starts a fresh batch; an exhausted batch
/// waits for an explicit Web retry instead of looping forever.
pub fn spawn_volte_auto_restore(app: AppState) {
    tokio::spawn(async move {
        let initial = app.config_manager.get_volte_config();
        tokio::time::sleep(Duration::from_secs(
            initial.auto_restore_initial_delay_secs.clamp(5, 300),
        ))
        .await;
        loop {
            let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
            for profile in app
                .config_manager
                .get_line_profiles()
                .iter()
                .filter(|profile| profile.enabled && profile.volte_connection_enabled)
            {
                let Some(line) = app.line_registry.get(&profile.line_id).await else {
                    continue;
                };
                let status = line.volte.status().await;
                if status.registered
                    || status.manual_retry_available
                    || line.volte_retry_in_progress()
                {
                    continue;
                }
                start_line_volte_restore(app.clone(), line, "automatic").await;
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
}

pub async fn connect_vowifi_handler(
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VowifiStatusResponse>>) {
    if !app.config_manager.get_vowifi_config().feature_enabled {
        return (
            StatusCode::OK,
            Json(ApiResponse::<VowifiStatusResponse>::error(
                "Failed: vowifi_feature_disabled",
            )),
        );
    }
    if let Err(err) = app.config_manager.set_vowifi_connection_enabled(true) {
        return (
            StatusCode::OK,
            Json(ApiResponse::<VowifiStatusResponse>::error(format!(
                "Failed: {err}"
            ))),
        );
    }
    let status = connect_vowifi_with_attempts(
        &app,
        VOWIFI_MANUAL_CONNECT_ATTEMPTS,
        std::time::Duration::from_secs(VOWIFI_MANUAL_CONNECT_RETRY_DELAY_SECS),
        true,
    )
    .await;
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", status)),
    )
}

fn persist_vowifi_runtime_snapshot(app: &AppState, status: &VowifiStatusResponse) {
    let profile_meta = status.profile.profile.as_ref();
    if let Err(err) =
        app.database
            .upsert_vowifi_runtime_snapshot(crate::platform::db::NewVowifiRuntimeSnapshot {
                phase: status.phase,
                profile_id: profile_meta.map(|profile| profile.profile_id),
                plmn: profile_meta.map(|profile| profile.plmn),
                identity_ready: status.readiness.identity_ready,
                sim_auth_ready: status.readiness.sim_auth_ready,
                profile_matched: status.readiness.profile_matched,
                epdg_ready: status.readiness.epdg_ready,
                ike_ready: status.readiness.ike_ready,
                child_sa_ready: status.readiness.child_sa_ready,
                esp_ready: status.readiness.esp_ready,
                ims_registered: status.readiness.ims_registered,
                sms_ready: status.readiness.sms_ready,
                degraded_reason: status.degraded_reason.as_deref(),
            })
    {
        warn!(error = %err, "Failed to persist VoWiFi runtime snapshot");
    }
}

pub async fn get_vowifi_status_handler(
    Query(query): Query<VowifiStatusQuery>,
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VowifiStatusResponse>>) {
    let status = current_vowifi_status(&app, query.live.unwrap_or(true)).await;
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", status)),
    )
}

pub async fn get_vowifi_diagnostics_handler(
    Query(query): Query<VowifiListQuery>,
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VowifiDiagnosticsResponse>>) {
    let status = current_vowifi_status(&app, query.live.unwrap_or(true)).await;
    let trace_filter = query
        .trace_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let persisted_snapshot = match app.database.get_vowifi_runtime_snapshot() {
        Ok(snapshot) => snapshot,
        Err(err) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::error(format!("Failed: {}", err))),
            )
        }
    };
    let events = match app.database.get_vowifi_runtime_events_filtered(
        query.limit.unwrap_or(100),
        query.offset.unwrap_or(0),
        trace_filter.as_deref(),
    ) {
        Ok(events) => events,
        Err(err) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::error(format!("Failed: {}", err))),
            )
        }
    };
    let sms_deliveries = match app.database.get_vowifi_sms_deliveries(200, 0) {
        Ok(deliveries) => deliveries,
        Err(err) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::error(format!("Failed: {}", err))),
            )
        }
    };
    let soak_runs = match app.database.get_vowifi_soak_runs(20, 0) {
        Ok(runs) => runs,
        Err(err) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::error(format!("Failed: {}", err))),
            )
        }
    };
    let restore = match app.database.get_vowifi_esim_restore() {
        Ok(restore) => restore,
        Err(err) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::error(format!("Failed: {}", err))),
            )
        }
    };

    let diagnostics = vowifi_diagnostics::build_diagnostics_response(
        status,
        persisted_snapshot,
        events,
        sms_deliveries,
        soak_runs,
        restore,
        trace_filter,
    );

    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", diagnostics)),
    )
}

pub async fn get_vowifi_events_handler(
    Query(query): Query<VowifiListQuery>,
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VowifiRuntimeEventsResponse>>) {
    match app.database.get_vowifi_runtime_events_filtered(
        query.limit.unwrap_or(100),
        query.offset.unwrap_or(0),
        query.trace_id.as_deref(),
    ) {
        Ok(events) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", events)),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", err))),
        ),
    }
}

pub async fn get_vowifi_soak_runs_handler(
    Query(query): Query<VowifiListQuery>,
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VowifiSoakRunsResponse>>) {
    match app
        .database
        .get_vowifi_soak_runs(query.limit.unwrap_or(20), query.offset.unwrap_or(0))
    {
        Ok(runs) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", runs)),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", err))),
        ),
    }
}

pub async fn get_vowifi_sms_deliveries_handler(
    Query(query): Query<VowifiListQuery>,
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VowifiSmsDeliveriesResponse>>) {
    match app
        .database
        .get_vowifi_sms_deliveries(query.limit.unwrap_or(50), query.offset.unwrap_or(0))
    {
        Ok(deliveries) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", deliveries)),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", err))),
        ),
    }
}

pub async fn get_vowifi_sms_delivery_handler(
    Path(message_id): Path<String>,
    State(app): State<AppState>,
) -> (
    StatusCode,
    Json<ApiResponse<Option<crate::platform::db::VowifiSmsDeliveryEntry>>>,
) {
    match app.database.get_vowifi_sms_delivery(&message_id) {
        Ok(delivery) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", delivery)),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", err))),
        ),
    }
}

pub async fn get_vowifi_esim_restore_handler(
    State(app): State<AppState>,
) -> (
    StatusCode,
    Json<ApiResponse<Option<VowifiEsimRestoreEntry>>>,
) {
    match app.database.get_vowifi_esim_restore() {
        Ok(restore) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", restore)),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", err))),
        ),
    }
}

pub async fn get_voice_path_policy_handler(
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VoicePathPolicy>>) {
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            app.config_manager.get_voice_path_policy(),
        )),
    )
}

pub async fn set_voice_path_policy_handler(
    State(app): State<AppState>,
    Json(payload): Json<VoicePathPolicy>,
) -> (StatusCode, Json<ApiResponse<VoicePathPolicy>>) {
    match app.config_manager.set_voice_path_policy(payload) {
        Ok(policy) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Saved", policy)),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {err}"))),
        ),
    }
}

pub async fn get_web_call_capabilities_handler(
) -> (StatusCode, Json<ApiResponse<WebCallCapabilitiesResponse>>) {
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            WebCallCapabilitiesResponse {
                available: false,
                control_plane_ready: true,
                ingress: MediaIngressCapabilities::unwired(),
                recommended_adapter: "browser_webrtc_gateway".to_string(),
                required_media_security: vec![
                    "wss".to_string(),
                    "webrtc_dtls_srtp".to_string(),
                    "ice".to_string(),
                    "short_lived_session_token".to_string(),
                ],
                note: "网页可作为独立内部话机，但浏览器不能直接收发 IMS SIP/RTP；需先接入可插拔 WebRTC 媒体网关。".to_string(),
            },
        )),
    )
}

pub(crate) fn temperature_sensor_label(sensor_type: &str, zone: &str) -> String {
    let source = if sensor_type.trim().is_empty() {
        if zone.trim().is_empty() {
            "unknown"
        } else {
            zone.trim()
        }
    } else {
        sensor_type.trim()
    };
    let normalized = source.to_ascii_lowercase().replace('_', "-");

    if ["modem", "baseband", "wwan", "qmi", "mhi"]
        .iter()
        .any(|pattern| normalized.contains(pattern))
    {
        return "基带".to_string();
    }
    if ["gpu", "adreno"]
        .iter()
        .any(|pattern| normalized.contains(pattern))
    {
        return "GPU".to_string();
    }
    if ["camera", "cam", "isp"]
        .iter()
        .any(|pattern| normalized.contains(pattern))
    {
        return "摄像头".to_string();
    }
    if ["wifi", "wlan"]
        .iter()
        .any(|pattern| normalized.contains(pattern))
    {
        return "Wi-Fi".to_string();
    }
    if ["battery", "batt"]
        .iter()
        .any(|pattern| normalized.contains(pattern))
    {
        return "电池".to_string();
    }
    if ["charger", "charge"]
        .iter()
        .any(|pattern| normalized.contains(pattern))
    {
        return "充电".to_string();
    }
    if ["pmic", "power"]
        .iter()
        .any(|pattern| normalized.contains(pattern))
    {
        return "电源管理".to_string();
    }
    if ["soc", "tsens"]
        .iter()
        .any(|pattern| normalized.contains(pattern))
    {
        return "SoC".to_string();
    }
    if ["skin", "shell", "case"]
        .iter()
        .any(|pattern| normalized.contains(pattern))
    {
        return "外壳".to_string();
    }
    if ["ambient", "board"]
        .iter()
        .any(|pattern| normalized.contains(pattern))
    {
        return "环境".to_string();
    }

    if let Some((first, second)) = extract_number_range_after(&normalized, "cpu") {
        return second
            .map(|second| format!("CPU {first}-{second}"))
            .unwrap_or_else(|| format!("CPU {first}"));
    }
    if normalized.contains("cpu") {
        return "CPU".to_string();
    }

    if let Some((first, second)) = extract_number_range_after(&normalized, "core") {
        return second
            .map(|second| format!("核心 {first}-{second}"))
            .unwrap_or_else(|| format!("核心 {first}"));
    }
    if normalized.contains("core") {
        return "核心".to_string();
    }

    let cleaned = source
        .replace(['-', '_', ' '], " ")
        .split_whitespace()
        .filter(|part| {
            !matches!(
                part.to_ascii_lowercase().as_str(),
                "thermal" | "therm" | "temperature" | "temp" | "sensor" | "zone"
            )
        })
        .collect::<Vec<_>>()
        .join(" ");

    if cleaned.is_empty() {
        source.to_string()
    } else {
        cleaned
    }
}

fn extract_number_range_after(value: &str, prefix: &str) -> Option<(String, Option<String>)> {
    let start = value.find(prefix)? + prefix.len();
    let chars = value[start..].char_indices();
    let mut first_start = None;
    for (index, ch) in chars {
        if ch.is_ascii_digit() {
            first_start = Some(start + index);
            break;
        }
    }
    let first_start = first_start?;
    let first_end = value[first_start..]
        .char_indices()
        .find_map(|(index, ch)| (!ch.is_ascii_digit()).then_some(first_start + index))
        .unwrap_or(value.len());
    let first = value[first_start..first_end].to_string();

    let after_first = &value[first_end..];
    let mut second_start = None;
    for (index, ch) in after_first.char_indices() {
        if ch.is_ascii_digit() {
            second_start = Some(first_end + index);
            break;
        }
        if ch.is_ascii_alphabetic() {
            break;
        }
    }
    let Some(second_start) = second_start else {
        return Some((first, None));
    };
    let second_end = value[second_start..]
        .char_indices()
        .find_map(|(index, ch)| (!ch.is_ascii_digit()).then_some(second_start + index))
        .unwrap_or(value.len());
    Some((first, Some(value[second_start..second_end].to_string())))
}

pub(crate) fn read_temperature_sensors() -> Vec<ThermalZone> {
    use std::fs;
    use std::path::Path;

    let thermal_path = Path::new("/sys/class/thermal");
    let mut sensors = Vec::new();

    if let Ok(entries) = fs::read_dir(thermal_path) {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();

            if name.starts_with("thermal_zone") {
                let zone_path = entry.path();
                let sensor_type = fs::read_to_string(zone_path.join("type"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                let temperature = fs::read_to_string(zone_path.join("temp"))
                    .ok()
                    .and_then(|s| s.trim().parse::<i32>().ok())
                    .map(|t| t as f64 / 1000.0)
                    .unwrap_or(0.0);

                let label = temperature_sensor_label(&sensor_type, &name);
                sensors.push(ThermalZone {
                    zone: name.to_string(),
                    sensor_type,
                    label,
                    temperature,
                });
            }
        }
    }
    sensors.sort_by(|a, b| a.zone.cmp(&b.zone));
    sensors
}

/// GET /api/stats
pub async fn get_system_stats(State(dbus_conn): State<Arc<Connection>>) -> impl IntoResponse {
    let result: Result<SystemStatsResponse, String> = async {
        let interfaces =
            get_active_interfaces().map_err(|e| format!("Failed to get interfaces: {}", e))?;

        let mut initial: Vec<(String, u64, u64)> = Vec::new();
        for iface in &interfaces {
            if let Ok((rx, tx)) = read_interface_stats(iface, Some(&dbus_conn)).await {
                initial.push((iface.clone(), rx, tx));
            }
        }

        // 并行执行 CPU 采样 (200ms) 和网速采样间隔 (1000ms)，节省 200ms
        let (cpu_usage, _) = tokio::join!(
            async { sample_cpu_usage().await.unwrap_or(0.0) },
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)),
        );

        let mut speed_data = Vec::new();
        let elapsed = 1.0_f64;
        for (interface, rx1, tx1) in &initial {
            if let Ok((rx2, tx2)) = read_interface_stats(interface, Some(&dbus_conn)).await {
                let rx_speed = rx2.saturating_sub(*rx1);
                let tx_speed = tx2.saturating_sub(*tx1);
                speed_data.push(NetworkSpeed {
                    interface: interface.clone(),
                    rx_bytes_per_sec: rx_speed,
                    tx_bytes_per_sec: tx_speed,
                    total_rx_bytes: rx2,
                    total_tx_bytes: tx2,
                });
            }
        }

        let (total, available, cached, buffers) = read_memory_info()?;
        let used = total.saturating_sub(available);
        let used_percent = if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        let disk = read_disk_info();
        let mut cpu_load = read_cpu_load_sync().unwrap_or_default();
        cpu_load.load_percent = cpu_usage;
        let (uptime, idle) = read_uptime()?;
        let formatted = format_uptime(uptime);
        let system_info = read_system_info()?;
        let temperature = read_temperature_sensors();

        Ok(SystemStatsResponse {
            network_speed: NetworkSpeedResponse {
                interfaces: speed_data,
                interval_seconds: elapsed,
            },
            memory: MemoryInfo {
                total_bytes: total,
                available_bytes: available,
                used_bytes: used,
                used_percent,
                cached_bytes: cached,
                buffers_bytes: buffers,
            },
            disk,
            cpu_load,
            uptime: UptimeInfo {
                uptime_seconds: uptime,
                idle_seconds: idle,
                uptime_formatted: formatted,
            },
            system_info,
            temperature,
        })
    }
    .await;

    match result {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(msg) => (
            StatusCode::OK,
            Json(ApiResponse::<SystemStatsResponse>::error(msg)),
        ),
    }
}

/// GET /api/stats/cpu
pub async fn get_cpu_info() -> impl IntoResponse {
    match read_cpu_info() {
        Ok(info) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", info)),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<CpuInfo>::error(format!("Failed: {}", e))),
        ),
    }
}

/// GET /api/connectivity
pub async fn get_connectivity_check() -> (StatusCode, Json<ApiResponse<ConnectivityCheckResponse>>)
{
    // 两个 ping 并行执行，超时从 2s 缩短到 1s
    let (ipv4_result, ipv6_result) = tokio::join!(
        async_ping_host("223.5.5.5", false),
        async_ping_host("2400:3200::1", true),
    );
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Connectivity check completed",
            ConnectivityCheckResponse {
                ipv4: ipv4_result,
                ipv6: ipv6_result,
            },
        )),
    )
}

pub(crate) async fn async_ping_host(target: &str, is_ipv6: bool) -> PingResult {
    let cmd = if is_ipv6 { "ping6" } else { "ping" };
    let output = tokio::process::Command::new(cmd)
        .args(["-c", "1", "-W", "1", target])
        .output()
        .await;
    match output {
        Ok(result) => {
            if result.status.success() {
                let stdout = String::from_utf8_lossy(&result.stdout);
                let latency = parse_ping_latency(&stdout);
                PingResult {
                    success: true,
                    latency_ms: latency,
                    target: target.to_string(),
                    error: None,
                }
            } else {
                let stderr = String::from_utf8_lossy(&result.stderr);
                PingResult {
                    success: false,
                    latency_ms: None,
                    target: target.to_string(),
                    error: Some(if stderr.is_empty() {
                        "Unreachable".to_string()
                    } else {
                        stderr.trim().to_string()
                    }),
                }
            }
        }
        Err(e) => PingResult {
            success: false,
            latency_ms: None,
            target: target.to_string(),
            error: Some(format!("Failed: {}", e)),
        },
    }
}

fn parse_ping_latency(output: &str) -> Option<f64> {
    for line in output.lines() {
        if let Some(time_pos) = line.find("time=") {
            let after_time = &line[time_pos + 5..];
            let num_str: String = after_time
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(latency) = num_str.parse::<f64>() {
                return Some(latency);
            }
        }
    }
    None
}

/// POST /api/system/reboot
pub async fn system_reboot(
    State(app): State<AppState>,
    Json(payload): Json<SystemRebootRequest>,
) -> impl IntoResponse {
    let delay = payload.delay_seconds;
    app.system_event_emitter
        .emit_code(
            system_event_codes::SYSTEM_SERVICE_REBOOT_REQUESTED,
            system_event_severity::WARNING,
            system_event_status::TRIGGERED,
            "system",
            format!("用户触发系统重启，延迟 {} 秒执行", delay),
        )
        .await;
    let system_events = Arc::clone(&app.system_event_emitter);
    tokio::spawn(async move {
        run_safe_os_reboot_sequence(delay, system_events).await;
    });
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            format!("System will perform safe OS reboot in {} seconds", delay),
            json!({ "delay_seconds": delay }),
        )),
    )
}

pub async fn run_safe_os_reboot_sequence(
    delay_seconds: u32,
    system_events: Arc<crate::services::system::system_event::SystemEventEmitter>,
) {
    if delay_seconds > 0 {
        tokio::time::sleep(tokio::time::Duration::from_secs(delay_seconds as u64)).await;
    }

    info!("Starting safe OS reboot sequence");

    if let Some(message) =
        run_reboot_prep_command("disable modem radio", "mmcli", &["-m", "0", "-d"], false)
    {
        system_events
            .emit_code(
                system_event_codes::SYSTEM_SERVICE_REBOOT_PREP_FAILED,
                system_event_severity::WARNING,
                system_event_status::FAILED,
                "disable modem radio",
                message,
            )
            .await;
    }
    if let Some(message) = run_reboot_prep_command(
        "stop ModemManager IPC service",
        "systemctl",
        &["stop", "ModemManager"],
        false,
    ) {
        system_events
            .emit_code(
                system_event_codes::SYSTEM_SERVICE_REBOOT_PREP_FAILED,
                system_event_severity::WARNING,
                system_event_status::FAILED,
                "stop ModemManager IPC service",
                message,
            )
            .await;
    }
    let _ = run_reboot_prep_command("stop qmi-proxy", "killall", &["qmi-proxy"], true);
    cleanup_modemmanager_runtime_cache();
    if let Some(message) = run_reboot_prep_command("flush filesystem cache", "sync", &[], false) {
        system_events
            .emit_code(
                system_event_codes::SYSTEM_SERVICE_REBOOT_PREP_FAILED,
                system_event_severity::WARNING,
                system_event_status::FAILED,
                "flush filesystem cache",
                message,
            )
            .await;
    }

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    info!("Safe OS reboot preparation complete, executing reboot");
    if let Err(err) = Command::new("reboot").output() {
        error!(error = %err, "Failed to execute reboot command");
    }
}

fn run_reboot_prep_command(
    label: &str,
    program: &str,
    args: &[&str],
    allow_failure: bool,
) -> Option<String> {
    match Command::new(program).args(args).output() {
        Ok(output) if output.status.success() => {
            info!(step = label, "Safe OS reboot step completed");
            None
        }
        Ok(output) => {
            let severity = if allow_failure {
                "optional"
            } else {
                "required"
            };
            warn_reboot_prep_failure(label, program, severity, &output);
            if allow_failure {
                None
            } else {
                Some(format!(
                    "重启预处理步骤失败: {label}; command={program}; status={}",
                    output.status
                ))
            }
        }
        Err(err) if allow_failure => {
            warn!(step = label, command = program, error = %err, "Optional safe OS reboot step failed");
            None
        }
        Err(err) => {
            warn!(step = label, command = program, error = %err, "Safe OS reboot step failed");
            Some(format!(
                "重启预处理步骤失败: {label}; command={program}; error={err}"
            ))
        }
    }
}

fn cleanup_modemmanager_runtime_cache() {
    const CACHE_DIR: &str = "/var/lib/ModemManager";

    match fs::read_dir(CACHE_DIR) {
        Ok(entries) => {
            let mut removed = 0usize;
            for entry in entries {
                match entry {
                    Ok(entry) => {
                        let path = entry.path();
                        let result = if path.is_dir() {
                            fs::remove_dir_all(&path)
                        } else {
                            fs::remove_file(&path)
                        };

                        match result {
                            Ok(()) => removed += 1,
                            Err(err) => warn!(
                                path = %path.display(),
                                error = %err,
                                "Failed to remove ModemManager runtime cache entry"
                            ),
                        }
                    }
                    Err(err) => warn!(
                        directory = CACHE_DIR,
                        error = %err,
                        "Failed to read ModemManager runtime cache entry"
                    ),
                }
            }
            info!(
                directory = CACHE_DIR,
                removed, "ModemManager runtime cache cleanup completed"
            );
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            info!(
                directory = CACHE_DIR,
                "ModemManager runtime cache directory does not exist"
            );
        }
        Err(err) => {
            warn!(
                directory = CACHE_DIR,
                error = %err,
                "Failed to open ModemManager runtime cache directory"
            );
        }
    }
}

fn warn_reboot_prep_failure(label: &str, program: &str, severity: &str, output: &Output) {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    warn!(
        step = label,
        command = program,
        severity = severity,
        status = %output.status,
        stderr = %stderr,
        stdout = %stdout,
        "Safe OS reboot step returned non-zero status"
    );
}

// ============ 通知配置 ============

pub async fn restart_service_handler(State(app): State<AppState>) -> impl IntoResponse {
    app.system_event_emitter
        .emit_code(
            system_event_codes::SYSTEM_SERVICE_SIMADMIN_RESTART_REQUESTED,
            system_event_severity::WARNING,
            system_event_status::TRIGGERED,
            "simadmin",
            "用户触发 SimAdmin 服务重启",
        )
        .await;
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        let _ = Command::new("systemctl")
            .args(["restart", "simadmin"])
            .output();
    });
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "SimAdmin service will restart",
            json!({}),
        )),
    )
}

use crate::platform::config::ConfigManager;
use crate::services::notify::notification::NotificationSender;

#[derive(Debug, Default, Deserialize)]
pub struct NotificationLogQuery {
    #[serde(default, rename = "type")]
    pub event_type: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub start_date: String,
    #[serde(default)]
    pub end_date: String,
    #[serde(default = "default_notification_log_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

#[derive(Debug, Default, Deserialize)]
pub struct NotificationLogClearRequest {
    #[serde(default, rename = "type")]
    pub event_type: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub start_date: String,
    #[serde(default)]
    pub end_date: String,
}

fn default_notification_log_limit() -> i64 {
    50
}

/// GET /api/notifications/config
pub async fn get_notification_config_handler(
    State(config_manager): State<Arc<ConfigManager>>,
) -> (
    StatusCode,
    Json<ApiResponse<crate::platform::config::NotificationConfig>>,
) {
    let config = config_manager.get_notifications();
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", config)),
    )
}

/// POST /api/notifications/config
pub async fn set_notification_config_handler(
    State(config_manager): State<Arc<ConfigManager>>,
    Json(notification_config): Json<crate::platform::config::NotificationConfig>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    match config_manager.set_notifications(notification_config) {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Notification config updated",
                json!({}),
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", e))),
        ),
    }
}

/// POST /api/notifications/test/{channel}
pub async fn test_notification_channel_handler(
    Path(channel): Path<String>,
    State(notification_sender): State<Arc<NotificationSender>>,
) -> (
    StatusCode,
    Json<ApiResponse<crate::api::models::WebhookTestResponse>>,
) {
    match notification_sender.test_channel(&channel).await {
        Ok(message) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Notification test successful",
                WebhookTestResponse {
                    success: true,
                    message,
                },
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Notification test failed",
                WebhookTestResponse {
                    success: false,
                    message: e,
                },
            )),
        ),
    }
}

// ============ OTA 更新 ============

/// GET /api/notifications/logs
pub async fn get_notification_logs_handler(
    Query(query): Query<NotificationLogQuery>,
    State(database): State<Arc<Database>>,
) -> (
    StatusCode,
    Json<ApiResponse<crate::platform::db::NotificationLogsResponse>>,
) {
    match database.get_notification_logs(
        &query.event_type,
        &query.status,
        &query.q,
        &query.start_date,
        &query.end_date,
        query.limit,
        query.offset,
    ) {
        Ok(logs) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", logs)),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", err))),
        ),
    }
}

/// POST /api/notifications/logs/clear
pub async fn clear_notification_logs_handler(
    State(database): State<Arc<Database>>,
    payload: Option<Json<NotificationLogClearRequest>>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let filters = payload.map(|Json(value)| value).unwrap_or_default();
    match database.clear_notification_logs(
        &filters.event_type,
        &filters.status,
        &filters.start_date,
        &filters.end_date,
    ) {
        Ok(deleted) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Notification logs cleared",
                json!({ "deleted": deleted }),
            )),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", err))),
        ),
    }
}

/// GET /api/ota/status
pub async fn get_ota_status_handler() -> impl IntoResponse {
    let status = crate::services::system::ota::get_ota_status();
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", status)),
    )
}

/// POST /api/ota/upload
pub async fn upload_ota_handler(body: axum::body::Bytes) -> impl IntoResponse {
    match crate::services::system::ota::handle_ota_upload(&body) {
        Ok(response) => {
            let message = if response.validation.valid {
                "OTA uploaded and validated"
            } else {
                "OTA uploaded but validation failed"
            };
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(message, response)),
            )
        }
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<crate::api::models::OtaUploadResponse>::error(
                format!("Failed: {}", e),
            )),
        ),
    }
}

/// POST /api/ota/latest-release
pub async fn get_latest_ota_release_handler(
    Json(req): Json<crate::api::models::OtaOnlinePrepareRequest>,
) -> impl IntoResponse {
    let result: Result<crate::api::models::OtaLatestReleaseResponse, String> = async {
        let include_builtin_proxies = req
            .proxy_prefix
            .as_ref()
            .map(|prefix| !prefix.trim().is_empty())
            .unwrap_or(false);
        let proxy_prefix = crate::services::system::ota::normalize_proxy_prefix(req.proxy_prefix);
        let client = crate::services::system::ota::build_ota_http_client()?;

        crate::services::system::ota::fetch_latest_github_release(
            &client,
            &proxy_prefix,
            include_builtin_proxies,
        )
        .await
    }
    .await;

    match result {
        Ok(release) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", release)),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<crate::api::models::OtaLatestReleaseResponse>::error(format!(
                "Failed: {}. GitHub may have rate-limited this request; try again later or enable a proxy.",
                e
            ))),
        ),
    }
}

/// POST /api/ota/online-prepare
pub async fn prepare_online_ota_handler(
    Json(req): Json<crate::api::models::OtaOnlinePrepareRequest>,
) -> impl IntoResponse {
    let result: Result<crate::api::models::OtaUploadResponse, String> = async {
        let include_builtin_proxies = req
            .proxy_prefix
            .as_ref()
            .map(|prefix| !prefix.trim().is_empty())
            .unwrap_or(false);
        let proxy_prefix = crate::services::system::ota::normalize_proxy_prefix(req.proxy_prefix);
        let client = crate::services::system::ota::build_ota_http_client()?;

        let release = crate::services::system::ota::fetch_latest_github_release(
            &client,
            &proxy_prefix,
            include_builtin_proxies,
        )
        .await?;

        let asset = crate::services::system::ota::supported_release_asset(&release)
            .ok_or_else(|| "No supported OTA asset found in latest release".to_string())?;

        if asset.size > crate::services::system::ota::MAX_OTA_BYTES {
            return Err(format!(
                "OTA asset is too large: {} bytes exceeds {} bytes",
                asset.size,
                crate::services::system::ota::MAX_OTA_BYTES
            ));
        }

        let bytes = crate::services::system::ota::download_ota_asset_bytes(
            &client,
            &proxy_prefix,
            include_builtin_proxies,
            asset,
        )
        .await?;

        crate::services::system::ota::handle_ota_upload(&bytes)
    }
    .await;

    match result {
        Ok(response) => {
            let message = if response.validation.valid {
                "Online OTA downloaded and validated"
            } else {
                "Online OTA downloaded but validation failed"
            };
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(message, response)),
            )
        }
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<crate::api::models::OtaUploadResponse>::error(
                format!("Failed: {}", e),
            )),
        ),
    }
}

/// POST /api/ota/apply
pub async fn apply_ota_handler(
    Json(req): Json<crate::api::models::OtaApplyRequest>,
) -> impl IntoResponse {
    match crate::services::system::ota::apply_ota_update(req.restart_now) {
        Ok(message) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                &message,
                json!({ "applied": true }),
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// POST /api/ota/cancel
pub async fn cancel_ota_handler() -> impl IntoResponse {
    match crate::services::system::ota::cancel_pending_update() {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Update cancelled",
                json!({}),
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

fn default_log_limit() -> i64 {
    100
}

#[derive(Debug, Deserialize)]
pub struct AutomationLogQuery {
    #[serde(default, rename = "type")]
    pub task_type: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub start_date: String,
    #[serde(default)]
    pub end_date: String,
    #[serde(default = "default_log_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

#[derive(Debug, Deserialize, Default)]
pub struct AutomationLogClearRequest {
    #[serde(default, rename = "type")]
    pub task_type: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub start_date: String,
    #[serde(default)]
    pub end_date: String,
}

/// GET /api/automation/config
pub async fn get_automation_config_handler(
    State(config_manager): State<Arc<ConfigManager>>,
) -> (
    StatusCode,
    Json<ApiResponse<crate::platform::config::AutomationConfig>>,
) {
    let config = config_manager.get_automation_config();
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", config)),
    )
}

/// POST /api/automation/config
pub async fn set_automation_config_handler(
    State(config_manager): State<Arc<ConfigManager>>,
    Json(config): Json<crate::platform::config::AutomationConfig>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    match config_manager.set_automation_config(config) {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Automation config updated",
                json!({}),
            )),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", e))),
        ),
    }
}

/// GET /api/automation/logs
pub async fn get_automation_logs_handler(
    Query(query): Query<AutomationLogQuery>,
    State(database): State<Arc<Database>>,
) -> (
    StatusCode,
    Json<ApiResponse<crate::platform::db::AutomationLogsResponse>>,
) {
    match database.get_automation_logs(
        &query.task_type,
        &query.status,
        &query.q,
        &query.start_date,
        &query.end_date,
        query.limit,
        query.offset,
    ) {
        Ok(logs) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", logs)),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", err))),
        ),
    }
}

/// POST /api/automation/logs/clear
pub async fn clear_automation_logs_handler(
    State(database): State<Arc<Database>>,
    payload: Option<Json<AutomationLogClearRequest>>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let filters = payload.map(|Json(value)| value).unwrap_or_default();
    match database.clear_automation_logs(
        &filters.task_type,
        &filters.status,
        &filters.start_date,
        &filters.end_date,
    ) {
        Ok(deleted) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Automation logs cleared",
                json!({ "deleted": deleted }),
            )),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", err))),
        ),
    }
}

/// POST /api/automation/test/{task_id}
pub async fn test_automation_task_handler(
    Path(task_id): Path<String>,
    State(app_state): State<AppState>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let config = app_state.config_manager.get_automation_config();
    let task = config.tasks.iter().find(|t| t.id == task_id).cloned();

    let Some(task) = task else {
        return (StatusCode::OK, Json(ApiResponse::error("自动化任务不存在")));
    };

    tokio::spawn(async move {
        let registry = crate::services::automation::tasks::TaskRegistry::new();
        let task_type = match &task.action {
            crate::platform::config::AutomationAction::RestartBaseband => "restart_baseband",
            crate::platform::config::AutomationAction::RebootDevice { .. } => "reboot_device",
            crate::platform::config::AutomationAction::SendSms { .. } => "send_sms",
            crate::platform::config::AutomationAction::ConsumeData { .. } => "consume_data",
            crate::platform::config::AutomationAction::DialCall { .. } => "dial_call",
        };

        let handler = match registry.get(task_type) {
            Some(h) => h,
            None => {
                let err_msg = format!("未找到该任务类型的处理器: {}", task_type);
                let _ = app_state
                    .database
                    .insert_automation_log(&task.id, &task.name, task_type, "failed", &err_msg);
                return;
            }
        };

        let mut delay_secs = 0u64;
        let params = match &task.action {
            crate::platform::config::AutomationAction::RestartBaseband => serde_json::Value::Null,
            crate::platform::config::AutomationAction::RebootDevice { delay_seconds } => {
                serde_json::json!({ "delay_seconds": delay_seconds })
            }
            crate::platform::config::AutomationAction::SendSms {
                phone_number,
                content,
                random_delay_seconds,
                retry_limit,
            } => {
                delay_secs = u64::from(random_delay_seconds.unwrap_or(0));
                serde_json::json!({
                    "phone_number": phone_number,
                    "content": content,
                    "random_delay_seconds": random_delay_seconds,
                    "retry_limit": retry_limit
                })
            }
            crate::platform::config::AutomationAction::ConsumeData { bytes, unit } => {
                delay_secs = 120;
                serde_json::json!({ "bytes": bytes, "unit": unit, "target": &task.target })
            }
            crate::platform::config::AutomationAction::DialCall {
                country_code,
                phone_number,
                duration_seconds,
            } => {
                delay_secs = u64::from(*duration_seconds).min(7_200);
                serde_json::json!({
                    "country_code": country_code,
                    "phone_number": phone_number,
                    "duration_seconds": duration_seconds,
                    "target": &task.target,
                })
            }
        };

        let params = if params.get("target").is_some() {
            params
        } else {
            let mut params = params;
            if let Some(target) = &task.target {
                if let Ok(value) = serde_json::to_value(target) {
                    params["target"] = value;
                }
            }
            params
        };

        let result = tokio::time::timeout(
            tokio::time::Duration::from_secs(60 + delay_secs),
            handler.execute(&app_state, &params),
        )
        .await;

        let (status, detail) = match result {
            Ok(Ok(_)) => ("success", "执行成功".to_string()),
            Ok(Err(e)) => ("failed", format!("执行失败: {}", e)),
            Err(_) => ("failed", "执行超时 (超过60秒限制)".to_string()),
        };

        let _ = app_state
            .database
            .insert_automation_log(&task.id, &task.name, task_type, status, &detail);

        let event = crate::services::notify::notification::AutomationEvent {
            task_id: task.id.clone(),
            task_name: task.name.clone(),
            task_type: task_type.to_string(),
            status: status.to_string(),
            message: detail.clone(),
            timestamp: crate::platform::db::beijing_sms_now_string(),
        };

        let _ = app_state
            .notification_sender
            .forward_automation_event(&event)
            .await;
    });

    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "任务已在后台下发立即执行",
            json!({}),
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::cellular::modem_manager::SimIdentity;

    #[test]
    fn enriches_enabled_esim_profile_from_current_sim_identity() {
        let mut profiles = vec![
            EsimProfile {
                iccid: "profile-a".to_string(),
                state: "disabled".to_string(),
                ..Default::default()
            },
            EsimProfile {
                iccid: "profile-b".to_string(),
                state: "disabled".to_string(),
                ..Default::default()
            },
        ];
        let identity = SimIdentity {
            iccid: "profile-b".to_string(),
            imsi: "234336".to_string(),
            operator_id: "234336".to_string(),
        };

        enrich_profiles_with_current_identity(&mut profiles, &identity);

        assert_eq!(profiles[1].state, "enabled");
        assert_eq!(profiles[1].imsi.as_deref(), Some("234336"));
        assert_eq!(profiles[1].mcc.as_deref(), Some("234"));
        assert_eq!(profiles[1].mnc.as_deref(), Some("336"));
        assert!(profiles[0].mcc.is_none());
    }

    #[test]
    fn splits_five_digit_operator_codes_for_profile_enrichment() {
        assert_eq!(
            split_profile_operator_code("46002"),
            ("460".to_string(), "02".to_string())
        );
    }

    #[test]
    fn labels_temperature_sensors_with_dashboard_names() {
        assert_eq!(temperature_sensor_label("modem-thermal", ""), "基带");
        assert_eq!(temperature_sensor_label("cpu0-1-thermal", ""), "CPU 0-1");
        assert_eq!(temperature_sensor_label("core2_3_temp", ""), "核心 2-3");
        assert_eq!(temperature_sensor_label("wifi_sensor", ""), "Wi-Fi");
    }

    #[test]
    fn vowifi_mt_storage_key_preserves_repeated_identical_replies() {
        let first = crate::connectivity::modems::softstack::vowifi::sms::MoSmsSipOutcome {
            trace_id: "trace-a".to_string(),
            message_id: "mo-a".to_string(),
            sip_status: 202,
            rpdu_ack: crate::connectivity::modems::softstack::vowifi::sms::RpduAckState::None,
            delivery_state: crate::connectivity::modems::softstack::vowifi::sms::SmsDeliveryState::Accepted,
            failure_cause: None,
            mt_deliveries: Vec::new(),
        };
        let mut second = first.clone();
        second.trace_id = "trace-b".to_string();
        second.message_id = "mo-b".to_string();

        let first_key = vowifi_mt_storage_key(&first, "10086", "You don't have any credit balance");
        let second_key =
            vowifi_mt_storage_key(&second, "10086", "You don't have any credit balance");

        assert_ne!(first_key, second_key);
    }

    #[test]
    fn vowifi_mt_complete_group_count_collapses_segments() {
        let outcome = crate::connectivity::modems::softstack::vowifi::sms::MoSmsSipOutcome {
            trace_id: "trace-a".to_string(),
            message_id: "mo-a".to_string(),
            sip_status: 202,
            rpdu_ack: crate::connectivity::modems::softstack::vowifi::sms::RpduAckState::None,
            delivery_state: crate::connectivity::modems::softstack::vowifi::sms::SmsDeliveryState::Accepted,
            failure_cause: None,
            mt_deliveries: vec![
                crate::connectivity::modems::softstack::vowifi::sms::MtSmsDeliver {
                    rp_message_reference: 1,
                    originator: "10086".to_string(),
                    text: "part1".to_string(),
                    user_data_bytes: 5,
                    service_center_timestamp: "2026-06-22 13:13:59".to_string(),
                    segment_reference: Some(7),
                    segment_sequence: 1,
                    segment_total: 2,
                },
                crate::connectivity::modems::softstack::vowifi::sms::MtSmsDeliver {
                    rp_message_reference: 2,
                    originator: "10086".to_string(),
                    text: "part2".to_string(),
                    user_data_bytes: 5,
                    service_center_timestamp: "2026-06-22 13:13:59".to_string(),
                    segment_reference: Some(7),
                    segment_sequence: 2,
                    segment_total: 2,
                },
            ],
        };

        assert_eq!(vowifi_mt_complete_group_count(&outcome), 1);
    }
}
