//! API 处理器模块 (ModemManager 版)
//!
//! 包含所有 HTTP API 的处理函数
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    Json,
};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::fs;
use std::process::{Command, Output};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};
use zbus::Connection;

use crate::{
    api::models::*,
    connectivity::modems::ims::vowifi::diagnostics::{
        self as vowifi_diagnostics, VowifiDiagnosticsResponse, VowifiProfilesResponse,
        VowifiStatusResponse,
    },
    connectivity::modems::ims::vowifi::restore::RestorePhase,
    connectivity::modems::ims::vowifi::{
        live::{
            clear_live_runtime_for_line, live_xcap_access_for_line, place_live_voice_call_for_line,
            send_live_sms_over_ims_for_line, verify_live_sim_auth_access_for_line,
        },
        sms::{MoSmsSipOutcome, MtSmsDeliver},
    },
    hardware::cellular::modem_manager,
    hardware::cellular::modem_manager::{
        answer_call_on_modem, get_band_lock_status_for_modem,
        get_baseband_restart_progress_for_line, get_call_by_path_for_modem,
        get_call_settings_for_modem, get_cell_location_for_modem, get_cells_data_for_modem,
        get_device_info_for_modem, get_is_roaming_for_modem, get_network_info_for_modem,
        get_operators_list_for_modem, get_radio_mode_for_modem, get_signal_strength_for_modem,
        get_sim_info_for_modem_with_cache, hangup_all_calls_for_modem, hangup_call_on_modem,
        list_current_calls_for_modem, make_call_on_modem,
        power_cycle_sim_for_profile_switch_via_modem, recover_absent_baseband_via_qmi,
        register_operator_for_modem, request_operator_registration_for_modem,
        restart_baseband_via_modem, scan_operators_for_modem, send_call_dtmf_on_modem,
        send_sms_via_modem, set_band_lock_for_modem, set_call_waiting_for_modem,
        set_radio_mode_for_modem, sim_identity_for_modem, start_cell_monitoring_for_modem,
        stop_cell_monitoring_for_modem,
    },
    hardware::sim::esim::EsimApiError,
    platform::config::{
        AccessPathKind, AutoRestoreConfig, EsimReaderConfig, GithubDownloadProxyConfig,
        ImsVideoConfig, LineDataProxyConfig, LineProfileConfig, LineVowifiConfig, SmsPathPolicy,
        StandaloneSimSlotConfig, TrunkProfileConfig, VoicePathPolicy,
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
    services::system::system_event::{
        codes as system_event_codes, mask_identifier, severity as system_event_severity,
        status as system_event_status,
    },
    state::AppState,
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
const VOLTE_MODEM_MISSING_POLLS: u32 = 6;
const VOLTE_MODEM_MISSING_POLL_DELAY_SECS: u64 = 5;
const VOWIFI_RESTORE_IDENTITY_GATE_DELAY_SECS: u64 = 2;
const VOWIFI_PROFILE_SWITCH_CONNECT_ATTEMPTS: u8 = 2;
const VOWIFI_PROFILE_SWITCH_CONNECT_RETRY_DELAY_SECS: u64 = 1;
const SMS_DB_MAINTENANCE_DELETE_THRESHOLD: usize = 100;
const SMS_DB_MAINTENANCE_DELAY_SECS: u64 = 60;
const LINE_DATA_WATCHDOG_INTERVAL_SECS: u64 = 15;
const LINE_DATA_REGISTER_THRESHOLD: u32 = 4;
const LINE_DATA_REGISTER_COOLDOWN_SECS: u64 = 120;
const LINE_DATA_CONNECT_COOLDOWN_SECS: u64 = 60;
const CALL_MONITOR_INTERVAL_SECS: u64 = 2;
const CALL_END_MISSING_POLLS: u8 = 2;
const DEFAULT_CARRIER_CATALOG_URL: &str = "https://github.com/autisticryptic/carrier_Bundles/releases/download/v0.3.0-catalog-v7/carrier-bundles-pixel-mustang.sqlite3";
const ALLOWED_CARRIER_CATALOG_URLS: &[&str] = &[
    DEFAULT_CARRIER_CATALOG_URL,
    "https://github.com/autisticryptic/carrier_Bundles/releases/download/v0.3.0-catalog-v7/carrier-bundles-iphone16promax-26.6.sqlite3",
    "https://github.com/autisticryptic/carrier_Bundles/releases/download/v0.3.0-catalog-v7/carrier-bundles-ios-ipcc.sqlite3",
];
const MAX_CARRIER_CATALOG_BYTES: usize = 64 * 1024 * 1024;
const MM_MODEM_STATE_SEARCHING: i32 = 7;
/// Local sink used by HTTP-originated supplementary calls until a local audio
/// backend attaches to the per-line trunk. The IMS relay still owns distinct
/// ephemeral sockets per dialog; this is only the internal RTP destination.
const LOCAL_VOICE_API_MEDIA_PORT: u16 = 40000;

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
        || matches!(
            binding.esim_status.as_str(),
            "no-profiles" | "with-profiles"
        )
}

/// Returns the explicitly named line when it is allowed to run eSIM operations.
/// A missing or unknown line is never allowed to fall through to a different
/// reader or modem.
async fn resolve_line_esim_gate(
    app: &AppState,
    line_id: &str,
) -> Result<Arc<crate::services::line_registry::LineRuntime>, EsimApiError> {
    let line_id = line_id.trim();
    if line_id.is_empty() {
        return Err(EsimApiError::Unavailable("line_id_required".to_string()));
    }
    let mut line = app.line_registry.get(line_id).await;
    if line.is_none() {
        let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
        line = app.line_registry.get(line_id).await;
    }
    let line = line.ok_or_else(|| EsimApiError::Unavailable("line_not_found".to_string()))?;

    match app.config_manager.get_line_profile(line_id).esim_control {
        Some(false) => Err(EsimApiError::Disabled),
        Some(true) => Ok(line),
        None => {
            if line_reports_euicc(&line.binding()) {
                Ok(line)
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
    Json(mut payload): Json<EsimLpacRepairRequest>,
) -> impl IntoResponse {
    if payload.proxy_prefix.is_none() {
        payload.proxy_prefix = Some(configured_github_proxy_prefix(&app));
    }
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

fn configured_github_proxy_prefix(app: &AppState) -> String {
    let config = app.config_manager.get_github_download_proxy();
    if config.enabled {
        crate::services::system::ota::normalize_proxy_prefix(Some(config.proxy_prefix))
    } else {
        String::new()
    }
}

fn requested_github_proxy_prefix(app: &AppState, requested: Option<String>) -> String {
    requested.map_or_else(
        || configured_github_proxy_prefix(app),
        |prefix| crate::services::system::ota::normalize_proxy_prefix(Some(prefix)),
    )
}

/// GET /api/settings/github-download-proxy
pub async fn get_github_download_proxy_handler(State(app): State<AppState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            app.config_manager.get_github_download_proxy(),
        )),
    )
}

/// POST /api/settings/github-download-proxy
pub async fn set_github_download_proxy_handler(
    State(app): State<AppState>,
    Json(payload): Json<GithubDownloadProxyConfig>,
) -> impl IntoResponse {
    match app.config_manager.set_github_download_proxy(payload) {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "GitHub download proxy updated",
                app.config_manager.get_github_download_proxy(),
            )),
        ),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<GithubDownloadProxyConfig>::error(error)),
        ),
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

/// GET /api/modem/lines/{line_id}/esim-reader
pub async fn get_line_esim_reader_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<EsimReaderConfig>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    if app.line_registry.get(&line_id).await.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    }
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            app.config_manager.get_line_esim_reader_config(&line_id),
        )),
    )
}

/// POST /api/modem/lines/{line_id}/esim-reader
pub async fn set_line_esim_reader_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<EsimReaderConfig>,
) -> (StatusCode, Json<ApiResponse<EsimReaderConfig>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    if app.line_registry.get(&line_id).await.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    }
    match app
        .config_manager
        .set_line_esim_reader_config(&line_id, payload)
    {
        Ok(reader) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "eSIM reader config updated",
                reader,
            )),
        ),
        Err(error) => (StatusCode::BAD_REQUEST, Json(ApiResponse::error(error))),
    }
}

/// GET /api/modem/lines/{line_id}/esim/euicc
pub async fn get_esim_euicc_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    if let Err(err) = resolve_line_esim_gate(&app, &line_id).await {
        return esim_error_response::<EsimEuiccInfo>(err);
    }
    match app.esim_supervisor.get_euicc_info_for_line(&line_id).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(err) => esim_error_response::<EsimEuiccInfo>(err),
    }
}

/// GET /api/esim/profiles/cache
pub async fn get_cached_esim_profiles_handler(State(app): State<AppState>) -> impl IntoResponse {
    match app.database.list_esim_profile_cache() {
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
    }
}

/// GET /api/modem/lines/{line_id}/esim/profiles
pub async fn get_esim_profiles_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let line = match resolve_line_esim_gate(&app, &line_id).await {
        Ok(line) => line,
        Err(err) => return esim_error_response::<EsimProfilesResponse>(err),
    };
    match app.esim_supervisor.get_profiles_for_line(&line_id).await {
        Ok(mut data) => {
            hydrate_profiles_from_cache(&app.database, &mut data.profiles);
            let binding = line.binding();
            if binding.present && !binding.modem_path.is_empty() {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(ESIM_SIM_IDENTITY_TIMEOUT_SECS),
                    sim_identity_for_modem(&app.dbus_conn, &binding.modem_path),
                )
                .await
                {
                    Ok(Some(identity)) => {
                        enrich_profiles_with_current_identity(&mut data.profiles, &identity)
                    }
                    Ok(None) => {}
                    Err(_) => warn!(
                        line_id = %line_id,
                        timeout_secs = ESIM_SIM_IDENTITY_TIMEOUT_SECS,
                        "Timed out enriching eSIM profiles with line SIM identity"
                    ),
                }
                match tokio::time::timeout(
                    std::time::Duration::from_secs(ESIM_SIM_ENRICH_TIMEOUT_SECS),
                    get_sim_info_for_modem_with_cache(
                        &app.dbus_conn,
                        &binding.modem_path,
                        Some(&app.database),
                    ),
                )
                .await
                {
                    Ok(Ok(sim_info)) => {
                        enrich_profiles_with_current_sim(&mut data.profiles, &sim_info)
                    }
                    Ok(Err(err)) => warn!(
                        line_id = %line_id,
                        error = %err,
                        "Failed to enrich eSIM profiles with line SIM"
                    ),
                    Err(_) => warn!(
                        line_id = %line_id,
                        timeout_secs = ESIM_SIM_ENRICH_TIMEOUT_SECS,
                        "Timed out enriching eSIM profiles with line SIM"
                    ),
                }
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

/// POST /api/modem/lines/{line_id}/esim/profiles/{iccid}/enable
pub async fn enable_esim_profile_handler(
    State(app): State<AppState>,
    Path((line_id, iccid)): Path<(String, String)>,
) -> impl IntoResponse {
    let line = match resolve_line_esim_gate(&app, &line_id).await {
        Ok(line) => line,
        Err(err) => return esim_error_response::<EsimCommandResponse>(err).into_response(),
    };
    let event_entity = mask_identifier(&iccid);
    let bg_line_id = line_id.clone();
    let bg_binding = line.binding();
    let line_vowifi_before_switch = app.config_manager.get_line_profile(&line_id).vowifi;
    let switch_token = new_vowifi_switch_token("profile-switch");
    if line_vowifi_before_switch.enabled {
        persist_vowifi_restore_phase(
            &app,
            &line_id,
            &switch_token,
            RestorePhase::Snapshot.as_str(),
            Instant::now(),
            false,
            false,
            None,
            0,
        );
        let scope = VowifiScope::for_line(Arc::clone(&line));
        let _ = reset_vowifi_runtime_for_scope(&app, &scope, "vowifi_profile_switch_pre_teardown")
            .await;
    }

    // Progress belongs to this line even though the underlying ModemManager
    // maintenance operation is process-wide.
    modem_manager::reset_baseband_restart_progress_for_line(&line_id);
    modem_manager::record_restart_step_for_line(&line_id, "启用 eSIM Profile", "running", None);

    let bg_app = app.clone();
    let bg_iccid = iccid.clone();
    let bg_event_entity = event_entity.clone();
    let bg_switch_token = switch_token.clone();

    let progress_line_id = bg_line_id.clone();
    tokio::spawn(modem_manager::with_baseband_restart_progress(
        progress_line_id,
        async move {
            let _guard = modem_manager::BasebandRestartRunGuard::for_line(&bg_line_id);

            match bg_app
                .esim_supervisor
                .enable_profile(&bg_line_id, bg_iccid.clone())
                .await
            {
                Ok(data) => {
                    if esim_command_succeeded(&data) {
                        modem_manager::record_restart_step("启用 eSIM Profile", "ok", None);
                        let line_profile = bg_app.config_manager.get_line_profile(&bg_line_id);
                        let auto_connect_data = line_profile.data_connection_enabled;
                        let allow_roaming = line_profile.roaming_allowed;
                        let apn_config = bg_app.config_manager.get_line_apn_config(&bg_line_id);
                        let recovery = if bg_binding.line_kind == "reader"
                            || bg_binding.modem_path.is_empty()
                        {
                            modem_manager::record_restart_step(
                                "独立读卡器无需重启基带",
                                "ok",
                                Some(bg_line_id.clone()),
                            );
                            Ok(get_baseband_restart_progress_for_line(&bg_line_id))
                        } else {
                            power_cycle_sim_for_profile_switch_via_modem(
                                &bg_app.dbus_conn,
                                &bg_line_id,
                                &bg_binding.modem_path,
                                bg_binding.qmi_device.as_deref(),
                                auto_connect_data,
                                allow_roaming,
                                Some(apn_config),
                            )
                            .await
                        };
                        match recovery {
                            Ok(_recovery) => {
                                if bg_app.sms_resync.request_scan("profile-switch") {
                                    info!("Requested SMS resync after eSIM profile switch");
                                } else {
                                    warn!("Failed to request SMS resync after eSIM profile switch");
                                }
                                spawn_vowifi_profile_switch_restore(
                                    bg_app.clone(),
                                    bg_switch_token,
                                    bg_line_id.clone(),
                                );
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
                                    info!(
                                        "Requested SMS resync after failed eSIM profile recovery"
                                    );
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
        },
    ));

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

/// POST /api/modem/lines/{line_id}/esim/profiles/{iccid}/rename
pub async fn rename_esim_profile_handler(
    State(app): State<AppState>,
    Path((line_id, iccid)): Path<(String, String)>,
    Json(payload): Json<EsimRenameRequest>,
) -> impl IntoResponse {
    if let Err(err) = resolve_line_esim_gate(&app, &line_id).await {
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
        .rename_profile(&line_id, iccid, name)
        .await
    {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Profile renamed", data)),
        ),
        Err(err) => esim_error_response::<EsimCommandResponse>(err),
    }
}

/// DELETE /api/modem/lines/{line_id}/esim/profiles/{iccid}
pub async fn delete_esim_profile_handler(
    State(app): State<AppState>,
    Path((line_id, iccid)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(err) = resolve_line_esim_gate(&app, &line_id).await {
        return esim_error_response::<EsimCommandResponse>(err);
    }
    match app
        .esim_supervisor
        .delete_profile(&line_id, iccid.clone())
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

/// POST /api/modem/lines/{line_id}/esim/profiles
pub async fn download_esim_profile_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<EsimDownloadRequest>,
) -> impl IntoResponse {
    if let Err(err) = resolve_line_esim_gate(&app, &line_id).await {
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
        .get_profiles_for_line(&line_id)
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
        .download_profile(&line_id, payload.clone())
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
                        match app.esim_supervisor.get_profiles_for_line(&line_id).await {
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
                    if let Ok(profiles_resp) =
                        app.esim_supervisor.get_profiles_for_line(&line_id).await
                    {
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

/// GET /api/modem/lines/{line_id}/device
pub async fn get_device_info(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, &line_id).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<DeviceInfoResponse>::error(reason)),
            )
        }
    };
    match get_device_info_for_modem(&app.dbus_conn, &modem_path).await {
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

/// GET /api/modem/lines/{line_id}/sim
pub async fn get_sim_info(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, &line_id).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<SimInfoResponse>::error(reason)),
            )
        }
    };
    match get_sim_info_for_modem_with_cache(&app.dbus_conn, &modem_path, Some(&app.database)).await
    {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", data)),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(ApiResponse::<SimInfoResponse>::error(format!(
                "Failed: {}",
                e
            ))),
        ),
    }
}

/// POST /api/modem/lines/{line_id}/sim/cache
pub async fn update_sim_cache_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<UpdateSimCacheRequest>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, &line_id).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            )
        }
    };
    let identity = match tokio::time::timeout(
        std::time::Duration::from_secs(ESIM_SIM_IDENTITY_TIMEOUT_SECS),
        sim_identity_for_modem(&app.dbus_conn, &modem_path),
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

/// GET /api/modem/lines/{line_id}/network
pub async fn get_network_info(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, &line_id).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<NetworkInfoResponse>::error(reason)),
            )
        }
    };
    match get_network_info_for_modem(&app.dbus_conn, &modem_path).await {
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

/// Resolve the modem owned by an explicitly named cellular line. Unknown,
/// absent, or disabled lines fail instead of falling through to another
/// baseband.
async fn resolve_modem_path(app: &AppState, line_id: &str) -> Result<String, String> {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let line_id = line_id.trim();
    if line_id.is_empty() {
        return Err("line_id_required".to_string());
    }
    let line = app
        .line_registry
        .get(line_id)
        .await
        .ok_or_else(|| "line_not_found".to_string())?;
    let binding = line.binding();
    if !binding.present {
        return Err("line_not_present".to_string());
    }
    if !app
        .config_manager
        .get_line_profile(&binding.line_id)
        .enabled
    {
        return Err("line_disabled".to_string());
    }
    Ok(binding.modem_path)
}

fn binding_has_baseband(binding: &crate::hardware::cellular::modem_manager::ModemBinding) -> bool {
    binding.line_kind.is_empty() || binding.line_kind == "baseband"
}

/// Resolve an actual cellular voice line. Standalone SIM readers can run
/// VoWiFi, but they do not own a ModemManager voice interface and must never
/// fall through to another line's baseband.
async fn resolve_call_line(app: &AppState, line_id: &str) -> Result<(String, String), String> {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let line_id = line_id.trim();
    if line_id.is_empty() {
        return Err("call_line_id_required".to_string());
    }
    let line = app
        .line_registry
        .get(line_id)
        .await
        .ok_or_else(|| "line_not_found".to_string())?;
    let binding = line.binding();
    if !binding.present {
        return Err("line_not_present".to_string());
    }
    if !app
        .config_manager
        .get_line_profile(&binding.line_id)
        .enabled
    {
        return Err("line_disabled".to_string());
    }
    if !binding_has_baseband(&binding) || binding.modem_path.trim().is_empty() {
        return Err("line_has_no_baseband".to_string());
    }
    Ok((binding.line_id, binding.modem_path))
}

async fn resolve_baseband_line(app: &AppState, line_id: &str) -> Result<(String, String), String> {
    resolve_call_line(app, line_id).await
}

async fn resolve_sms_line_id(app: &AppState, requested: &str) -> Result<String, String> {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let line_id = requested.trim();
    if line_id.is_empty() {
        return Err("sms_line_id_required".to_string());
    }
    let line = app
        .line_registry
        .get(line_id)
        .await
        .ok_or_else(|| "line_not_found".to_string())?;
    if !line.binding().present {
        return Err("line_not_present".to_string());
    }
    if !app.config_manager.get_line_profile(line_id).enabled {
        return Err("line_disabled".to_string());
    }
    Ok(line.binding().line_id)
}

/// GET /api/modem/lines/{line_id}/cells
pub async fn get_cells(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, &line_id).await {
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

/// POST /api/modem/lines/{line_id}/cell-monitor/start
pub async fn start_cell_monitor_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let (line_id, modem_path) = match resolve_baseband_line(&app, &line_id).await {
        Ok(line) => line,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            )
        }
    };
    if !app
        .cell_monitoring_active
        .lock()
        .await
        .insert(line_id.clone())
    {
        return (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Cell monitor already active",
                json!({ "line_id": line_id }),
            )),
        );
    }

    match start_cell_monitoring_for_modem(&modem_path).await {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Cell monitor activated",
                json!({ "line_id": line_id }),
            )),
        ),
        Err(e) => {
            app.cell_monitoring_active.lock().await.remove(&line_id);
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

/// POST /api/modem/lines/{line_id}/cell-monitor/stop
pub async fn stop_cell_monitor_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let (line_id, modem_path) = match resolve_baseband_line(&app, &line_id).await {
        Ok(line) => line,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            )
        }
    };
    if !app.cell_monitoring_active.lock().await.remove(&line_id) {
        return (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Cell monitor already inactive",
                json!({ "line_id": line_id }),
            )),
        );
    }

    match stop_cell_monitoring_for_modem(&modem_path).await {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Cell monitor deactivated",
                json!({ "line_id": line_id }),
            )),
        ),
        Err(e) => {
            app.cell_monitoring_active.lock().await.insert(line_id);
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

/// GET /api/modem/lines/{line_id}/radio-mode
pub async fn get_radio_mode_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, &line_id).await {
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

/// POST /api/modem/lines/{line_id}/radio-mode
pub async fn set_radio_mode_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<RadioModeRequest>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, &line_id).await {
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

/// GET /api/modem/lines/{line_id}/band-lock
pub async fn get_band_lock_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, &line_id).await {
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

/// POST /api/modem/lines/{line_id}/band-lock
pub async fn set_band_lock_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<BandLockRequest>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, &line_id).await {
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

/// GET /api/modem/lines/{line_id}/location/cell-info
pub async fn get_cell_location_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, &line_id).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<CellLocationResponse>::error(reason)),
            )
        }
    };
    match get_cell_location_for_modem(&app.dbus_conn, &modem_path).await {
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

/// GET /api/modem/lines/{line_id}/network/operators
pub async fn get_network_operators(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, &line_id).await {
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

/// GET /api/modem/lines/{line_id}/network/operators/scan
pub async fn scan_network_operators(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, &line_id).await {
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

/// POST /api/modem/lines/{line_id}/network/register-manual
pub async fn register_network_manual(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<ManualRegisterRequest>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, &line_id).await {
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

/// POST /api/modem/lines/{line_id}/network/register-auto
pub async fn register_network_auto(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, &line_id).await {
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

/// GET /api/modem/lines/{line_id}/cell-lock
pub async fn get_cell_lock_status_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let line_id = match resolve_cell_lock_line(&app, &line_id).await {
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

/// The cell-lock store is keyed by line, so every request must name the exact
/// line whose lock state it reads or mutates.
async fn resolve_cell_lock_line(app: &AppState, line_id: &str) -> Result<String, String> {
    let line_id = line_id.trim();
    if line_id.is_empty() {
        return Err("line_id_required".to_string());
    }
    app.line_registry
        .get(line_id)
        .await
        .map(|line| line.binding().line_id)
        .ok_or_else(|| "line_not_found".to_string())
}

/// POST /api/modem/lines/{line_id}/cell-lock
pub async fn set_cell_lock_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<CellLockRequest>,
) -> impl IntoResponse {
    let line_id = match resolve_cell_lock_line(&app, &line_id).await {
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

/// POST /api/modem/lines/{line_id}/cell-lock/unlock-all
pub async fn unlock_all_cells_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let line_id = match resolve_cell_lock_line(&app, &line_id).await {
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
    let previous = crate::services::network::device_network::wlan_status()
        .await
        .ok();
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
    let previous = crate::services::network::device_network::wlan_status()
        .await
        .ok();
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

/// GET /api/modem/lines/{line_id}/network/signal-strength
pub async fn get_signal_strength_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let modem_path = match resolve_modem_path(&app, &line_id).await {
        Ok(path) => path,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<SignalStrengthResponse>::error(reason)),
            )
        }
    };
    match get_signal_strength_for_modem(&app.dbus_conn, &modem_path).await {
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

async fn restart_selected_baseband(
    app: &AppState,
    requested_line_id: &str,
) -> (StatusCode, Json<ApiResponse<BasebandRestartResponse>>) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let line_id = requested_line_id.trim();
    let Some(line) = app.line_registry.get(line_id).await else {
        return (
            StatusCode::OK,
            Json(ApiResponse::<BasebandRestartResponse>::error(
                "line_not_found",
            )),
        );
    };
    let binding = line.binding();
    if !binding_has_baseband(&binding) {
        return (
            StatusCode::OK,
            Json(ApiResponse::<BasebandRestartResponse>::error(
                "line_has_no_baseband",
            )),
        );
    }
    let profile = app.config_manager.get_line_profile(&line_id);
    if !profile.enabled {
        return (
            StatusCode::OK,
            Json(ApiResponse::<BasebandRestartResponse>::error(
                "line_disabled",
            )),
        );
    }
    let apn_config = app.config_manager.get_line_apn_config(&line_id);
    let result = if binding.present && !binding.modem_path.trim().is_empty() {
        restart_baseband_via_modem(
            &app.dbus_conn,
            line_id,
            &binding.modem_path,
            profile.data_connection_enabled,
            profile.roaming_allowed,
            Some(apn_config),
        )
        .await
    } else if let Some(qmi_device) = binding
        .qmi_device
        .as_deref()
        .filter(|device| !device.trim().is_empty())
    {
        recover_absent_baseband_via_qmi(
            &app.dbus_conn,
            line_id,
            qmi_device,
            profile.data_connection_enabled,
            profile.roaming_allowed,
            Some(apn_config),
        )
        .await
    } else {
        Err("离线线路没有保留的 QMI 控制口，无法安全定位要恢复的基带".to_string())
    };
    let result = match result {
        Ok(mut data) if profile.airplane_mode_enabled => {
            let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
            let recovered = line.binding();
            if !recovered.present || recovered.modem_path.trim().is_empty() {
                Err("基带已执行恢复，但重新枚举后仍无法应用飞行模式配置".to_string())
            } else {
                match modem_manager::set_airplane_mode_for_modem(
                    app.dbus_conn.as_ref(),
                    &recovered.modem_path,
                    true,
                )
                .await
                {
                    Ok(_) => {
                        let step = BasebandRestartStep {
                            step: "应用已保存的飞行模式".to_string(),
                            status: "ok".to_string(),
                            detail: Some("移动射频保持关闭".to_string()),
                        };
                        modem_manager::record_restart_step_for_line(
                            line_id,
                            &step.step,
                            &step.status,
                            step.detail.clone(),
                        );
                        data.steps.push(step);
                        Ok(data)
                    }
                    Err(error) => Err(format!("基带已恢复，但应用飞行模式失败：{error}")),
                }
            }
        }
        result => result,
    };
    match result {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                if binding.present {
                    "Baseband restarted"
                } else {
                    "Offline baseband recovered"
                },
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

pub async fn restart_line_baseband_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    restart_selected_baseband(&app, &line_id).await
}

pub async fn get_line_baseband_restart_status_handler(
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            get_baseband_restart_progress_for_line(line_id.trim()),
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
    if binding.present
        && profile.data_connection_enabled
        && !proxy_status.running
        && proxy_status.phase == "disabled"
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

/// GET /api/modem/lines/{line_id}/data
///
/// Every cellular data status is scoped to a physical SIM line. The bulk
/// `/api/modem/line-controls` endpoint remains useful for dashboards, while
/// this endpoint gives controllers an unambiguous per-line read path.
pub async fn get_line_data_connection_handler(
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
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            build_line_network_controls(&app, &line).await,
        )),
    )
}

pub(crate) async fn stop_line_data_runtime(
    app: &AppState,
    line: &Arc<crate::services::line_registry::LineRuntime>,
) {
    let _guard = line.bearer_operation_lock.lock().await;
    stop_line_data_runtime_locked(app, line).await;
}

/// Restore only the persisted network intents owned by one present line. Used
/// at boot and when a stable line reappears after hotplug.
pub(crate) async fn restore_line_runtime_intents(
    app: &AppState,
    line: &Arc<crate::services::line_registry::LineRuntime>,
) {
    let binding = line.binding();
    if !binding.present {
        return;
    }
    let profile = app.config_manager.get_line_profile(&binding.line_id);
    if !profile.enabled {
        stop_line_data_runtime(app, line).await;
        return;
    }
    if profile.airplane_mode_enabled {
        stop_line_data_runtime(app, line).await;
        if let Err(error) = modem_manager::set_airplane_mode_for_modem(
            app.dbus_conn.as_ref(),
            &binding.modem_path,
            true,
        )
        .await
        {
            warn!(line_id = %binding.line_id, error = %error, "Failed to restore line airplane mode");
        }
        return;
    }
    if profile.data_connection_enabled {
        if let Err(error) = start_line_data_runtime(app, line, &profile).await {
            line.data_proxy.record_error(error.clone()).await;
            warn!(line_id = %binding.line_id, error = %error, "Failed to restore per-line data runtime");
        }
    }
}

fn cooldown_elapsed(last_attempt: Option<Instant>, cooldown_secs: u64) -> bool {
    last_attempt
        .map(|attempt| attempt.elapsed() >= Duration::from_secs(cooldown_secs))
        .unwrap_or(true)
}

/// Reconcile registration, bearer and proxy health for one selected line. The
/// caller holds that line's watchdog state lock; no counters or cooldowns are
/// shared with another SIM.
async fn reconcile_line_data_health(
    app: &AppState,
    line: &Arc<crate::services::line_registry::LineRuntime>,
) {
    let Ok(mut watchdog) = line.data_watchdog.try_lock() else {
        return;
    };
    let binding = line.binding();
    let profile = app.config_manager.get_line_profile(&binding.line_id);
    if !binding.present || !profile.enabled || profile.airplane_mode_enabled {
        watchdog.reset();
        return;
    }

    match modem_manager::get_modem_state_for_modem(app.dbus_conn.as_ref(), &binding.modem_path)
        .await
    {
        Ok(MM_MODEM_STATE_SEARCHING) => {
            watchdog.searching_polls = watchdog.searching_polls.saturating_add(1);
            if watchdog.searching_polls >= LINE_DATA_REGISTER_THRESHOLD
                && cooldown_elapsed(
                    watchdog.last_register_attempt,
                    LINE_DATA_REGISTER_COOLDOWN_SECS,
                )
            {
                watchdog.last_register_attempt = Some(Instant::now());
                app.system_event_emitter
                    .emit_code(
                        system_event_codes::CELLULAR_SEARCHING_THRESHOLD,
                        system_event_severity::WARNING,
                        system_event_status::TRIGGERED,
                        binding.line_id.clone(),
                        format!(
                            "Line {} remained searching; requesting automatic registration on {}",
                            binding.line_id, binding.modem_path
                        ),
                    )
                    .await;
                match request_operator_registration_for_modem(
                    app.dbus_conn.as_ref(),
                    &binding.modem_path,
                    "",
                )
                .await
                {
                    Ok(()) => {
                        watchdog.searching_polls = 0;
                        info!(line_id = %binding.line_id, modem_path = %binding.modem_path, "Per-line watchdog requested automatic registration");
                    }
                    Err(error) => {
                        warn!(line_id = %binding.line_id, modem_path = %binding.modem_path, error = %error, "Per-line automatic registration failed")
                    }
                }
            }
        }
        Ok(_) => watchdog.searching_polls = 0,
        Err(error) => {
            warn!(line_id = %binding.line_id, modem_path = %binding.modem_path, error = %error, "Per-line watchdog could not read modem state");
            return;
        }
    }

    if !profile.data_connection_enabled {
        watchdog.missing_data_polls = 0;
        watchdog.last_connect_attempt = None;
        return;
    }

    let interface = if let Some(interface) = line.secondary_data.interface().await {
        Some(interface)
    } else {
        modem_manager::data_interface_for_modem(app.dbus_conn.as_ref(), &binding.modem_path)
            .await
            .unwrap_or(None)
    };
    let proxy = line.data_proxy.status().await;
    let healthy = interface.as_deref().is_some_and(|interface| {
        proxy.running && proxy.interface_name.as_deref() == Some(interface)
    });
    if healthy {
        watchdog.missing_data_polls = 0;
        return;
    }

    watchdog.missing_data_polls = watchdog.missing_data_polls.saturating_add(1);
    if !cooldown_elapsed(
        watchdog.last_connect_attempt,
        LINE_DATA_CONNECT_COOLDOWN_SECS,
    ) {
        return;
    }
    watchdog.last_connect_attempt = Some(Instant::now());
    match start_line_data_runtime(app, line, &profile).await {
        Ok(()) => {
            watchdog.missing_data_polls = 0;
            info!(line_id = %binding.line_id, "Per-line watchdog restored data bearer and proxy");
        }
        Err(error) => {
            line.data_proxy.record_error(error.clone()).await;
            warn!(line_id = %binding.line_id, error = %error, "Per-line watchdog data restore failed");
        }
    }
}

/// Start one independent data-health workflow per present line. Each tick is
/// scheduled concurrently, while a line-local `try_lock` prevents overlap.
pub fn spawn_line_data_supervisor(app: AppState) {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(LINE_DATA_WATCHDOG_INTERVAL_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        loop {
            interval.tick().await;
            for line in app.line_registry.all().await {
                let reconcile_app = app.clone();
                tokio::spawn(async move {
                    reconcile_line_data_health(&reconcile_app, &line).await;
                });
            }
        }
    });
}

/// Tear down only volatile resources for a removed line. Persisted switches are
/// deliberately untouched so the same stable line can recover after hotplug.
pub(crate) async fn suspend_line_runtime_for_hotplug(
    app: &AppState,
    line: &Arc<crate::services::line_registry::LineRuntime>,
) {
    let binding = line.binding();
    {
        let _bearer_guard = line.bearer_operation_lock.lock().await;
        line.data_proxy.stop().await;
        line.secondary_data.stop().await;
        let _connect_guard = line.volte_connect_lock.lock().await;
        crate::connectivity::modems::ims::volte::live::disconnect_live_for_line(
            &line.volte_live,
            &line.volte,
            "volte_line_not_present",
        )
        .await;
    }
    let scope = VowifiScope::for_line(Arc::clone(line));
    let _ = reset_vowifi_runtime_for_scope(app, &scope, "vowifi_line_not_present").await;
    info!(line_id = %binding.line_id, "Suspended volatile runtimes for absent line");
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
    crate::connectivity::modems::ims::volte::data_slot::DataSlotMode,
    crate::connectivity::modems::ims::volte::VolteError,
> {
    use crate::connectivity::modems::ims::volte::data_slot::{
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
    let profile = app.config_manager.get_line_profile(&line_id);
    if payload.enabled && !profile.enabled {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("line_disabled")),
        );
    }
    if payload.enabled && profile.airplane_mode_enabled {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("line_airplane_mode_enabled")),
        );
    }

    if payload.enabled && !profile.data_connection_enabled {
        // Each explicit disabled -> enabled transition starts a new usage
        // session. Clear both the in-memory counters and persisted baseline
        // before accepting clients on the new listener.
        app.line_registry.reset_data_traffic(&line_id).await;
    }
    let profile = match app
        .config_manager
        .set_line_data_connection_enabled(&line_id, payload.enabled)
    {
        Ok(profile) => profile,
        Err(error) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::error(format!("Failed: {error}"))),
            )
        }
    };
    // Offline lines remain configurable. Persist the requested state now and
    // let the inventory reconciler apply it after this exact line reappears.
    if !binding.present {
        return (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Saved; device offline",
                build_line_network_controls(&app, &line).await,
            )),
        );
    }
    if payload.enabled {
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
        if binding.present {
            if let Err(error) = start_line_data_runtime(&app, &line, &profile).await {
                line.data_proxy.record_error(error.clone()).await;
                return (
                    StatusCode::OK,
                    Json(ApiResponse::error(format!("Failed: {error}"))),
                );
            }
        } else {
            line.data_proxy.stop().await;
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
        if binding.present {
            stop_line_data_runtime(&app, &line).await;
            if let Err(error) = start_line_data_runtime(&app, &line, &profile).await {
                line.data_proxy.record_error(error.clone()).await;
                data_error = Some(error);
            }
        }
    }
    if line.binding().present && profile.enabled && profile.volte_connection_enabled {
        let status = line.volte.status().await;
        if status.registered {
            let _bearer_guard = line.bearer_operation_lock.lock().await;
            let _guard = line.volte_connect_lock.lock().await;
            crate::connectivity::modems::ims::volte::live::disconnect_live_for_line(
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
    if let Err(error) = app
        .config_manager
        .set_line_airplane_mode(&line_id, payload.enabled)
    {
        return (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {error}"))),
        );
    }
    if !binding.present {
        return (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Saved; device offline",
                build_line_network_controls(&app, &line).await,
            )),
        );
    }
    if payload.enabled {
        let _bearer_guard = line.bearer_operation_lock.lock().await;
        stop_line_data_runtime_locked(&app, &line).await;
        let _volte_guard = line.volte_connect_lock.lock().await;
        crate::connectivity::modems::ims::volte::live::disconnect_live_for_line(
            &line.volte_live,
            &line.volte,
            "line_airplane_mode_enabled",
        )
        .await;
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
    sync_line_video_capabilities(&app).await;
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

fn persist_vowifi_mt_deliveries(
    db: &Database,
    line_id: &str,
    outcome: &MoSmsSipOutcome,
    dedupe_enabled: bool,
) -> Vec<SmsMessage> {
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
            storage_key = vowifi_mt_storage_key(line_id, outcome, originator, &text);
            let storage_marker = format!("vowifi-mt:{storage_key}");
            api_sms_id = db
                .sms_id_by_pdu_for_line(line_id, &storage_marker)
                .unwrap_or(None);
            let should_insert = if api_sms_id.is_some() {
                false
            } else if !dedupe_enabled {
                true
            } else {
                let service_center_timestamp = parts
                    .first()
                    .map(|part| part.service_center_timestamp.as_str())
                    .unwrap_or_default();
                let fingerprint = crate::services::orchestrator::message_fingerprint(
                    &crate::services::orchestrator::MessageFingerprintInput {
                        service_center_timestamp,
                        originator,
                        text: &text,
                        segment_reference: None,
                        segment_sequence: 1,
                        segment_total: 1,
                    },
                );
                match db.claim_sms_dedup(line_id, &fingerprint, "vowifi_ims") {
                    Ok(claimed) => claimed,
                    Err(error) => {
                        warn!(
                            line_id,
                            error = %error,
                            "Failed to claim VoWiFi MT SMS fingerprint"
                        );
                        false
                    }
                }
            };
            if should_insert {
                let timestamp = crate::platform::db::beijing_sms_now_string();
                api_sms_id = db
                    .insert_sms_at_with_transport_for_line(
                        "incoming",
                        originator,
                        &text,
                        &timestamp,
                        "received",
                        Some(&storage_marker),
                        "vowifi_ims",
                        Some(line_id),
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
                        line_id: Some(line_id.to_string()),
                    });
                }
            }
        }
        let short_key = &storage_key[..std::cmp::min(16, storage_key.len())];
        let mt_message_id = format!("vowifi-mt-{short_key}");
        let mt_trace_id = format!("{}-mt-{short_key}", outcome.trace_id);

        let _ = db.upsert_vowifi_sms_delivery(NewVowifiSmsDelivery {
            message_id: &mt_message_id,
            line_id,
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
                line_id,
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
    line_id: String,
    mut followup: tokio::sync::mpsc::UnboundedReceiver<
        crate::connectivity::modems::ims::vowifi::live::LiveSmsFollowupFrame,
    >,
) {
    tokio::spawn(async move {
        while let Some(frame) = followup.recv().await {
            let dedupe_enabled = app
                .config_manager
                .get_line_sms_path_policy(&line_id)
                .dedupe_enabled;
            let mt_messages = persist_vowifi_mt_deliveries(
                &app.database,
                &line_id,
                &frame.outcome,
                dedupe_enabled,
            );
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

fn vowifi_mt_storage_key(
    line_id: &str,
    outcome: &MoSmsSipOutcome,
    originator: &str,
    text: &str,
) -> String {
    let text_hash = format!("{:x}", md5::compute(text.as_bytes()));
    let material = format!("{line_id}|{}|{originator}|{text_hash}", outcome.message_id);
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
    line_id: &str,
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
                line_id,
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

/// POST /api/modem/lines/{line_id}/sms/send
pub async fn send_sms_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<SendSmsRequest>,
) -> impl IntoResponse {
    let line_id = match resolve_sms_line_id(&app, &line_id).await {
        Ok(line_id) => line_id,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<serde_json::Value>::error(format!(
                    "Failed to send SMS: {reason}"
                ))),
            )
        }
    };
    // Sending uses a fixed VoWiFi -> VoLTE -> CS fallback order unless this
    // line explicitly requires VoWiFi-only delivery.
    let scope = match VowifiScope::resolve(&app, &line_id).await {
        Ok(scope) => scope,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<serde_json::Value>::error(format!(
                    "Failed to send SMS: {reason}"
                ))),
            )
        }
    };
    let policy = app.config_manager.get_line_sms_path_policy(scope.line_id());
    let mut failures = Vec::new();
    for path in policy.enabled_layers() {
        let result = match path {
            AccessPathKind::Vowifi => send_sms_over_vowifi_path(&app, &scope, &payload).await,
            AccessPathKind::Volte => send_sms_over_volte_path(&app, &line_id, &payload).await,
            AccessPathKind::Cs => send_sms_over_cs_path(&app, &line_id, &payload).await,
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
                warn!(path = path.as_str(), reason = %reason, "SMS send path failed");
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
        .insert_sms_with_transport_for_line(
            "outgoing",
            &payload.phone_number,
            &payload.content,
            outcome.api_status(),
            None,
            "vowifi_ims",
            Some(scope.line_id()),
        )
        .ok();
    let _ = app
        .database
        .upsert_vowifi_sms_delivery(NewVowifiSmsDelivery {
            message_id: &outcome.message_id,
            line_id: scope.line_id(),
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
    spawn_vowifi_sms_followup_persist(
        app.clone(),
        scope.line_id().to_string(),
        send_result.followup,
    );
    Ok(json!({
        "path": "vowifi_ims",
        "transport": "vowifi_ims",
        "message_id": outcome.message_id,
        "trace_id": outcome.trace_id,
        "delivery_state": outcome.delivery_state.as_str(),
        "rpdu_ack": outcome.rpdu_ack.as_str(),
        "mt_followup": "background",
        "line_id": scope.line_id(),
    }))
}

async fn send_sms_over_volte_path(
    app: &AppState,
    line_id: &str,
    payload: &SendSmsRequest,
) -> Result<serde_json::Value, String> {
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
    let profile = app.config_manager.get_line_profile(line_id);
    if !profile.enabled || !profile.volte_connection_enabled {
        return Err("line_volte_connection_disabled".to_string());
    }
    if !line.volte.status().await.registered {
        let (_, sim_override) = ims_override_for_line(app, line_id).await?;
        let ip_families = app.config_manager.get_line_volte_ip_families(line_id);
        let device =
            crate::connectivity::modems::ims::volte::live::VolteDeviceBinding::from_modem(&binding)
                .map_err(|error| error.to_string())?;
        let _bearer_guard = line.bearer_operation_lock.lock().await;
        let data_slot_mode = prepare_line_data_slot_for_volte(app, &line, &profile)
            .await
            .map_err(|error| error.to_string())?;
        let _guard = line.volte_connect_lock.lock().await;
        if !line.volte.status().await.registered {
            crate::connectivity::modems::ims::volte::live::connect_live_for_line(
                &line.volte_live,
                &device,
                &line.volte,
                app.config_manager.get_line_volte_voice_enabled(line_id),
                &ip_families,
                profile.roaming_allowed,
                data_slot_mode,
                app.config_manager
                    .get_line_sms_path_policy(line_id)
                    .dedupe_enabled,
                profile_store(app),
                sim_override,
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
    let result = crate::connectivity::modems::ims::volte::live::send_live_sms_for_line(
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
    line_id: &str,
    payload: &SendSmsRequest,
) -> Result<serde_json::Value, String> {
    let line = app
        .line_registry
        .get(line_id)
        .await
        .ok_or_else(|| "line_not_found".to_string())?;
    let binding = line.binding();
    if !binding.present {
        return Err("line_not_present".to_string());
    }
    if !binding_has_baseband(&binding) || binding.modem_path.trim().is_empty() {
        return Err("line_has_no_baseband".to_string());
    }
    let path = send_sms_via_modem(
        &app.dbus_conn,
        &binding.modem_path,
        &payload.phone_number,
        &payload.content,
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
            "modem",
            Some(line_id),
        )
        .map_err(|error| error.to_string())?;
    Ok(json!({ "path": path, "transport": "modem", "line_id": line_id }))
}

/// Drain the voice call follow-up channel, logging dialog/media progress
/// (ringing, answer, hangup). Mirrors `spawn_vowifi_sms_followup_persist`.
fn spawn_vowifi_call_followup(
    mut followup: tokio::sync::mpsc::UnboundedReceiver<
        crate::connectivity::modems::ims::vowifi::live::LiveCallFollowupFrame,
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

/// POST /api/vowifi/lines/{line_id}/voice/call
///
/// Places a VoWiFi voice call over IMS. Requires the VoWiFi feature to be
/// enabled and the runtime to have reached `voice_ready`. Unlike SMS, there is
/// no modem fallback wired here yet: the carrier (AT + USB-Audio) leg is a
/// reserved interface and is not exposed until a media backend is attached.
pub async fn place_call_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<PlaceCallRequest>,
) -> impl IntoResponse {
    let scope = match VowifiScope::resolve(&app, &line_id).await {
        Ok(scope) => scope,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            )
        }
    };
    if !scope.is_present() {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::<serde_json::Value>::error("line_not_present")),
        );
    }
    if !app.config_manager.get_line_profile(scope.line_id()).enabled {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::<serde_json::Value>::error("line_disabled")),
        );
    }
    let line_vowifi_enabled = app
        .config_manager
        .get_line_profile(scope.line_id())
        .vowifi
        .enabled;
    if !line_vowifi_enabled {
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
                        "line_id": scope.line_id(),
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
    Query(params): Query<SmsMutationRequest>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let Some(channel_id) = sms_channel_filter(Some(&params.channel_id)) else {
        return (
            StatusCode::OK,
            Json(ApiResponse::error("channel_id is required")),
        );
    };

    match app.database.clear_sms_for_channel(channel_id) {
        Ok(deleted) => {
            schedule_sms_db_maintenance(&app, deleted);
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "SMS channel cleared",
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
    Query(params): Query<SmsMutationRequest>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let Some(channel_id) = sms_channel_filter(Some(&params.channel_id)) else {
        return (
            StatusCode::OK,
            Json(ApiResponse::error("channel_id is required")),
        );
    };

    match db.delete_sms_for_channel(id, channel_id) {
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
    Query(params): Query<SmsMutationRequest>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let Some(channel_id) = sms_channel_filter(Some(&params.channel_id)) else {
        return (
            StatusCode::OK,
            Json(ApiResponse::error("channel_id is required")),
        );
    };

    match app
        .database
        .delete_sms_conversation_for_channel(&phone_number, channel_id)
    {
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
    let Some(channel_id) = sms_channel_filter(Some(&payload.channel_id)) else {
        return (
            StatusCode::OK,
            Json(ApiResponse::error("channel_id is required")),
        );
    };

    match app.database.delete_sms_batch_for_channel(
        &payload.ids,
        &payload.phone_numbers,
        channel_id,
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
    line_id: &str,
    path: &str,
    direction: &str,
    phone_number: &str,
    answered: bool,
) {
    let mut active = app.active_calls.lock().await;
    if let Some(record) = active.get_mut(path) {
        if record.direction == "unknown" && direction != "unknown" {
            record.direction = direction.to_string();
        }
        if answered {
            record.answered = true;
            if record.answered_at.is_none() {
                record.answered_at = Some(std::time::Instant::now());
            }
        }
        record.missing_polls = 0;
        return;
    }
    if let Ok(id) = app
        .database
        .insert_call(Some(line_id), direction, phone_number, answered)
    {
        active.insert(
            path.to_string(),
            crate::state::ActiveCallRecord {
                id,
                line_id: line_id.to_string(),
                direction: direction.to_string(),
                answered_at: answered.then(std::time::Instant::now),
                answered,
                missing_polls: 0,
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
        record.missing_polls = 0;
    }
}

fn call_poll_marks_finished(record: &mut crate::state::ActiveCallRecord, observed: bool) -> bool {
    if observed {
        record.missing_polls = 0;
        return false;
    }
    record.missing_polls = record.missing_polls.saturating_add(1);
    record.missing_polls >= CALL_END_MISSING_POLLS
}

async fn finish_tracked_call(
    app: &AppState,
    path: &str,
    answered_now: bool,
) -> Option<crate::platform::db::CallRecord> {
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
        let answered = record.answered || answered_now;
        let result = if record.direction == "incoming" && !answered {
            app.database.mark_call_missed(record.id)
        } else {
            app.database.update_call_end(record.id, duration, answered)
        };
        if let Err(error) = result {
            warn!(call_id = record.id, %error, "Failed to finish tracked call history");
            return None;
        }
        let call = app.database.get_call_by_id(record.id).ok().flatten();
        if let Some(call) = call.clone() {
            let notification_sender = Arc::clone(&app.notification_sender);
            tokio::spawn(async move {
                if let Err(error) = notification_sender.forward_call(&call).await {
                    warn!(
                        call_id = call.id,
                        line_id = ?call.line_id,
                        %error,
                        "Failed to forward call notification"
                    );
                }
            });
        }
        return call;
    }
    None
}

async fn list_calls_for_line(
    app: &AppState,
    line_id: &str,
    modem_path: &str,
) -> Result<CallListResponse, String> {
    let mut data = list_current_calls_for_modem(&app.dbus_conn, modem_path)
        .await
        .map_err(|error| error.to_string())?;
    for call in &mut data.calls {
        call.line_id = line_id.to_string();
    }
    Ok(data)
}

async fn track_observed_calls(app: &AppState, data: &CallListResponse) {
    for call in &data.calls {
        let answered = matches!(call.state.as_str(), "active" | "held");
        track_call_start(
            app,
            &call.line_id,
            &call.path,
            &call.direction,
            &call.phone_number,
            answered,
        )
        .await;
    }
}

async fn reconcile_finished_calls(
    app: &AppState,
    reconciled_lines: &HashSet<String>,
    observed_paths: &HashSet<String>,
) {
    let finished_paths = {
        let mut active = app.active_calls.lock().await;
        active
            .iter_mut()
            .filter_map(|(path, record)| {
                if !reconciled_lines.contains(&record.line_id) {
                    return None;
                }
                call_poll_marks_finished(record, observed_paths.contains(path))
                    .then(|| path.clone())
            })
            .collect::<Vec<_>>()
    };
    for path in finished_paths {
        let _ = finish_tracked_call(app, &path, false).await;
    }
}

pub fn spawn_call_monitor(app: AppState) {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(CALL_MONITOR_INTERVAL_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let mut reconciled_lines = HashSet::new();
            let mut observed_paths = HashSet::new();
            for line in app.line_registry.all().await {
                let binding = line.binding();
                if binding.line_kind == "reader" {
                    continue;
                }
                if !binding.present {
                    reconciled_lines.insert(binding.line_id);
                    continue;
                }
                match list_calls_for_line(&app, &binding.line_id, &binding.modem_path).await {
                    Ok(data) => {
                        reconciled_lines.insert(binding.line_id);
                        observed_paths.extend(data.calls.iter().map(|call| call.path.clone()));
                        track_observed_calls(&app, &data).await;
                    }
                    Err(error) => tracing::debug!(
                        line_id = %binding.line_id,
                        %error,
                        "Call monitor could not poll line"
                    ),
                }
            }
            reconcile_finished_calls(&app, &reconciled_lines, &observed_paths).await;
        }
    });
}

pub async fn get_line_calls_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let (line_id, modem_path) = match resolve_call_line(&app, &line_id).await {
        Ok(line) => line,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<CallListResponse>::error(reason)),
            )
        }
    };
    match list_calls_for_line(&app, &line_id, &modem_path).await {
        Ok(data) => {
            track_observed_calls(&app, &data).await;
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message("Success", data)),
            )
        }
        Err(error) => (
            StatusCode::OK,
            Json(ApiResponse::<CallListResponse>::error(format!(
                "Failed: {error}"
            ))),
        ),
    }
}

async fn dial_call_on_line(
    app: &AppState,
    requested_line_id: &str,
    phone_number: String,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let phone_number = phone_number.trim().to_string();
    if phone_number.is_empty() {
        return (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(
                "Phone number is required",
            )),
        );
    }
    let (line_id, modem_path) = match resolve_call_line(app, requested_line_id).await {
        Ok(line) => line,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            )
        }
    };
    match make_call_on_modem(&app.dbus_conn, &modem_path, &phone_number).await {
        Ok(path) => {
            track_call_start(app, &line_id, &path, "outgoing", &phone_number, false).await;
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Call started",
                    json!({ "path": path, "line_id": line_id }),
                )),
            )
        }
        Err(error) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed to dial: {error}"
            ))),
        ),
    }
}

pub async fn dial_line_call_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<MakeCallRequest>,
) -> impl IntoResponse {
    dial_call_on_line(&app, &line_id, payload.phone_number).await
}

async fn resolve_call_owner(
    app: &AppState,
    requested_line_id: &str,
    call_path: &str,
) -> Result<(String, String, CallInfo), String> {
    let (line_id, modem_path) = resolve_call_line(app, requested_line_id).await?;
    let mut call = get_call_by_path_for_modem(&app.dbus_conn, &modem_path, call_path)
        .await
        .map_err(|_| "call_not_found_on_selected_line".to_string())?;
    call.line_id = line_id.clone();
    Ok((line_id, modem_path, call))
}

async fn hangup_call_on_line(
    app: &AppState,
    requested_line_id: &str,
    path: String,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let (line_id, modem_path, before) =
        match resolve_call_owner(app, requested_line_id, &path).await {
            Ok(owner) => owner,
            Err(reason) => {
                return (
                    StatusCode::OK,
                    Json(ApiResponse::<serde_json::Value>::error(reason)),
                )
            }
        };
    let answered = matches!(before.state.as_str(), "active" | "held");
    track_call_start(
        app,
        &line_id,
        &path,
        &before.direction,
        &before.phone_number,
        answered,
    )
    .await;
    match hangup_call_on_modem(&app.dbus_conn, &modem_path, &path).await {
        Ok(()) => {
            let _ = finish_tracked_call(app, &path, answered).await;
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Call hung up",
                    json!({ "line_id": line_id }),
                )),
            )
        }
        Err(error) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed to hang up: {error}"
            ))),
        ),
    }
}

pub async fn hangup_line_call_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<HangupCallRequest>,
) -> impl IntoResponse {
    hangup_call_on_line(&app, &line_id, payload.path).await
}

async fn hangup_all_calls_on_lines(
    app: &AppState,
    requested_line_id: &str,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let lines = match resolve_call_line(app, requested_line_id).await {
        Ok(line) => vec![line],
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            )
        }
    };

    let mut failures = Vec::new();
    for (line_id, modem_path) in lines {
        let before = list_calls_for_line(app, &line_id, &modem_path)
            .await
            .unwrap_or_default();
        if before.calls.is_empty() {
            continue;
        }
        for call in &before.calls {
            track_call_start(
                app,
                &line_id,
                &call.path,
                &call.direction,
                &call.phone_number,
                matches!(call.state.as_str(), "active" | "held"),
            )
            .await;
        }
        if let Err(error) = hangup_all_calls_for_modem(&app.dbus_conn, &modem_path).await {
            failures.push(format!("{line_id}: {error}"));
            continue;
        }
        for call in before.calls {
            let answered = matches!(call.state.as_str(), "active" | "held");
            let _ = finish_tracked_call(app, &call.path, answered).await;
        }
    }
    if failures.is_empty() {
        (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "All calls hung up",
                json!({}),
            )),
        )
    } else {
        (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed to hang up calls: {}",
                failures.join("; ")
            ))),
        )
    }
}

pub async fn hangup_all_line_calls_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    hangup_all_calls_on_lines(&app, &line_id).await
}

async fn answer_call_on_line(
    app: &AppState,
    requested_line_id: &str,
    path: String,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let (line_id, modem_path, before) =
        match resolve_call_owner(app, requested_line_id, &path).await {
            Ok(owner) => owner,
            Err(reason) => {
                return (
                    StatusCode::OK,
                    Json(ApiResponse::<serde_json::Value>::error(reason)),
                )
            }
        };
    track_call_start(
        app,
        &line_id,
        &path,
        &before.direction,
        &before.phone_number,
        matches!(before.state.as_str(), "active" | "held"),
    )
    .await;
    match answer_call_on_modem(&app.dbus_conn, &modem_path, &path).await {
        Ok(()) => {
            mark_tracked_call_answered(app, &path).await;
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Call answered",
                    json!({ "line_id": line_id }),
                )),
            )
        }
        Err(error) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed to answer call: {error}"
            ))),
        ),
    }
}

pub async fn answer_line_call_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<HangupCallRequest>,
) -> impl IntoResponse {
    answer_call_on_line(&app, &line_id, payload.path).await
}

pub async fn send_line_call_dtmf_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<SendCallDtmfRequest>,
) -> impl IntoResponse {
    let (line_id, modem_path, call) = match resolve_call_owner(&app, &line_id, &payload.path).await
    {
        Ok(owner) => owner,
        Err(reason) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            )
        }
    };
    if !matches!(call.state.as_str(), "active" | "held") {
        return (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(
                "call_not_active_on_selected_line",
            )),
        );
    }
    match send_call_dtmf_on_modem(&app.dbus_conn, &modem_path, &payload.path, &payload.digit).await
    {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "DTMF sent",
                json!({ "line_id": line_id }),
            )),
        ),
        Err(error) => (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(format!(
                "Failed to send DTMF: {error}"
            ))),
        ),
    }
}

pub async fn get_line_call_history_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Query(params): Query<CallHistoryRequest>,
) -> (StatusCode, Json<ApiResponse<CallHistoryResponse>>) {
    let (line_id, _) = match resolve_call_line(&app, &line_id).await {
        Ok(line) => line,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<CallHistoryResponse>::error(reason)),
            )
        }
    };
    let limit = if params.limit > 0 { params.limit } else { 50 };
    let offset = if params.offset >= 0 { params.offset } else { 0 };
    let records = app
        .database
        .get_call_history_for_line(&line_id, limit, offset);
    let stats = app.database.get_call_stats_for_line(&line_id);
    match (records, stats) {
        (Ok(records), Ok(stats)) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Success",
                CallHistoryResponse {
                    line_id,
                    records,
                    stats,
                },
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

pub async fn delete_line_call_history_handler(
    State(app): State<AppState>,
    Path((line_id, id)): Path<(String, i64)>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let (line_id, _) = match resolve_call_line(&app, &line_id).await {
        Ok(line) => line,
        Err(reason) => return (StatusCode::OK, Json(ApiResponse::error(reason))),
    };
    match app.database.delete_call_for_line(&line_id, id) {
        Ok(1) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Call record deleted",
                json!({}),
            )),
        ),
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::error("call_record_not_found_on_selected_line")),
        ),
        Err(error) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {error}"))),
        ),
    }
}

pub async fn clear_line_call_history_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let (line_id, _) = match resolve_call_line(&app, &line_id).await {
        Ok(line) => line,
        Err(reason) => return (StatusCode::OK, Json(ApiResponse::error(reason))),
    };
    match app.database.clear_calls_for_line(&line_id) {
        Ok(deleted) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Call history cleared",
                json!({ "deleted": deleted }),
            )),
        ),
        Err(error) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {error}"))),
        ),
    }
}

pub async fn get_line_call_settings_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    let (_, modem_path) = match resolve_call_line(&app, &line_id).await {
        Ok(line) => line,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<CallSettingsResponse>::error(reason)),
            )
        }
    };
    match get_call_settings_for_modem(&app.dbus_conn, &modem_path).await {
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

pub async fn set_line_call_settings_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
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
    let (_, modem_path) = match resolve_call_line(&app, &line_id).await {
        Ok(line) => line,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            )
        }
    };
    match set_call_waiting_for_modem(&app.dbus_conn, &modem_path, enabled).await {
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

pub async fn get_line_call_volume_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    if let Err(reason) = resolve_call_line(&app, &line_id).await {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<CallVolumeResponse>::error(reason)),
        );
    }
    (
        StatusCode::OK,
        Json(ApiResponse::<CallVolumeResponse>::error(
            "Call volume control is not exposed by ModemManager on this backend",
        )),
    )
}

pub async fn set_line_call_volume_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<SetCallVolumeRequest>,
) -> impl IntoResponse {
    if let Err(reason) = resolve_call_line(&app, &line_id).await {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<CallVolumeResponse>::error(reason)),
        );
    }
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

pub async fn get_line_call_forwarding_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    if let Err(reason) = resolve_call_line(&app, &line_id).await {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<CallForwardingResponse>::error(reason)),
        );
    }
    (
        StatusCode::OK,
        Json(ApiResponse::<CallForwardingResponse>::error(
            "Call forwarding is not exposed by ModemManager on this backend",
        )),
    )
}

pub async fn set_line_call_forwarding_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<SetCallForwardingRequest>,
) -> impl IntoResponse {
    if let Err(reason) = resolve_call_line(&app, &line_id).await {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<CallForwardingResponse>::error(reason)),
        );
    }
    let _ = (payload.forward_type, payload.number, payload.timeout);
    (
        StatusCode::OK,
        Json(ApiResponse::<CallForwardingResponse>::error(
            "Call forwarding is not exposed by ModemManager on this backend",
        )),
    )
}

pub async fn get_line_ims_status_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> impl IntoResponse {
    if let Err(reason) = resolve_call_line(&app, &line_id).await {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<ImsStatusResponse>::error(reason)),
        );
    }
    (
        StatusCode::OK,
        Json(ApiResponse::<ImsStatusResponse>::error(
            "IMS status is not exposed by ModemManager on this backend",
        )),
    )
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

/// One explicitly selected line and its private VoWiFi runtime.
///
/// There is deliberately no unbound/global scope: an unknown line must fail
/// instead of sharing a fallback runtime with another SIM.
#[derive(Clone)]
pub struct VowifiScope {
    line: Arc<crate::services::line_registry::LineRuntime>,
    runtime: Arc<crate::connectivity::modems::ims::vowifi::runtime::VowifiRuntime>,
    line_id: String,
}

impl VowifiScope {
    fn for_line(line: Arc<crate::services::line_registry::LineRuntime>) -> Self {
        let line_id = line.binding().line_id;
        let runtime = Arc::clone(&line.vowifi);
        Self {
            line,
            runtime,
            line_id,
        }
    }

    async fn resolve(app: &AppState, line_id: &str) -> Result<Self, String> {
        let line_id = line_id.trim();
        if line_id.is_empty() {
            return Err("vowifi_line_id_required".to_string());
        }
        app.line_registry
            .get(line_id)
            .await
            .map(Self::for_line)
            .ok_or_else(|| "vowifi_line_not_found".to_string())
    }

    fn line_id(&self) -> &str {
        &self.line_id
    }

    fn runtime(&self) -> &Arc<crate::connectivity::modems::ims::vowifi::runtime::VowifiRuntime> {
        &self.runtime
    }

    fn is_present(&self) -> bool {
        self.line.binding().present
    }

    /// The modem this line owns, when the line is actually present. `None` means
    /// there is nothing to act on, which callers treat as a no-op rather than
    /// falling back to some other baseband.
    fn modem_path(&self) -> Option<String> {
        let binding = self.line.binding();
        binding.present.then_some(binding.modem_path)
    }

    /// The connect guard is per line, so two lines can dial their ePDGs at the
    /// same time while each line still rejects a second concurrent connect.
    fn try_connect_lock(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.line.vowifi_connect_lock.try_lock().ok()
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

struct VowifiRestoreClaim(Arc<crate::services::line_registry::LineRuntime>);

impl VowifiRestoreClaim {
    fn acquire(scope: &VowifiScope) -> Option<Self> {
        if scope.line.begin_vowifi_restore() {
            Some(Self(Arc::clone(&scope.line)))
        } else {
            None
        }
    }
}

impl Drop for VowifiRestoreClaim {
    fn drop(&mut self) {
        self.0.finish_vowifi_restore();
    }
}

fn vowifi_restore_intent_enabled(app: &AppState, workflow: &VowifiRestoreWorkflow) -> bool {
    app.config_manager
        .get_line_profile(&workflow.line_id)
        .vowifi
        .enabled
}

async fn reset_vowifi_runtime_for_scope(
    app: &AppState,
    scope: &VowifiScope,
    reason: &str,
) -> VowifiStatusResponse {
    clear_live_runtime_for_line(scope.line_id()).await;
    let snapshot = scope.runtime().reset_runtime(reason).await;
    let status = snapshot.status_response();
    persist_vowifi_runtime_snapshot(app, scope.line_id(), &status);
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
            line_id: scope.line_id(),
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
            line_id: scope.line_id(),
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
            line_id: scope.line_id(),
            trace_id: Some("runtime-stop"),
            level: "info",
            phase: "connection_stop",
            profile_id,
            event_type: "runtime_stop",
            detail_json: "{}",
        });

    status
}

async fn stop_vowifi_and_restore_cellular(
    app: &AppState,
    scope: &VowifiScope,
    reason: &str,
) -> VowifiStatusResponse {
    let _ = disable_vowifi_connection_for_scope(app, scope);
    restore_cellular_and_reset_vowifi(app, scope, reason).await
}

/// Turn off the VoWiFi connection intent for exactly one line.
fn disable_vowifi_connection_for_scope(app: &AppState, scope: &VowifiScope) -> Result<(), String> {
    app.config_manager
        .set_line_vowifi_connection_enabled(scope.line_id(), false)
        .map(|_| ())
}

fn spawn_vowifi_profile_switch_restore(app: AppState, switch_token: String, line_id: String) {
    let line_enabled = app.config_manager.get_line_profile(&line_id).vowifi.enabled;
    if !line_enabled {
        return;
    }
    tokio::spawn(async move {
        run_vowifi_restore_workflow(
            app,
            VowifiRestoreWorkflow::profile_switch(switch_token, line_id),
        )
        .await;
    });
}

#[derive(Clone)]
struct VowifiRestoreWorkflow {
    trigger: VowifiRestoreTrigger,
    line_id: String,
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
    fn profile_switch(switch_token: String, line_id: String) -> Self {
        Self {
            trigger: VowifiRestoreTrigger::ProfileSwitch { switch_token },
            line_id,
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

    fn boot_auto_restore(config: &AutoRestoreConfig, line_id: String) -> Self {
        Self {
            trigger: VowifiRestoreTrigger::BootAutoRestore,
            line_id,
            initial_delay: Duration::from_secs(config.initial_delay_secs.clamp(30, 300)),
            attempts: config.attempts.clamp(1, 5),
            retry_delay: Duration::from_secs(config.retry_delay_secs.clamp(10, 180)),
            connect_attempts: config.attempts.clamp(1, 5),
            connect_retry_delay: Duration::from_secs(config.retry_delay_secs.clamp(10, 180)),
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
    let scope = match VowifiScope::resolve(&app, &workflow.line_id).await {
        Ok(scope) => scope,
        Err(error) => {
            warn!(line_id = %workflow.line_id, error = %error, "Skipping WiFi Calling restore for an unknown line");
            persist_optional_vowifi_restore_phase(
                &app,
                &workflow,
                RestorePhase::Failed,
                Instant::now(),
                false,
                false,
                Some(&error),
                0,
            );
            return;
        }
    };
    let Some(_claim) = VowifiRestoreClaim::acquire(&scope) else {
        return;
    };
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

    if !vowifi_restore_intent_enabled(&app, &workflow) {
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

    // Explicit disable and physical removal both cancel a pending delayed
    // restore. A removed card keeps its persisted intent so the hotplug
    // reconciler can try again when the same stable line returns.
    if !vowifi_restore_intent_enabled(&app, &workflow) || !scope.is_present() {
        return;
    }

    if let Err(error) = configure_vowifi_live_overrides(&app, &scope).await {
        persist_optional_vowifi_restore_phase(
            &app,
            &workflow,
            RestorePhase::Failed,
            Instant::now(),
            false,
            false,
            Some(&error),
            0,
        );
        return;
    }

    let mut last_status = disabled_vowifi_status("vowifi_restore_not_attempted");
    let attempts = workflow.attempts.max(1);
    for attempt in 1..=attempts {
        if !vowifi_restore_intent_enabled(&app, &workflow) || !scope.is_present() {
            return;
        }
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
        last_status = connect_vowifi_on_line(
            &app,
            &scope,
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
    if !scope.is_present() || !vowifi_restore_intent_enabled(&app, &workflow) {
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
    // A transient route, DNS, or ePDG failure must not overwrite the user's
    // per-line enable intent. Reset only the failed runtime so a later manual
    // attempt or boot restore can retry after connectivity returns.
    let _ = restore_cellular_and_reset_vowifi(&app, &scope, workflow.fallback_reason).await;
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
        persist_vowifi_runtime_snapshot(app, scope.line_id(), &last_status);
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
    persist_vowifi_runtime_snapshot(app, scope.line_id(), &last_status);
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
        persist_vowifi_runtime_snapshot(app, scope.line_id(), &status);
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
            &workflow.line_id,
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
/// and only for the line that is connecting - a second SIM keeps its own state.
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
    let should_restore_data =
        line_profile.data_connection_enabled && !line_profile.airplane_mode_enabled;
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
    persist_vowifi_runtime_snapshot(app, scope.line_id(), &status);
    status
}

/// Publish the immutable SIM-bound connection snapshot before profile matching
/// starts. Manual connects and boot/profile restores must both do this so a
/// presented IMSI behaves consistently after restart.
async fn configure_vowifi_live_overrides(
    app: &AppState,
    scope: &VowifiScope,
) -> Result<(), String> {
    let line_id = scope.line_id().to_string();
    let line_config = app.config_manager.get_line_profile(&line_id).vowifi;
    let (_, sim_override) = ims_override_for_line(app, &line_id).await?;
    let device_imei = app
        .line_registry
        .get(&line_id)
        .await
        .map(|line| line.binding().equipment_identifier.clone());
    crate::connectivity::modems::ims::vowifi::live::configure_live_network_overrides_with_device_imei(
        &line_id,
        &line_config,
        Some(&sim_override),
        device_imei.as_deref(),
    )
}

async fn connect_vowifi_on_line(
    app: &AppState,
    scope: &VowifiScope,
    attempts: u8,
    retry_delay: std::time::Duration,
    fallback_to_cellular_on_failure: bool,
) -> VowifiStatusResponse {
    if !scope.is_present() {
        return disabled_vowifi_status("vowifi_line_not_present");
    }
    let current = scope.status().await;
    if current.readiness.sms_ready {
        persist_vowifi_runtime_snapshot(app, scope.line_id(), &current);
        return current;
    }

    // Resolve and publish SIM-bound values only after proving that this call is
    // starting a new session. PATCHing SimOverrideStore while a session is
    // active therefore cannot alter its IKE/REGISTER refresh behavior.
    if let Err(error) = configure_vowifi_live_overrides(app, scope).await {
        let mut status = disabled_vowifi_status("vowifi_line_network_config_invalid");
        status.degraded_reason = Some(error);
        persist_vowifi_runtime_snapshot(app, scope.line_id(), &status);
        return status;
    }

    let Some(_connect_guard) = scope.try_connect_lock() else {
        let mut status = scope.status().await;
        if !status.readiness.sms_ready {
            status.degraded_reason = Some("vowifi_connect_already_running".to_string());
        }
        persist_vowifi_runtime_snapshot(app, scope.line_id(), &status);
        return status;
    };

    let current = scope.status().await;
    if current.readiness.sms_ready {
        persist_vowifi_runtime_snapshot(app, scope.line_id(), &current);
        return current;
    }

    let profile_meta = current.profile.profile.as_ref();
    let profile_id = profile_meta.map(|p| p.profile_id);
    let _ = app
        .database
        .insert_vowifi_runtime_event(crate::platform::db::NewVowifiRuntimeEvent {
            line_id: scope.line_id(),
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
        persist_vowifi_runtime_snapshot(app, scope.line_id(), &status);
        return status;
    }

    let prepared = wait_for_vowifi_identity_gate(app, scope, None, 0).await;
    if !prepared.readiness.identity_ready || !prepared.readiness.profile_matched {
        persist_vowifi_runtime_snapshot(app, scope.line_id(), &prepared);
        return prepared;
    }

    if let Err(status) = wait_for_vowifi_sim_auth_gate(app, scope, None, 0).await {
        persist_vowifi_runtime_snapshot(app, scope.line_id(), &status);
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
        // Preserve the configured enable intent. The current runtime is
        // degraded and cellular data is restored, but the operator should not
        // have to re-enable VoWiFi after a temporary DNS or route outage.
        last_status = restore_cellular_and_reset_vowifi(app, scope, &fallback_reason).await;
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

#[derive(Debug, serde::Serialize, Default)]
pub struct VolteLineControlResponse {
    pub modem: crate::hardware::cellular::modem_manager::ModemBinding,
    pub profile: LineProfileConfig,
    pub runtime: crate::connectivity::modems::ims::volte::VolteRuntimeStatus,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct VowifiLineConfigResponse {
    pub line_id: String,
    pub modem: crate::hardware::cellular::modem_manager::ModemBinding,
    pub config: LineVowifiConfig,
    pub runtime_phase: String,
    pub runtime_stage: String,
    pub runtime_registered: bool,
    pub runtime_error: Option<String>,
    pub matched_profile_id: Option<String>,
}

async fn build_vowifi_line_response(
    app: &AppState,
    line: &crate::services::line_registry::LineRuntime,
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
    let mut response = Vec::with_capacity(lines.len());
    for line in &lines {
        response.push(build_vowifi_line_response(&app, line).await);
    }
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", response)),
    )
}

pub async fn get_vowifi_line_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<VowifiLineConfigResponse>>) {
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
            build_vowifi_line_response(&app, &line).await,
        )),
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
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            build_vowifi_line_response(&app, &line).await,
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
    let binding = line.binding();
    // Validate the future connection snapshot without replacing an active
    // session's immutable values. Publication happens inside the connect lock.
    if payload.enabled && binding.present {
        let mut next = app.config_manager.get_line_profile(&line_id).vowifi;
        next.enabled = true;
        let (_, sim_override) = match ims_override_for_line(&app, &line_id).await {
            Ok(resolved) => resolved,
            Err(error) => {
                return (
                    StatusCode::CONFLICT,
                    Json(ApiResponse::error(format!("Failed: {error}"))),
                );
            }
        };
        if let Err(error) =
            crate::connectivity::modems::ims::vowifi::live::validate_live_network_overrides(
                &next,
                Some(&sim_override),
            )
        {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse::error(format!("Failed: {error}"))),
            );
        }
    }
    if !payload.enabled {
        // Drop the overrides so a disabled line stops influencing anything.
        crate::connectivity::modems::ims::vowifi::live::forget_live_network_overrides(&line_id);
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
    if !binding.present {
        return (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Saved; device offline",
                build_vowifi_line_response(&app, &line).await,
            )),
        );
    }
    // Connect/disconnect the line the request names. Lines no longer share a
    // runtime, so a secondary SIM can hold its own VoWiFi registration while the
    // primary one is up (or down).
    let scope = VowifiScope::for_line(Arc::clone(&line));
    if payload.enabled {
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
        let _ = reset_vowifi_runtime_for_scope(&app, &scope, "vowifi_line_disabled").await;
    }
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            build_vowifi_line_response(&app, &line).await,
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

pub async fn get_pcsc_readers_handler(
    State(_app): State<AppState>,
) -> (
    StatusCode,
    Json<ApiResponse<Vec<crate::hardware::devices::pcsc::PcscReaderInfo>>>,
) {
    match crate::hardware::devices::pcsc::discover_readers().await {
        Ok(readers) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", readers)),
        ),
        Err(error) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {error}"))),
        ),
    }
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
    if !binding.present {
        sync_line_video_capabilities(&app).await;
        return (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Saved; device offline",
                VolteLineControlResponse {
                    modem: line.binding(),
                    profile: profile.redacted(),
                    runtime: line.volte.status().await,
                },
            )),
        );
    }
    let result: Result<
        crate::connectivity::modems::ims::volte::VolteRuntimeStatus,
        crate::connectivity::modems::ims::volte::VolteError,
    > = if payload.enabled {
        start_line_volte_restore(app.clone(), Arc::clone(&line), "connection_enabled").await;
        Ok(line.volte.status().await)
    } else {
        let _bearer_guard = line.bearer_operation_lock.lock().await;
        let _guard = line.volte_connect_lock.lock().await;
        Ok(
            crate::connectivity::modems::ims::volte::live::disconnect_live_for_line(
                &line.volte_live,
                &line.volte,
                "volte_line_connection_disabled",
            )
            .await,
        )
    };
    sync_line_video_capabilities(&app).await;
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
/// `families` is this line's non-empty ordered attempt list
/// (`["ipv4","ipv6"]`, `["ipv6"]`, …).
#[derive(Debug, serde::Deserialize)]
pub struct SetVolteIpFamiliesRequest {
    pub families: Vec<crate::platform::config::VolteIpFamily>,
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
            crate::connectivity::modems::ims::volte::live::disconnect_live_for_line(
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
        line.reconcile_trunk_profile(&profile.trunk).await;
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
    line.reconcile_trunk_profile(&profile.trunk).await;
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
    let active_line_ids = app
        .line_registry
        .all()
        .await
        .into_iter()
        .filter_map(|runtime| {
            let binding = runtime.binding();
            binding.present.then_some(binding.line_id)
        })
        .collect::<HashSet<_>>();
    match app.config_manager.set_line_trunk_profile_for_active_lines(
        &line_id,
        payload,
        &active_line_ids,
    ) {
        Ok(profile) => {
            line.activate_trunk_profile(&profile.trunk).await;
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
    let active_line_ids = app
        .line_registry
        .all()
        .await
        .into_iter()
        .filter_map(|runtime| {
            let binding = runtime.binding();
            binding.present.then_some(binding.line_id)
        })
        .collect::<HashSet<_>>();
    match app.config_manager.set_line_trunk_enabled_for_active_lines(
        &line_id,
        payload.enabled,
        &active_line_ids,
    ) {
        Ok(profile) => {
            line.activate_trunk_profile(&profile.trunk).await;
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

async fn resolve_control_line(
    app: &AppState,
    line_id: &str,
) -> Option<Arc<crate::services::line_registry::LineRuntime>> {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    app.line_registry.get(line_id).await
}

fn line_volte_enabled(app: &AppState, line: &crate::services::line_registry::LineRuntime) -> bool {
    let binding = line.binding();
    let profile = app.config_manager.get_line_profile(&binding.line_id);
    binding.present && profile.enabled && profile.volte_connection_enabled
}

async fn sync_line_video_capabilities(app: &AppState) {
    for line in app.line_registry.all().await {
        let line_id = line.binding().line_id;
        let line_enabled = line_volte_enabled(app, &line);
        let voice_enabled = app.config_manager.get_line_volte_voice_enabled(&line_id);
        let ims_video = app.config_manager.get_line_ims_video_config(&line_id);
        let registered = line.volte.status().await.registered;
        line.volte_live
            .operator_link()
            .set_ready(line_enabled && voice_enabled && registered);
        line.voice_access.set_backend_video_enabled(
            AccessPathKind::Volte,
            line_enabled && voice_enabled && ims_video.volte_enabled,
        );
        let vowifi = app.config_manager.get_line_profile(&line_id).vowifi;
        line.voice_access.set_backend_video_enabled(
            AccessPathKind::Vowifi,
            vowifi.enabled && ims_video.vowifi_enabled,
        );
    }
}

/// VoLTE voice (gateway-mode) status. Voice is a forward-looking extension: the
/// target device relays RTP between the operator IMS leg and an internal SIP UA;
/// it never plays audio locally. `enabled` reflects this line's IMS connection
/// and voice switches; `gateway_mode` is always true on this hardware class.
#[derive(Debug, serde::Serialize, Default)]
pub struct VolteVoiceStatusResponse {
    pub line_id: String,
    pub enabled: bool,
    pub ims_connection_enabled: bool,
    pub voice_enabled: bool,
    pub registered: bool,
    pub gateway_mode: bool,
    pub local_audio_capable: bool,
}

impl VolteVoiceStatusResponse {
    fn build(line_id: String, line_enabled: bool, voice_enabled: bool, registered: bool) -> Self {
        Self {
            line_id,
            enabled: line_enabled && voice_enabled,
            ims_connection_enabled: line_enabled,
            voice_enabled,
            registered,
            // Qualcomm 410 pocket-WiFi has no mic/speaker/PCM: relay only.
            gateway_mode: true,
            local_audio_capable: false,
        }
    }
}

async fn current_volte_voice_status(
    app: &AppState,
    line: &crate::services::line_registry::LineRuntime,
) -> VolteVoiceStatusResponse {
    let line_id = line.binding().line_id;
    let registered = line.volte.status().await.registered;
    VolteVoiceStatusResponse::build(
        line_id.clone(),
        line_volte_enabled(app, line),
        app.config_manager.get_line_volte_voice_enabled(&line_id),
        registered,
    )
}

pub async fn get_volte_call_status_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<VolteVoiceStatusResponse>>) {
    let Some(line) = resolve_control_line(&app, &line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            current_volte_voice_status(&app, &line).await,
        )),
    )
}

pub async fn set_volte_voice_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<VolteControlToggleRequest>,
) -> (StatusCode, Json<ApiResponse<VolteVoiceStatusResponse>>) {
    let Some(line) = resolve_control_line(&app, &line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    match app
        .config_manager
        .set_line_volte_voice_enabled(&line_id, payload.enabled)
    {
        Ok(_) => {
            sync_line_video_capabilities(&app).await;
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Success",
                    current_volte_voice_status(&app, &line).await,
                )),
            )
        }
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
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<SmsPathPolicy>>) {
    if resolve_control_line(&app, &line_id).await.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    }
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            app.config_manager.get_line_sms_path_policy(&line_id),
        )),
    )
}

/// Replace the SMS multi-path routing policy. The incoming policy is normalized
/// before persisting, so a partial or duplicated priority list from the UI can
/// never leave the config in an invalid state.
pub async fn set_sms_path_policy_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<SmsPathPolicy>,
) -> (StatusCode, Json<ApiResponse<SmsPathPolicy>>) {
    if resolve_control_line(&app, &line_id).await.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    }
    match app
        .config_manager
        .set_line_sms_path_policy(&line_id, payload)
    {
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
/// gating chain (the target line's IMS connection, voice gateway and ViLTE
/// switches). `gateway_mode`/`local_video_capable` mirror the
/// VoLTE voice response: the device is a pure video relay, never a video
/// endpoint.
#[derive(Debug, serde::Serialize, Default)]
pub struct VilteStatusResponse {
    pub line_id: String,
    /// Whether VoLTE video is effectively available (config + voice + ready).
    pub enabled: bool,
    /// Whether the VoLTE video gate is enabled in config (independent of ready).
    pub feature_enabled: bool,
    pub registered: bool,
    pub gateway_mode: bool,
    pub local_video_capable: bool,
    pub config: ImsVideoConfig,
}

impl VilteStatusResponse {
    async fn build(app: &AppState, line: &crate::services::line_registry::LineRuntime) -> Self {
        let line_id = line.binding().line_id;
        let ims_video = app.config_manager.get_line_ims_video_config(&line_id);
        let voice_ready = line_volte_enabled(app, line)
            && app.config_manager.get_line_volte_voice_enabled(&line_id);
        Self {
            line_id,
            enabled: voice_ready && ims_video.volte_enabled,
            feature_enabled: ims_video.volte_enabled,
            registered: line.volte.status().await.registered,
            // Qualcomm 410 pocket-WiFi has no camera/display/codec: relay only.
            gateway_mode: true,
            local_video_capable: false,
            config: ims_video,
        }
    }
}

pub async fn get_vilte_control_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<VilteStatusResponse>>) {
    let Some(line) = resolve_control_line(&app, &line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            VilteStatusResponse::build(&app, &line).await,
        )),
    )
}

pub async fn set_vilte_feature_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<VolteControlToggleRequest>,
) -> (StatusCode, Json<ApiResponse<VilteStatusResponse>>) {
    let Some(line) = resolve_control_line(&app, &line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    match app
        .config_manager
        .set_line_ims_video_volte_enabled(&line_id, payload.enabled)
    {
        Ok(_) => {
            sync_line_video_capabilities(&app).await;
            let status = VilteStatusResponse::build(&app, &line).await;
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

/// Replace the full IMS video config (codec / payload type / fmtp). The
/// `volte_enabled` gate is honored only when VoLTE voice is enabled, and
/// `vowifi_enabled` only when VoWiFi voice is enabled; otherwise each gate is
/// forced off by the config layer.
pub async fn set_vilte_config_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<ImsVideoConfig>,
) -> (StatusCode, Json<ApiResponse<VilteStatusResponse>>) {
    let Some(line) = resolve_control_line(&app, &line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    match app
        .config_manager
        .set_line_ims_video_config(&line_id, payload)
    {
        Ok(_) => {
            sync_line_video_capabilities(&app).await;
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Success",
                    VilteStatusResponse::build(&app, &line).await,
                )),
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

// ===================== VoWiFi carrier profile store =====================

fn profile_store(
    app: &AppState,
) -> crate::connectivity::modems::ims::vowifi::profile_store::ProfileStore {
    crate::connectivity::modems::ims::vowifi::profile_store::ProfileStore::new(
        Arc::clone(&app.carrier_catalog),
        Arc::clone(&app.database),
    )
}

fn carrier_catalog_status(app: &AppState) -> Result<CarrierCatalogStatusResponse, String> {
    use crate::connectivity::modems::ims::vowifi::carrier_catalog::CatalogAccessKind;

    let release = app.carrier_catalog.release()?;
    if !release.sealed {
        return Err(format!(
            "carrier_catalog_release_not_sealed:{}",
            release.release_id
        ));
    }
    let volte_profiles = app.carrier_catalog.list(CatalogAccessKind::LteEpc)?.len();
    let vowifi_profiles = app.carrier_catalog.list(CatalogAccessKind::WifiEpdg)?.len();
    Ok(CarrierCatalogStatusResponse {
        installed: true,
        usable: true,
        path: app.carrier_catalog.path().display().to_string(),
        release_id: release.release_id,
        generated_at: release.generated_at,
        sealed: true,
        volte_profiles,
        vowifi_profiles,
        message: "carrier catalog is ready".to_string(),
    })
}

/// GET /api/vowifi/carrier-catalog/status
pub async fn get_carrier_catalog_status_handler(
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<CarrierCatalogStatusResponse>>) {
    match carrier_catalog_status(&app) {
        Ok(status) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", status)),
        ),
        Err(error) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Carrier catalog unavailable",
                CarrierCatalogStatusResponse {
                    installed: app.carrier_catalog.path().is_file(),
                    usable: false,
                    path: app.carrier_catalog.path().display().to_string(),
                    message: error,
                    ..CarrierCatalogStatusResponse::default()
                },
            )),
        ),
    }
}

async fn download_carrier_catalog(asset_url: &str, proxy_prefix: &str) -> Result<Vec<u8>, String> {
    if !ALLOWED_CARRIER_CATALOG_URLS.contains(&asset_url) {
        return Err("carrier_catalog_asset_not_allowed".to_string());
    }

    let client = reqwest::Client::builder()
        .user_agent("SimAdmin carrier catalog installer")
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|error| format!("carrier_catalog_http_client_failed:{error}"))?;
    let mut last_error = String::new();
    for url in crate::services::system::ota::ota_request_urls(asset_url, proxy_prefix, false) {
        let response = match client.get(&url).send().await {
            Ok(response) => response,
            Err(error) => {
                last_error = format!("carrier_catalog_download_failed:{error}");
                continue;
            }
        };
        if !response.status().is_success() {
            last_error = format!("carrier_catalog_download_http_status:{}", response.status());
            continue;
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_CARRIER_CATALOG_BYTES as u64)
        {
            return Err("carrier_catalog_download_too_large".to_string());
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| format!("carrier_catalog_download_read_failed:{error}"))?;
        if bytes.len() > MAX_CARRIER_CATALOG_BYTES {
            return Err("carrier_catalog_download_too_large".to_string());
        }
        return Ok(bytes.to_vec());
    }
    Err(if last_error.is_empty() {
        "carrier_catalog_download_failed".to_string()
    } else {
        last_error
    })
}

/// POST /api/vowifi/carrier-catalog/install
pub async fn install_carrier_catalog_handler(
    State(app): State<AppState>,
    Json(payload): Json<CarrierCatalogInstallRequest>,
) -> (StatusCode, Json<ApiResponse<CarrierCatalogInstallResponse>>) {
    use crate::connectivity::modems::ims::vowifi::{
        carrier_catalog::{CarrierCatalog, CatalogAccessKind},
        profile_store::ProfileStore,
    };

    let asset_url = payload
        .asset_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_CARRIER_CATALOG_URL)
        .to_string();
    let proxy_prefix = requested_github_proxy_prefix(&app, payload.proxy_prefix);
    let result = async {
        let bytes = download_carrier_catalog(&asset_url, &proxy_prefix).await?;
        let target = app.carrier_catalog.path();
        let parent = target
            .parent()
            .ok_or_else(|| "carrier_catalog_target_parent_missing".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("carrier_catalog_target_create_failed:{error}"))?;
        let temp_path = parent.join(format!(
            ".carrier-bundles-{}-{}.sqlite3",
            std::process::id(),
            chrono::Utc::now().timestamp_millis()
        ));

        let install_result = (|| {
            fs::write(&temp_path, &bytes)
                .map_err(|error| format!("carrier_catalog_temp_write_failed:{error}"))?;
            let candidate = CarrierCatalog::open(&temp_path)?;
            let release = candidate.release()?;
            if !release.sealed {
                return Err(format!(
                    "carrier_catalog_release_not_sealed:{}",
                    release.release_id
                ));
            }
            let volte_profiles = candidate.list(CatalogAccessKind::LteEpc)?.len();
            let vowifi_profiles = candidate.list(CatalogAccessKind::WifiEpdg)?.len();
            fs::rename(&temp_path, target)
                .map_err(|error| format!("carrier_catalog_activate_failed:{error}"))?;
            ProfileStore::new(Arc::clone(&app.carrier_catalog), Arc::clone(&app.database))
                .publish();
            Ok(CarrierCatalogInstallResponse {
                installed: true,
                path: target.display().to_string(),
                asset_url: asset_url.clone(),
                release_id: release.release_id,
                generated_at: release.generated_at,
                volte_profiles,
                vowifi_profiles,
                message: "carrier catalog downloaded, validated, and activated".to_string(),
            })
        })();
        if temp_path.exists() {
            let _ = fs::remove_file(&temp_path);
        }
        install_result
    }
    .await;

    match result {
        Ok(installed) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Carrier catalog installed",
                installed,
            )),
        ),
        Err(error) => (StatusCode::BAD_REQUEST, Json(ApiResponse::error(error))),
    }
}

/// GET /api/vowifi/carrier-profiles
pub async fn list_vowifi_carrier_profiles_handler(
    State(app): State<AppState>,
) -> (
    StatusCode,
    Json<ApiResponse<Vec<crate::connectivity::modems::ims::vowifi::profile_store::StoredProfile>>>,
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
/// Store an operator-authored override in `data.db`. The sealed catalog remains
/// untouched, so downloading a newer release cannot overwrite local profiles.
pub async fn upsert_vowifi_carrier_profile_handler(
    State(app): State<AppState>,
    Json(record): Json<
        crate::connectivity::modems::ims::vowifi::profile_record::CarrierProfileRecord,
    >,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    match profile_store(&app).upsert(record) {
        Ok(profile) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Custom carrier profile saved",
                json!(profile),
            )),
        ),
        Err(error) => (StatusCode::BAD_REQUEST, Json(ApiResponse::error(error))),
    }
}

/// DELETE /api/vowifi/carrier-profiles/{profile_id}
pub async fn delete_vowifi_carrier_profile_handler(
    State(app): State<AppState>,
    Path(profile_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    match profile_store(&app).delete(&profile_id) {
        Ok(true) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Custom carrier profile deleted",
                json!({ "deleted": true }),
            )),
        ),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("custom_carrier_profile_not_found")),
        ),
        Err(error) => (StatusCode::BAD_REQUEST, Json(ApiResponse::error(error))),
    }
}

/// GET /api/vowifi/carrier-profiles/{profile_id}/icon
pub async fn get_vowifi_carrier_profile_icon_handler(
    State(app): State<AppState>,
    Path(profile_id): Path<String>,
) -> axum::response::Response {
    match app.carrier_catalog.profile_icon(&profile_id) {
        Ok(Some(icon)) => (
            StatusCode::OK,
            [
                (axum::http::header::CONTENT_TYPE, icon.media_type),
                (
                    axum::http::header::CACHE_CONTROL,
                    "private, max-age=86400".to_string(),
                ),
            ],
            icon.data,
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            warn!(profile_id, error = %error, "Failed to read carrier profile icon");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

/// GET /api/vowifi/carrier-profiles/resolve?plmn=23433
///
/// Show which profile a PLMN actually resolves to and where it came from
/// (carrier catalog).
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
            let record = crate::connectivity::modems::ims::vowifi::profile_record::CarrierProfileRecord::from_profile(
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

async fn resolve_vowifi_diagnostic_line_id(
    app: &AppState,
    requested: &str,
) -> Result<String, String> {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let line_ids = app
        .line_registry
        .all()
        .await
        .into_iter()
        .map(|line| line.binding().line_id)
        .collect::<Vec<_>>();
    select_vowifi_diagnostic_line_id(requested, &line_ids)
}

fn select_vowifi_diagnostic_line_id(
    requested: &str,
    present_line_ids: &[String],
) -> Result<String, String> {
    let line_id = requested.trim();
    if line_id.is_empty() {
        return Err("vowifi_line_id_required".to_string());
    }
    present_line_ids
        .iter()
        .find(|candidate| candidate.as_str() == line_id)
        .cloned()
        .ok_or_else(|| "vowifi_line_not_found".to_string())
}

async fn current_vowifi_status(
    app: &AppState,
    line_id: &str,
    live_probe: bool,
) -> VowifiStatusResponse {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let scope = match VowifiScope::resolve(app, line_id).await {
        Ok(scope) => scope,
        Err(reason) => return disabled_vowifi_status(&reason),
    };
    let connection_enabled = app
        .config_manager
        .get_line_profile(scope.line_id())
        .vowifi
        .enabled;
    scope
        .runtime()
        .refresh_identity_with_timeout(
            &app.dbus_conn,
            std::time::Duration::from_secs(VOWIFI_SIM_IDENTITY_TIMEOUT_SECS),
        )
        .await;
    let snapshot = if live_probe && connection_enabled {
        scope
            .runtime()
            .refresh_status_readiness_with_stage_timeout(
                Some(&app.database),
                std::time::Duration::from_secs(VOWIFI_STATUS_STAGE_TIMEOUT_SECS),
            )
            .await
    } else {
        scope.runtime().snapshot().await
    };
    let mut status = snapshot.status_response();
    if !connection_enabled {
        status.phase = "not_started";
    }
    persist_vowifi_runtime_snapshot(app, scope.line_id(), &status);
    status
}

pub fn spawn_vowifi_auto_restore(app: AppState) {
    tokio::spawn(async move {
        loop {
            let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
            let present_lines = app
                .line_registry
                .all()
                .await
                .into_iter()
                .filter(|line| {
                    let binding = line.binding();
                    binding.present
                        && app
                            .config_manager
                            .get_line_profile(&binding.line_id)
                            .enabled
                })
                .collect::<Vec<_>>();
            for line in present_lines.into_iter().filter(|line| {
                line_vowifi_restore_enabled(
                    &app.config_manager.get_line_profile(&line.binding().line_id),
                )
            }) {
                let line_id = line.binding().line_id;
                let auto_restore = app
                    .config_manager
                    .get_line_profile(&line_id)
                    .vowifi
                    .auto_restore;
                schedule_vowifi_auto_restore(&app, &auto_restore, line, line_id).await;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

fn line_vowifi_restore_enabled(profile: &LineProfileConfig) -> bool {
    profile.enabled && profile.vowifi.enabled
}

async fn schedule_vowifi_auto_restore(
    app: &AppState,
    config: &AutoRestoreConfig,
    line: Arc<crate::services::line_registry::LineRuntime>,
    line_id: String,
) {
    if line.vowifi_restore_in_progress() || line.vowifi.snapshot().await.readiness().sms_ready {
        return;
    }
    let workflow = VowifiRestoreWorkflow::boot_auto_restore(config, line_id);
    info!(
        line_id = %line.binding().line_id,
        initial_delay_secs = workflow.initial_delay.as_secs(),
        attempts = workflow.attempts,
        "WiFi Calling line auto-restore scheduled"
    );
    let restore_app = app.clone();
    tokio::spawn(async move {
        run_vowifi_restore_workflow(restore_app, workflow).await;
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
    if !line_volte_restore_enabled(&app, &line) || !line.begin_volte_retry() {
        return false;
    }
    let line_id = line.binding().line_id;
    let retry_max = u32::from(
        app.config_manager
            .get_line_profile(&line_id)
            .volte_auto_restore
            .attempts
            .clamp(1, 5),
    );
    let voice_enabled = app.config_manager.get_line_volte_voice_enabled(&line_id);
    let ims_video = app.config_manager.get_line_ims_video_config(&line_id);
    line.voice_access.set_backend_video_enabled(
        AccessPathKind::Volte,
        voice_enabled && ims_video.volte_enabled,
    );
    line.volte
        .update(|state| {
            state.recovery_state =
                crate::connectivity::modems::ims::volte::runtime::VolteRecoveryState::Connecting;
            state.recovery_source = Some(source.to_string());
            state.retry_attempt = 0;
            state.retry_max = retry_max;
            state.modem_restart_attempt = 0;
            state.modem_restart_max = 0;
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
    Deferred,
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
                state.phase = crate::connectivity::modems::ims::volte::runtime::VoltePhase::Degraded;
                state.stage = crate::connectivity::modems::ims::volte::runtime::VolteStage::Modem;
                state.recovery_state =
                    crate::connectivity::modems::ims::volte::runtime::VolteRecoveryState::WaitingModem;
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

    // A missing stable line may simply be a removed USB modem or SIM tray.
    // Never restart the process-wide baseband to recover one absent line: that
    // would interrupt healthy calls on every other card. Preserve intent and
    // let the inventory reconciler start a fresh batch after hotplug.
    line.volte
        .update(|state| {
            state.phase = crate::connectivity::modems::ims::volte::runtime::VoltePhase::Degraded;
            state.recovery_state =
                crate::connectivity::modems::ims::volte::runtime::VolteRecoveryState::WaitingModem;
            state.manual_retry_available = false;
            state.next_retry_at = None;
            state.last_error = Some("volte_line_not_present".to_string());
        })
        .await;
    LineModemWait::Deferred
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
                    state.recovery_state =
                        crate::connectivity::modems::ims::volte::runtime::VolteRecoveryState::Idle;
                    state.recovery_source = None;
                    state.next_retry_at = None;
                    state.manual_retry_available = false;
                })
                .await;
            return;
        }
        LineModemWait::Deferred => return,
    }

    let restore_policy = app
        .config_manager
        .get_line_profile(&line.binding().line_id)
        .volte_auto_restore;
    let max_attempts = u32::from(restore_policy.attempts.clamp(1, 5));
    for attempt in 1..=max_attempts {
        let profile = app.config_manager.get_line_profile(&line.binding().line_id);
        if !profile.enabled || !profile.volte_connection_enabled {
            line.volte
                .update(|state| {
                    state.recovery_state =
                        crate::connectivity::modems::ims::volte::runtime::VolteRecoveryState::Idle;
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
                LineModemWait::Cancelled | LineModemWait::Deferred => return,
            }
        }
        let binding = line.binding();
        let device =
            match crate::connectivity::modems::ims::volte::live::VolteDeviceBinding::from_modem(
                &binding,
            ) {
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
                    crate::connectivity::modems::ims::volte::runtime::VolteRecoveryState::Connecting;
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
                Ok(data_slot_mode) => match ims_override_for_line(app, &binding.line_id).await {
                    Err(error) => Err(
                        crate::connectivity::modems::ims::volte::VolteError::with_detail(
                            "volte_sim_override_not_ready",
                            error,
                        ),
                    ),
                    Ok((_, sim_override)) => {
                        let _guard = line.volte_connect_lock.lock().await;
                        if line.volte.status().await.registered {
                            return;
                        }
                        let ip_families = app
                            .config_manager
                            .get_line_volte_ip_families(&binding.line_id);
                        crate::connectivity::modems::ims::volte::live::connect_live_for_line(
                            &line.volte_live,
                            &device,
                            &line.volte,
                            app.config_manager
                                .get_line_volte_voice_enabled(&binding.line_id),
                            &ip_families,
                            profile.roaming_allowed,
                            data_slot_mode,
                            app.config_manager
                                .get_line_sms_path_policy(&binding.line_id)
                                .dedupe_enabled,
                            profile_store(app),
                            sim_override,
                            Arc::clone(&app.database),
                            Arc::clone(&app.notification_sender),
                        )
                        .await
                    }
                },
                Err(error) => Err(error),
            }
        };
        match result {
            Ok(_) => {
                line.volte
                    .update(|state| {
                        state.recovery_state =
                            crate::connectivity::modems::ims::volte::runtime::VolteRecoveryState::Registered;
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
                if crate::connectivity::modems::ims::volte::plan::FailureClass::from_details(
                    error.detail().unwrap_or_default(),
                ) == crate::connectivity::modems::ims::volte::plan::FailureClass::BasebandWedged
                {
                    warn!(
                        line_id = %binding.line_id,
                        error = %error,
                        "VoLTE IMS restore aborted: the baseband refused the session in a way that is unsafe to retry"
                    );
                    line.volte
                        .update(|state| {
                            state.phase = crate::connectivity::modems::ims::volte::runtime::VoltePhase::Degraded;
                            state.recovery_state =
                                crate::connectivity::modems::ims::volte::runtime::VolteRecoveryState::Exhausted;
                            state.manual_retry_available = true;
                            state.next_retry_at = None;
                            state.last_error = Some(format!("volte_baseband_wedged:{error}"));
                            state.last_failure_at = Some(chrono::Utc::now().to_rfc3339());
                        })
                        .await;
                    return;
                }
                if attempt < max_attempts {
                    let delay = restore_policy.retry_delay_secs.clamp(5, 180);
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
            state.phase = crate::connectivity::modems::ims::volte::runtime::VoltePhase::Degraded;
            state.recovery_state =
                crate::connectivity::modems::ims::volte::runtime::VolteRecoveryState::Exhausted;
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
        let started_at = Instant::now();
        loop {
            let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
            for profile in app
                .config_manager
                .get_line_profiles()
                .iter()
                .filter(|profile| profile.enabled && profile.volte_connection_enabled)
            {
                if started_at.elapsed()
                    < Duration::from_secs(
                        profile.volte_auto_restore.initial_delay_secs.clamp(5, 300),
                    )
                {
                    continue;
                }
                let Some(line) = app.line_registry.get(&profile.line_id).await else {
                    continue;
                };
                if !line.binding().present {
                    continue;
                }
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

fn persist_vowifi_runtime_snapshot(app: &AppState, line_id: &str, status: &VowifiStatusResponse) {
    let profile_meta = status.profile.profile.as_ref();
    if let Err(err) =
        app.database
            .upsert_vowifi_runtime_snapshot(crate::platform::db::NewVowifiRuntimeSnapshot {
                line_id,
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
    Path(line_id): Path<String>,
    Query(query): Query<VowifiStatusQuery>,
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VowifiStatusResponse>>) {
    let line_id = match resolve_vowifi_diagnostic_line_id(&app, &line_id).await {
        Ok(line_id) => line_id,
        Err(error) => return (StatusCode::BAD_REQUEST, Json(ApiResponse::error(error))),
    };
    let status = current_vowifi_status(&app, &line_id, query.live.unwrap_or(true)).await;
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", status)),
    )
}

async fn vowifi_diagnostics_for_line(
    app: &AppState,
    line_id: &str,
    query: &VowifiListQuery,
) -> Result<VowifiDiagnosticsResponse, String> {
    let status = current_vowifi_status(app, line_id, query.live.unwrap_or(true)).await;
    let trace_filter = query
        .trace_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let persisted_snapshot = app
        .database
        .get_vowifi_runtime_snapshot_for_line(line_id)
        .map_err(|error| error.to_string())?;
    let events = app
        .database
        .get_vowifi_runtime_events_for_line(
            line_id,
            query.limit.unwrap_or(100),
            query.offset.unwrap_or(0),
            trace_filter.as_deref(),
        )
        .map_err(|error| error.to_string())?;
    let sms_deliveries = app
        .database
        .get_vowifi_sms_deliveries_for_line(line_id, 200, 0)
        .map_err(|error| error.to_string())?;
    let soak_runs = app
        .database
        .get_vowifi_soak_runs_for_line(line_id, 20, 0)
        .map_err(|error| error.to_string())?;
    let restore = app
        .database
        .get_vowifi_esim_restore_for_line(line_id)
        .map_err(|error| error.to_string())?;

    Ok(vowifi_diagnostics::build_diagnostics_response(
        Some(line_id.to_string()),
        status,
        persisted_snapshot,
        events,
        sms_deliveries,
        soak_runs,
        restore,
        trace_filter,
    ))
}

pub async fn get_vowifi_diagnostics_handler(
    Path(line_id): Path<String>,
    Query(query): Query<VowifiListQuery>,
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VowifiDiagnosticsResponse>>) {
    let line_id = match resolve_vowifi_diagnostic_line_id(&app, &line_id).await {
        Ok(line_id) => line_id,
        Err(error) => return (StatusCode::BAD_REQUEST, Json(ApiResponse::error(error))),
    };
    match vowifi_diagnostics_for_line(&app, &line_id, &query).await {
        Ok(diagnostics) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", diagnostics)),
        ),
        Err(error) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {error}"))),
        ),
    }
}

pub async fn get_vowifi_events_handler(
    Path(line_id): Path<String>,
    Query(query): Query<VowifiListQuery>,
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VowifiRuntimeEventsResponse>>) {
    let line_id = match resolve_vowifi_diagnostic_line_id(&app, &line_id).await {
        Ok(line_id) => line_id,
        Err(error) => return (StatusCode::BAD_REQUEST, Json(ApiResponse::error(error))),
    };
    let result = app.database.get_vowifi_runtime_events_for_line(
        &line_id,
        query.limit.unwrap_or(100),
        query.offset.unwrap_or(0),
        query.trace_id.as_deref(),
    );
    match result {
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
    Path(line_id): Path<String>,
    Query(query): Query<VowifiListQuery>,
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VowifiSoakRunsResponse>>) {
    let line_id = match resolve_vowifi_diagnostic_line_id(&app, &line_id).await {
        Ok(line_id) => line_id,
        Err(error) => return (StatusCode::BAD_REQUEST, Json(ApiResponse::error(error))),
    };
    let result = app.database.get_vowifi_soak_runs_for_line(
        &line_id,
        query.limit.unwrap_or(20),
        query.offset.unwrap_or(0),
    );
    match result {
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
    Path(line_id): Path<String>,
    Query(query): Query<VowifiListQuery>,
    State(app): State<AppState>,
) -> (StatusCode, Json<ApiResponse<VowifiSmsDeliveriesResponse>>) {
    let line_id = match resolve_vowifi_diagnostic_line_id(&app, &line_id).await {
        Ok(line_id) => line_id,
        Err(error) => return (StatusCode::BAD_REQUEST, Json(ApiResponse::error(error))),
    };
    let result = app.database.get_vowifi_sms_deliveries_for_line(
        &line_id,
        query.limit.unwrap_or(50),
        query.offset.unwrap_or(0),
    );
    match result {
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
    Path((line_id, message_id)): Path<(String, String)>,
    State(app): State<AppState>,
) -> (
    StatusCode,
    Json<ApiResponse<Option<crate::platform::db::VowifiSmsDeliveryEntry>>>,
) {
    let line_id = match resolve_vowifi_diagnostic_line_id(&app, &line_id).await {
        Ok(line_id) => line_id,
        Err(error) => return (StatusCode::BAD_REQUEST, Json(ApiResponse::error(error))),
    };
    let result = app
        .database
        .get_vowifi_sms_delivery_for_line(&line_id, &message_id);
    match result {
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
    Path(line_id): Path<String>,
    State(app): State<AppState>,
) -> (
    StatusCode,
    Json<ApiResponse<Option<VowifiEsimRestoreEntry>>>,
) {
    let line_id = match resolve_vowifi_diagnostic_line_id(&app, &line_id).await {
        Ok(line_id) => line_id,
        Err(error) => return (StatusCode::BAD_REQUEST, Json(ApiResponse::error(error))),
    };
    let result = app.database.get_vowifi_esim_restore_for_line(&line_id);
    match result {
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
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<VoicePathPolicy>>) {
    if resolve_control_line(&app, &line_id).await.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    }
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            app.config_manager.get_line_voice_path_policy(&line_id),
        )),
    )
}

pub async fn set_voice_path_policy_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<VoicePathPolicy>,
) -> (StatusCode, Json<ApiResponse<VoicePathPolicy>>) {
    let Some(line) = resolve_control_line(&app, &line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    match app
        .config_manager
        .set_line_voice_path_policy(&line_id, payload)
    {
        Ok(policy) => {
            line.voice_access.set_policy(policy.clone());
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message("Saved", policy)),
            )
        }
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
    let dbus_conn = Arc::clone(&app.dbus_conn);
    tokio::spawn(async move {
        run_safe_os_reboot_sequence(delay, dbus_conn, system_events).await;
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
    dbus_conn: Arc<Connection>,
    system_events: Arc<crate::services::system::system_event::SystemEventEmitter>,
) {
    if delay_seconds > 0 {
        tokio::time::sleep(tokio::time::Duration::from_secs(delay_seconds as u64)).await;
    }

    info!("Starting safe OS reboot sequence");

    if let Some(message) = disable_all_modem_radios_for_reboot(&dbus_conn).await {
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

async fn disable_all_modem_radios_for_reboot(conn: &Connection) -> Option<String> {
    let modem_paths = match modem_manager::list_modem_paths(conn).await {
        Ok(paths) if !paths.is_empty() => paths,
        Ok(_) => return Some("重启预处理步骤失败: ModemManager 未枚举到任何基带".to_string()),
        Err(error) => {
            return Some(format!(
                "重启预处理步骤失败: 无法枚举 ModemManager 基带: {error}"
            ))
        }
    };

    let mut failures = Vec::new();
    for modem_path in modem_paths {
        match modem_manager::set_modem_enabled(conn, &modem_path, false).await {
            Ok(_) => info!(modem_path = %modem_path, "Disabled modem radio for safe OS reboot"),
            Err(error) => failures.push(format!("{modem_path}: {error}")),
        }
    }
    if failures.is_empty() {
        None
    } else {
        Some(format!(
            "重启预处理步骤失败: 部分基带射频未关闭: {}",
            failures.join("; ")
        ))
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

/// Restart the system ModemManager service without restarting SimAdmin or the OS.
pub async fn restart_modem_manager_handler(State(app): State<AppState>) -> impl IntoResponse {
    let output = tokio::task::spawn_blocking(|| {
        Command::new("systemctl")
            .args(["restart", "ModemManager.service"])
            .output()
    })
    .await;

    let result = match output {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(output)) => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(error.to_string()),
    };

    match result {
        Ok(()) => {
            app.system_event_emitter
                .emit_code(
                    system_event_codes::BASEBAND_MODEMMANAGER_RESTARTED,
                    system_event_severity::WARNING,
                    system_event_status::SUCCEEDED,
                    "ModemManager.service",
                    "用户手动重启 ModemManager.service",
                )
                .await;
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "ModemManager service restarted",
                    json!({}),
                )),
            )
        }
        Err(error) => {
            app.system_event_emitter
                .emit_code(
                    system_event_codes::BASEBAND_MODEMMANAGER_RESTART_FAILED,
                    system_event_severity::CRITICAL,
                    system_event_status::FAILED,
                    "ModemManager.service",
                    format!("用户手动重启 ModemManager.service 失败: {error}"),
                )
                .await;
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<serde_json::Value>::error(format!(
                    "重启 ModemManager.service 失败: {error}"
                ))),
            )
        }
    }
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
    pub line_id: String,
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
    pub line_id: String,
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
        &query.line_id,
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
        &filters.line_id,
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
    State(app): State<AppState>,
    Json(req): Json<crate::api::models::OtaOnlinePrepareRequest>,
) -> impl IntoResponse {
    let result: Result<crate::api::models::OtaLatestReleaseResponse, String> = async {
        let proxy_prefix = requested_github_proxy_prefix(&app, req.proxy_prefix);
        let client = crate::services::system::ota::build_ota_http_client()?;

        crate::services::system::ota::fetch_latest_github_release(&client, &proxy_prefix, false)
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
    State(app): State<AppState>,
    Json(req): Json<crate::api::models::OtaOnlinePrepareRequest>,
) -> impl IntoResponse {
    let result: Result<crate::api::models::OtaUploadResponse, String> = async {
        let proxy_prefix = requested_github_proxy_prefix(&app, req.proxy_prefix);
        let client = crate::services::system::ota::build_ota_http_client()?;

        let release = crate::services::system::ota::fetch_latest_github_release(
            &client,
            &proxy_prefix,
            false,
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
            false,
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
    pub line_id: String,
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
    pub line_id: String,
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
        &query.line_id,
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
        &filters.line_id,
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

    let start_result = crate::services::automation::spawn_automation_task(
        app_state,
        Arc::new(crate::services::automation::tasks::TaskRegistry::new()),
        task,
    );
    if start_result == crate::services::automation::AutomationStartResult::AlreadyRunning {
        return (
            StatusCode::OK,
            Json(ApiResponse::error("该任务或目标线路已有自动化任务正在执行")),
        );
    }

    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "任务已在后台下发立即执行",
            json!({}),
        )),
    )
}

// =====================================================================
// P1.4 — per-SIM IMS override and effective profile API
// =====================================================================

use crate::connectivity::modems::ims::effective_profile::{
    EffectiveServices, EffectiveVowifiProfile,
};
use crate::connectivity::modems::ims::profile_override::{
    OverrideSource, SimBindingKey, SimOverride,
};
use crate::connectivity::modems::ims::vowifi::carrier_catalog::CatalogAccessKind;
use crate::connectivity::modems::ims::vowifi::profiles::CarrierProfile;

/// Resolve the current SIM binding for a line. The binding is re-read here and
/// again before a write so a card swap during editing cannot target another SIM.
async fn resolve_ims_binding(
    app: &AppState,
    line_id: &str,
) -> Result<
    (
        SimBindingKey,
        crate::hardware::cellular::modem_manager::ModemBinding,
    ),
    String,
> {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let line = app
        .line_registry
        .get(line_id)
        .await
        .ok_or_else(|| "line_not_found".to_string())?;
    let binding = line.binding();
    let iccid = if binding.sim_iccid.trim().is_empty() {
        None
    } else {
        Some(binding.sim_iccid.as_str())
    };
    let eid = if binding.sim_type == "esim" {
        app.esim_supervisor
            .get_euicc_info_for_line(line_id)
            .await
            .ok()
            .map(|info| info.eid)
    } else {
        None
    };
    let key = SimBindingKey::resolve(iccid, eid.as_deref()).map_err(|error| error.to_string())?;
    Ok((key, binding))
}

fn effective_vowifi_dto(profile: &EffectiveVowifiProfile) -> EffectiveVowifiDto {
    EffectiveVowifiDto {
        profile_id: profile.profile_id.clone(),
        epdg_host: FieldDto {
            value: profile.epdg_host.value.clone(),
            source: source_str(profile.epdg_host.source),
        },
        epdg_port: profile.epdg_port,
        epdg_port_source: source_str(profile.epdg_port_source),
        apn: profile.apn.as_ref().map(|apn| FieldDto {
            value: apn.value.clone(),
            source: source_str(apn.source),
        }),
        ip_stack: FieldDto {
            value: profile.ip_stack.value.clone(),
            source: source_str(profile.ip_stack.source),
        },
        dns_servers: profile
            .dns_servers
            .iter()
            .map(|server| FieldDto {
                value: server.value.clone(),
                source: source_str(server.source),
            })
            .collect(),
    }
}

fn source_str(source: OverrideSource) -> String {
    match source {
        OverrideSource::Catalog => "catalog".to_string(),
        OverrideSource::SimOverride => "sim_override".to_string(),
        OverrideSource::Modem => "modem".to_string(),
        OverrideSource::Network => "network".to_string(),
    }
}

/// Map the effective device identity origin to the `custom|modem|unavailable`
/// vocabulary used by the P2 plan and logs. The raw IMEI is never returned.
fn device_identity_source_str(source: OverrideSource) -> String {
    match source {
        OverrideSource::SimOverride => "custom".to_string(),
        OverrideSource::Modem => "modem".to_string(),
        OverrideSource::Catalog | OverrideSource::Network => "unavailable".to_string(),
    }
}

fn source_opt_str(source: Option<OverrideSource>) -> Option<String> {
    source.map(source_str)
}

fn binding_dto(key: &SimBindingKey) -> BindingKeyDto {
    let iccid = key.iccid();
    BindingKeyDto {
        kind: match key {
            SimBindingKey::Plain { .. } => "plain".to_string(),
            SimBindingKey::Euicc { .. } => "euicc".to_string(),
        },
        iccid_last4: (!iccid.is_empty()).then(|| {
            let len = iccid.chars().count();
            if len <= 4 {
                iccid.to_string()
            } else {
                iccid.chars().skip(len - 4).collect()
            }
        }),
    }
}

fn identifier_last4(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let len = value.chars().count();
    Some(if len <= 4 {
        value.to_string()
    } else {
        value.chars().skip(len - 4).collect()
    })
}

/// Resolve the carrier catalog baseline for a line, honoring a pinned
/// `profile_id` from the override before falling back to IMSI/PLMN matching.
fn resolve_ims_catalog(
    app: &AppState,
    imsi: Option<&str>,
    pinned: Option<&str>,
    access: CatalogAccessKind,
) -> Option<&'static CarrierProfile> {
    if let Some(profile_id) = pinned.filter(|value| !value.trim().is_empty()) {
        if let Ok(Some(profile)) = app.carrier_catalog.get(profile_id, access) {
            if let Some(interned) = catalog_profile_to_static(&profile) {
                return Some(interned);
            }
        }
    }
    if let Some(imsi) = imsi {
        if let Ok(Some(profile)) = app.carrier_catalog.resolve_for_imsi(imsi, None, access) {
            if let Some(interned) = catalog_profile_to_static(&profile) {
                return Some(interned);
            }
        }
    }
    None
}

fn catalog_profile_to_static(
    profile: &crate::connectivity::modems::ims::vowifi::carrier_catalog::CatalogProfile,
) -> Option<&'static CarrierProfile> {
    Some(profile.record.intern())
}

async fn query_sim_voicemail_number(modem_id: &str) -> Option<String> {
    if modem_id.trim().is_empty() {
        return None;
    }
    let output = tokio::time::timeout(
        Duration::from_secs(4),
        tokio::process::Command::new("mmcli")
            .args(["-m", modem_id, "--command=AT+CSVM?"])
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_csvm_voicemail_number(&String::from_utf8_lossy(&output.stdout))
}

fn parse_csvm_voicemail_number(output: &str) -> Option<String> {
    let payload = output.split("+CSVM:").nth(1)?;
    let start = payload.find('"')? + 1;
    let end = payload[start..].find('"')? + start;
    let number = payload[start..end].trim();
    (!number.is_empty()
        && number.len() <= 32
        && number
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'+' | b'*' | b'#')))
    .then(|| number.to_string())
}

fn build_effective_response(
    app: &AppState,
    key: &SimBindingKey,
    binding: &crate::hardware::cellular::modem_manager::ModemBinding,
    imsi: Option<&str>,
    sim_voicemail_number: Option<&str>,
) -> Result<EffectiveImsProfileResponse, String> {
    let override_ = app
        .sim_overrides
        .load(key)
        .map_err(|error| error.to_string())?;
    let volte_pinned = override_
        .as_ref()
        .and_then(|o| o.ims_volte.profile_id.as_deref());
    let vowifi_pinned = override_
        .as_ref()
        .and_then(|o| o.ims_vowifi.profile_id.as_deref());
    let volte_catalog = resolve_ims_catalog(app, imsi, volte_pinned, CatalogAccessKind::LteEpc)
        .ok_or_else(|| "volte_carrier_profile_not_resolved".to_string())?;
    let vowifi_catalog = resolve_ims_catalog(app, imsi, vowifi_pinned, CatalogAccessKind::WifiEpdg)
        .ok_or_else(|| "vowifi_carrier_profile_not_resolved".to_string())?;

    let vowifi =
        crate::connectivity::modems::ims::effective_profile::resolve_effective_vowifi_profile(
            vowifi_catalog,
            override_.as_ref(),
        );
    let volte_ims =
        crate::connectivity::modems::ims::effective_profile::resolve_effective_ims_profile(
            volte_catalog,
            override_.as_ref(),
        );
    let vowifi_ims =
        crate::connectivity::modems::ims::effective_profile::resolve_effective_vowifi_ims_profile(
            vowifi_catalog,
            override_.as_ref(),
        );
    let identity =
        crate::connectivity::modems::ims::effective_profile::resolve_effective_device_identity(
            override_.as_ref(),
            (!binding.equipment_identifier.trim().is_empty())
                .then_some(binding.equipment_identifier.as_str()),
        );
    let common =
        crate::connectivity::modems::ims::effective_profile::resolve_effective_common_with_sources(
            override_.as_ref(),
            sim_voicemail_number,
            vowifi_catalog
                .voice
                .voicemail_number
                .or(volte_catalog.voice.voicemail_number),
        );
    let services = EffectiveServices::from_override(override_.as_ref());
    let emergency =
        crate::connectivity::modems::ims::effective_profile::resolve_effective_emergency(
            override_.as_ref(),
        );
    let source_map = crate::connectivity::modems::ims::effective_profile::source_map_of(
        &vowifi,
        &volte_ims,
        &vowifi_ims,
        &identity,
        &common,
        &services,
        &emergency,
    );

    Ok(EffectiveImsProfileResponse {
        binding: binding_dto(key),
        vowifi: effective_vowifi_dto(&vowifi),
        volte_ims: effective_ims_dto(&volte_ims),
        vowifi_ims: effective_ims_dto(&vowifi_ims),
        identity: EffectiveDeviceIdentityDto {
            available: identity.imei.is_some(),
            imei_last4: identity.imei.as_deref().and_then(identifier_last4),
            source: device_identity_source_str(identity.source),
        },
        common: EffectiveCommonDto {
            voicemail_number: common.voicemail_number.clone(),
            voicemail_number_source: source_opt_str(common.voicemail_number_source),
        },
        services: EffectiveServicesDto {
            call_waiting: services.call_waiting,
            call_waiting_source: source_opt_str(services.call_waiting_source),
            caller_id_restriction: services.caller_id_restriction,
            caller_id_restriction_source: source_opt_str(services.caller_id_restriction_source),
        },
        emergency: EffectiveEmergencyDto {
            address_saved_locally: emergency.address_saved_locally,
            address_source: source_opt_str(emergency.address_source),
        },
        source_map: source_map
            .into_iter()
            .map(|(field, source)| SourceEntryDto {
                field,
                source: source_str(source),
            })
            .collect(),
    })
}

fn effective_ims_dto(
    profile: &crate::connectivity::modems::ims::effective_profile::EffectiveImsProfile,
) -> EffectiveImsDto {
    EffectiveImsDto {
        profile_id: profile.profile_id.clone(),
        domain: FieldDto {
            value: profile.domain.value.clone(),
            source: source_str(profile.domain.source),
        },
        realm: FieldDto {
            value: profile.realm.value.clone(),
            source: source_str(profile.realm.source),
        },
        pcscf: profile.pcscf.as_ref().map(|pcscf| FieldDto {
            value: pcscf.value.clone(),
            source: source_str(pcscf.source),
        }),
        registrar: profile.registrar.as_ref().map(|registrar| FieldDto {
            value: registrar.value.clone(),
            source: source_str(registrar.source),
        }),
        ims_apn: profile.ims_apn.as_ref().map(|apn| FieldDto {
            value: apn.value.clone(),
            source: source_str(apn.source),
        }),
        pinned_profile_id: profile.pinned_profile_id.as_ref().map(|pinned| FieldDto {
            value: pinned.value.clone(),
            source: source_str(pinned.source),
        }),
    }
}

/// Normalize a user-submitted override before persistence. Whitespace-only
/// values are treated as unset so the API never writes empty strings.
fn normalize_ims_override_payload(mut payload: SimOverride) -> SimOverride {
    if let Some(imei) = payload.ims_common.custom_imei.as_deref() {
        let trimmed = imei.trim();
        if trimmed.is_empty() {
            payload.ims_common.custom_imei = None;
        } else if trimmed != imei {
            payload.ims_common.custom_imei = Some(trimmed.to_string());
        }
    }
    if let Some(number) = payload.ims_common.voicemail_number.as_deref() {
        let trimmed = number.trim();
        if trimmed.is_empty() {
            payload.ims_common.voicemail_number = None;
        } else if trimmed != number {
            payload.ims_common.voicemail_number = Some(trimmed.to_string());
        }
    }
    if let Some(address) = payload.emergency.e911_address.as_deref() {
        let trimmed = address.trim();
        if trimmed.is_empty() {
            payload.emergency.e911_address = None;
        } else if trimmed != address {
            payload.emergency.e911_address = Some(trimmed.to_string());
        }
    }
    if !payload.ims_vowifi.spoof_imsi {
        payload.ims_vowifi.custom_imsi = None;
    } else if let Some(imsi) = payload.ims_vowifi.custom_imsi.as_deref() {
        let trimmed = imsi.trim();
        if trimmed.is_empty() {
            payload.ims_vowifi.custom_imsi = None;
            payload.ims_vowifi.spoof_imsi = false;
        } else if trimmed != imsi {
            payload.ims_vowifi.custom_imsi = Some(trimmed.to_string());
        }
    }
    payload
}

async fn ims_override_for_line(
    app: &AppState,
    line_id: &str,
) -> Result<(SimBindingKey, SimOverride), String> {
    let (key, _binding) = resolve_ims_binding(app, line_id).await?;
    let override_ = app
        .sim_overrides
        .load(&key)
        .map_err(|error| error.to_string())?;
    Ok((key, override_.unwrap_or_default()))
}

fn override_response(key: &SimBindingKey, override_: &SimOverride) -> ImsOverrideResponse {
    ImsOverrideResponse {
        binding: binding_dto(key),
        override_: override_.clone(),
    }
}

/// GET /api/ims/lines/{line_id}/profile
pub async fn get_effective_ims_profile_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<EffectiveImsProfileResponse>>) {
    let (key, binding) = match resolve_ims_binding(&app, &line_id).await {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<EffectiveImsProfileResponse>::error(reason)),
            );
        }
    };
    let imsi = crate::hardware::cellular::modem_manager::sim_identity_for_modem(
        app.dbus_conn.as_ref(),
        &binding.modem_path,
    )
    .await
    .map(|identity| identity.imsi);
    let sim_voicemail_number = query_sim_voicemail_number(&binding.modem_id).await;
    match build_effective_response(
        &app,
        &key,
        &binding,
        imsi.as_deref(),
        sim_voicemail_number.as_deref(),
    ) {
        Ok(response) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", response)),
        ),
        Err(reason) => (
            StatusCode::OK,
            Json(ApiResponse::<EffectiveImsProfileResponse>::error(reason)),
        ),
    }
}

/// GET /api/ims/lines/{line_id}/supplementary
pub async fn get_ims_supplementary_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (
    StatusCode,
    Json<ApiResponse<crate::services::supplementary::SupplementarySnapshot>>,
) {
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<
                crate::services::supplementary::SupplementarySnapshot,
            >::error("line_not_found")),
        );
    };
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            line.supplementary.snapshot().await,
        )),
    )
}

fn local_voice_media_offer(
    profile: &'static CarrierProfile,
    local_ip: std::net::IpAddr,
) -> crate::services::trunk::bridge::MediaOffer {
    let params = crate::connectivity::modems::ims::vowifi::voice::voice_params(profile);
    let audio = crate::connectivity::core::voice::build_mo_audio_offer_with_params(
        &params,
        &local_ip.to_string(),
        crate::connectivity::core::voice::SdpAddrType::Ip4,
        LOCAL_VOICE_API_MEDIA_PORT,
    );
    crate::services::trunk::bridge::MediaOffer {
        audio,
        audio_endpoint: std::net::SocketAddr::new(local_ip, LOCAL_VOICE_API_MEDIA_PORT),
        video: None,
        dtmf: crate::services::trunk::bridge::DtmfCapabilities {
            rtp_event: None,
            sip_info: true,
            preferred: crate::services::trunk::bridge::DtmfSource::SipInfo,
        },
    }
}

/// POST /api/ims/lines/{line_id}/voicemail/call
///
/// Queue a voicemail call using this line's current voice-access policy. The
/// endpoint deliberately does not call a VoWiFi/VoLTE live adapter directly:
/// `VoiceAccessRouter` picks the registered IMS leg and records the route, so
/// every later SIP event, DTMF command, cancellation, and failover keeps the
/// normal per-line lifecycle.
///
/// There is no browser media endpoint in this API. Its RTP target is the local
/// reserved sink used by the existing direct-call API; a trunk/audio backend
/// may attach there later without changing this signaling contract.
pub async fn place_voicemail_call_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let (binding_key, binding) = match resolve_ims_binding(&app, &line_id).await {
        Ok(context) => context,
        Err(reason) => return (StatusCode::NOT_FOUND, Json(ApiResponse::error(reason))),
    };
    if !binding.present {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("line_not_present")),
        );
    }
    let line_profile = app.config_manager.get_line_profile(&line_id);
    if !line_profile.enabled {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("line_disabled")),
        );
    }
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    let override_ = match app.sim_overrides.load(&binding_key) {
        Ok(value) => value.unwrap_or_default(),
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::error(error.to_string())),
            )
        }
    };

    let imsi = crate::hardware::cellular::modem_manager::sim_identity_for_modem(
        app.dbus_conn.as_ref(),
        &binding.modem_path,
    )
    .await
    .map(|identity| identity.imsi);
    let volte_catalog = resolve_ims_catalog(
        &app,
        imsi.as_deref(),
        override_.ims_volte.profile_id.as_deref(),
        CatalogAccessKind::LteEpc,
    );
    let vowifi_catalog = resolve_ims_catalog(
        &app,
        imsi.as_deref(),
        override_.ims_vowifi.profile_id.as_deref(),
        CatalogAccessKind::WifiEpdg,
    );
    let sim_voicemail_number = query_sim_voicemail_number(&binding.modem_id).await;
    let common =
        crate::connectivity::modems::ims::effective_profile::resolve_effective_common_with_sources(
            Some(&override_),
            sim_voicemail_number.as_deref(),
            vowifi_catalog
                .and_then(|profile| profile.voice.voicemail_number)
                .or_else(|| volte_catalog.and_then(|profile| profile.voice.voicemail_number)),
        );
    let Some(voicemail_number) = common.voicemail_number else {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("voicemail_number_unavailable")),
        );
    };
    let voicemail_number =
        match crate::connectivity::core::voice::normalize_ims_dial_user(&voicemail_number) {
            Ok(value) => value,
            Err(_) => {
                return (
                    StatusCode::CONFLICT,
                    Json(ApiResponse::error("voicemail_number_invalid")),
                )
            }
        };

    let local_ip = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
    let call_id = format!("{}@simadmin", crate::services::trunk::sip::token(16));
    let mut plan = crate::services::trunk::access_router::VoiceCallPlan::new(
        call_id,
        "simadmin",
        voicemail_number,
        local_ip,
    );
    if let Some(profile) = vowifi_catalog {
        plan = plan.with_offer(
            AccessPathKind::Vowifi,
            local_voice_media_offer(profile, local_ip),
        );
    }
    if let Some(profile) = volte_catalog {
        plan = plan.with_offer(
            AccessPathKind::Volte,
            local_voice_media_offer(profile, local_ip),
        );
    }

    match line.voice_access.start_call(plan).await {
        Ok(queued) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Voicemail call queued",
                json!({
                    "line_id": line_id,
                    "call_id": queued.call_id,
                    "access": queued.access.as_str(),
                    "voicemail_number_source": common
                        .voicemail_number_source
                        .map(source_str),
                    "call_state": "dialing",
                    "invite_state": "queued",
                    "media_followup": "operator_link",
                }),
            )),
        ),
        Err(error) => (StatusCode::CONFLICT, Json(ApiResponse::error(error.code()))),
    }
}

fn parse_ut_document_kind(value: &str) -> Option<crate::connectivity::core::ut::UtDocumentKind> {
    use crate::connectivity::core::ut::UtDocumentKind;
    match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "communication-waiting" | "call-waiting" => Some(UtDocumentKind::CommunicationWaiting),
        "communication-diversion" | "call-forwarding" => {
            Some(UtDocumentKind::CommunicationDiversion)
        }
        "originating-identity-presentation" | "oip" | "clip" => {
            Some(UtDocumentKind::OriginatingIdentityPresentation)
        }
        "originating-identity-presentation-restriction"
        | "originating-identity-restriction"
        | "oir"
        | "clir" => Some(UtDocumentKind::OriginatingIdentityRestriction),
        _ => None,
    }
}

async fn xcap_client_for_line(
    line: &crate::services::line_registry::LineRuntime,
) -> Result<
    (
        crate::services::supplementary::ut::HttpXcapTransport,
        crate::connectivity::core::ut::XcapPolicy,
        crate::connectivity::core::registration::ImsRegistrationAccess,
    ),
    &'static str,
> {
    use crate::platform::config::AccessPathKind;
    use crate::services::supplementary::ut::{
        xcap_policy_from_carrier, HttpXcapTransport, XcapAccessContext,
    };

    let preferred = line.voice_access.preferred_ready_ims_access();
    let mut order = Vec::with_capacity(2);
    if let Some(access) = preferred {
        order.push(access);
    }
    for access in [AccessPathKind::Vowifi, AccessPathKind::Volte] {
        if !order.contains(&access) {
            order.push(access);
        }
    }

    let mut selected: Option<XcapAccessContext> = None;
    for access in order {
        let context = match access {
            AccessPathKind::Vowifi => live_xcap_access_for_line(&line.binding().line_id).await,
            AccessPathKind::Volte => line.volte_live.live_xcap_access().await,
            AccessPathKind::Cs => None,
        };
        if context.is_some() {
            selected = context;
            break;
        }
    }
    let context = selected.ok_or("ut_ims_registration_required")?;
    let policy = xcap_policy_from_carrier(context.profile)
        .map_err(|error| error.code())?
        .ok_or("ut_xcap_not_configured")?;
    let transport = HttpXcapTransport::new(Some(context.local_address), &policy)
        .map_err(|error| error.code())?
        .with_digest_provider(context.digest);
    Ok((transport, policy, context.access))
}

/// GET /api/ims/lines/{line_id}/ut/{document}
pub async fn get_ims_ut_document_handler(
    State(app): State<AppState>,
    Path((line_id, document)): Path<(String, String)>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let Some(kind) = parse_ut_document_kind(&document) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("ut_document_kind_invalid")),
        );
    };
    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    let (transport, policy, access) = match xcap_client_for_line(&line).await {
        Ok(client) => client,
        Err(reason) => return (StatusCode::CONFLICT, Json(ApiResponse::error(reason))),
    };
    line.supplementary.begin_ut_request(access, kind).await;
    match crate::services::supplementary::ut::read_document(&transport, &policy, kind).await {
        Ok(document) => {
            line.supplementary.mark_ut_document(access, &document).await;
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Success",
                    json!({
                        "access": match access {
                            crate::connectivity::core::registration::ImsRegistrationAccess::Volte => "volte",
                            crate::connectivity::core::registration::ImsRegistrationAccess::Vowifi => "vowifi",
                        },
                        "document": document,
                    }),
                )),
            )
        }
        Err(error) => {
            line.supplementary
                .fail_ut_request(access, kind, error.code())
                .await;
            (
                StatusCode::BAD_GATEWAY,
                Json(ApiResponse::error(error.code())),
            )
        }
    }
}

/// PUT /api/ims/lines/{line_id}/ut/{document}
pub async fn put_ims_ut_document_handler(
    State(app): State<AppState>,
    Path((line_id, document)): Path<(String, String)>,
    Json(payload): Json<UpdateImsUtRequest>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    use crate::connectivity::core::ut::{UtDocument, UtDocumentKind, UtMutation};

    let Some(kind) = parse_ut_document_kind(&document) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("ut_document_kind_invalid")),
        );
    };
    let supplied = usize::from(payload.call_waiting.is_some())
        + usize::from(payload.forwarding_rule.is_some())
        + usize::from(payload.identity_presentation.is_some());
    let matches_kind = match kind {
        UtDocumentKind::CommunicationWaiting => payload.call_waiting.is_some(),
        UtDocumentKind::CommunicationDiversion => payload.forwarding_rule.is_some(),
        UtDocumentKind::OriginatingIdentityPresentation
        | UtDocumentKind::OriginatingIdentityRestriction => payload.identity_presentation.is_some(),
    };
    if supplied != 1 || !matches_kind {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("ut_update_field_mismatch")),
        );
    }
    if let Some(rule) = payload.forwarding_rule.as_ref() {
        let mut validation = UtDocument::empty(UtDocumentKind::CommunicationDiversion);
        if let Err(error) = validation.set_forwarding_rule(rule.clone()) {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error(error.code())),
            );
        }
    }
    if let Some(presentation) = payload.identity_presentation {
        let mut validation = UtDocument::empty(kind);
        if let Err(error) = validation.set_identity_presentation(presentation) {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error(error.code())),
            );
        }
    }

    let _ = app.line_registry.refresh(app.dbus_conn.as_ref()).await;
    let Some(line) = app.line_registry.get(&line_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("line_not_found")),
        );
    };
    let (transport, policy, access) = match xcap_client_for_line(&line).await {
        Ok(client) => client,
        Err(reason) => return (StatusCode::CONFLICT, Json(ApiResponse::error(reason))),
    };
    line.supplementary.begin_ut_request(access, kind).await;
    let mutation = match kind {
        UtDocumentKind::CommunicationWaiting => {
            UtMutation::CallWaiting(payload.call_waiting.expect("validated field"))
        }
        UtDocumentKind::CommunicationDiversion => {
            UtMutation::ForwardingRule(payload.forwarding_rule.expect("validated field"))
        }
        UtDocumentKind::OriginatingIdentityPresentation
        | UtDocumentKind::OriginatingIdentityRestriction => UtMutation::IdentityPresentation(
            payload.identity_presentation.expect("validated field"),
        ),
    };
    let result =
        crate::services::supplementary::ut::update_document(&transport, &policy, kind, mutation)
            .await;
    match result {
        Ok(outcome) => {
            line.supplementary
                .mark_ut_document(access, &outcome.document)
                .await;
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Updated and read back",
                    json!({
                        "access": match access {
                            crate::connectivity::core::registration::ImsRegistrationAccess::Volte => "volte",
                            crate::connectivity::core::registration::ImsRegistrationAccess::Vowifi => "vowifi",
                        },
                        "changed": outcome.changed,
                        "document": outcome.document,
                    }),
                )),
            )
        }
        Err(error) => {
            line.supplementary
                .fail_ut_request(access, kind, error.code())
                .await;
            (
                StatusCode::BAD_GATEWAY,
                Json(ApiResponse::error(error.code())),
            )
        }
    }
}

/// GET /api/ims/lines/{line_id}/override
pub async fn get_ims_override_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<ImsOverrideResponse>>) {
    let (key, override_) = match ims_override_for_line(&app, &line_id).await {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<ImsOverrideResponse>::error(reason)),
            );
        }
    };
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            override_response(&key, &override_),
        )),
    )
}

/// PATCH /api/ims/lines/{line_id}/override
pub async fn patch_ims_override_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<SimOverride>,
) -> (StatusCode, Json<ApiResponse<ImsOverrideResponse>>) {
    let (initial_key, _binding) = match resolve_ims_binding(&app, &line_id).await {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<ImsOverrideResponse>::error(reason)),
            );
        }
    };
    let payload = normalize_ims_override_payload(payload);
    let problems = crate::connectivity::modems::ims::effective_profile::validate_override(&payload);
    if !problems.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ApiResponse::<ImsOverrideResponse>::error(
                problems.join(","),
            )),
        );
    }
    // Re-confirm the binding key right before writing so a card swap during
    // editing cannot target another SIM.
    let (key, _binding) = match resolve_ims_binding(&app, &line_id).await {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<ImsOverrideResponse>::error(reason)),
            );
        }
    };
    if key != initial_key {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::<ImsOverrideResponse>::error(
                "sim_binding_changed_during_update",
            )),
        );
    }
    if let Err(error) = app.sim_overrides.save(&key, &payload) {
        return (
            StatusCode::OK,
            Json(ApiResponse::<ImsOverrideResponse>::error(error.to_string())),
        );
    }
    let saved = app
        .sim_overrides
        .load(&key)
        .ok()
        .flatten()
        .unwrap_or_default();
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            override_response(&key, &saved),
        )),
    )
}

/// DELETE /api/ims/lines/{line_id}/override
pub async fn delete_ims_override_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<ImsOverrideResponse>>) {
    let (key, _binding) = match resolve_ims_binding(&app, &line_id).await {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<ImsOverrideResponse>::error(reason)),
            );
        }
    };
    if let Err(error) = app.sim_overrides.delete(&key) {
        return (
            StatusCode::OK,
            Json(ApiResponse::<ImsOverrideResponse>::error(error.to_string())),
        );
    }
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            override_response(&key, &SimOverride::default()),
        )),
    )
}

/// POST /api/ims/lines/{line_id}/override/validate
pub async fn validate_ims_override_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<SimOverride>,
) -> (StatusCode, Json<ApiResponse<ImsOverrideValidationResponse>>) {
    let _ = app;
    let _ = line_id;
    let problems = crate::connectivity::modems::ims::effective_profile::validate_override(&payload);
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            ImsOverrideValidationResponse {
                valid: problems.is_empty(),
                problems,
            },
        )),
    )
}

/// Resolve the current line's binding and the carrier profile (from the pinned
/// profile or the IMSI-based catalog), returning them together with the loaded
/// override. Shared by all E911 handlers so the SIM re-confirmation rule is
/// enforced in one place.
async fn resolve_e911_context(
    app: &AppState,
    line_id: &str,
) -> Result<
    (
        SimBindingKey,
        crate::hardware::cellular::modem_manager::ModemBinding,
        &'static CarrierProfile,
        SimOverride,
    ),
    String,
> {
    let (key, binding) = resolve_ims_binding(app, line_id).await?;
    let override_ = app
        .sim_overrides
        .load(&key)
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let pinned = override_
        .ims_volte
        .profile_id
        .as_deref()
        .or_else(|| override_.ims_vowifi.profile_id.as_deref());
    let access = if override_.ims_volte.profile_id.is_some() {
        CatalogAccessKind::LteEpc
    } else {
        CatalogAccessKind::WifiEpdg
    };
    let imsi = crate::hardware::cellular::modem_manager::sim_identity_for_modem(
        app.dbus_conn.as_ref(),
        &binding.modem_path,
    )
    .await
    .map(|identity| identity.imsi);
    let catalog = resolve_ims_catalog(app, imsi.as_deref(), pinned, access)
        .ok_or_else(|| "carrier_profile_not_resolved".to_string())?;
    Ok((key, binding, catalog, override_))
}

#[derive(Clone)]
struct LineE911AkaProvider {
    qmi_device: String,
    uim_slot: u8,
    proxy_socket: String,
}

impl crate::services::e911::SimAkaProvider for LineE911AkaProvider {
    fn authenticate<'a>(
        &'a self,
        rand: &'a [u8],
        autn: &'a [u8],
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<crate::connectivity::modems::ims::vowifi::qmi_uim::UsimAkaApduResult, String>,
    > {
        let qmi_device = self.qmi_device.clone();
        let uim_slot = self.uim_slot;
        let proxy_socket = self.proxy_socket.clone();
        let rand = rand.to_vec();
        let autn = autn.to_vec();
        Box::pin(async move {
            if qmi_device.is_empty() {
                return Err("e911_sim_auth_device_unavailable".to_string());
            }
            tokio::task::spawn_blocking(move || {
                crate::connectivity::modems::ims::vowifi::qmi_uim::execute_usim_authenticate_via_proxy_reason_with_retry(
                    &proxy_socket,
                    &qmi_device,
                    uim_slot,
                    crate::connectivity::modems::ims::vowifi::qmi_uim::USIM_AID_PREFIX,
                    &rand,
                    &autn,
                    3,
                    Duration::from_secs(5),
                    Duration::from_millis(250),
                )
                .map_err(str::to_string)
            })
            .await
            .map_err(|_| "e911_sim_auth_runtime_failed".to_string())?
        })
    }
}

fn e911_request_context(
    binding: &crate::hardware::cellular::modem_manager::ModemBinding,
    catalog: &CarrierProfile,
    override_: &SimOverride,
    imsi: String,
) -> crate::services::e911::EntitlementRequestContext {
    let device_identity =
        crate::connectivity::modems::ims::effective_profile::resolve_effective_device_identity(
            Some(override_),
            Some(&binding.equipment_identifier),
        );
    crate::services::e911::EntitlementRequestContext {
        imsi,
        mcc: catalog.meta.mcc.to_string(),
        mnc: catalog.meta.mnc.to_string(),
        // Sending an IMEI is carrier evidence, not a global default. Reuse the
        // sealed profile's existing device-identity policy as the privacy gate.
        terminal_id: catalog
            .identity
            .device_identity_enabled
            .then_some(device_identity.imei)
            .flatten(),
        terminal_vendor: binding.manufacturer.clone(),
        terminal_model: binding.model.clone(),
        terminal_sw_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// GET /api/ims/lines/{line_id}/e911/capability
pub async fn get_e911_capability_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<E911CapabilityDto>>) {
    let (key, _binding, catalog, override_) = match resolve_e911_context(&app, &line_id).await {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<E911CapabilityDto>::error(reason)),
            );
        }
    };
    let provider = crate::services::e911::registry::provider_from_profile(catalog);
    let dto = E911CapabilityDto {
        profile_id: provider.profile_id.clone(),
        provider_kind: provider.kind.as_str().to_string(),
        provider_id: provider.profile_id.clone(),
        operator_requires: provider.kind.may_query() || override_.emergency.e911_address.is_some(),
        query_supported: provider.kind.may_query(),
        websheet_expected: provider.websheet_host_policy.is_some(),
    };
    let _ = key;
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", dto)),
    )
}

/// GET /api/ims/lines/{line_id}/e911/status
pub async fn get_e911_status_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<E911StatusDto>>) {
    let (key, _binding, catalog, override_) = match resolve_e911_context(&app, &line_id).await {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<E911StatusDto>::error(reason)),
            );
        }
    };
    let provider = crate::services::e911::registry::provider_from_profile(catalog);
    let view = match app.e911.status_with_provider(
        &provider,
        &key,
        override_.emergency.e911_address.is_some(),
    ) {
        Ok(view) => view,
        Err(_reason) => {
            // A store read failure must not leak internals; report a stable
            // state and keep emergency unverified.
            crate::services::e911::orchestrator::E911StatusView {
                profile_id: provider.profile_id.clone(),
                provider_kind: provider.kind,
                state: crate::connectivity::core::entitlement::E911State::Unknown,
                source: crate::connectivity::core::entitlement::E911StateSource::Unknown,
                operator_requires: provider.kind.may_query(),
                address_saved_locally: override_.emergency.e911_address.is_some(),
                operator_confirmed: false,
                emergency_unverified: true,
                needs_user_action: false,
                needs_reconfirm: false,
                retry_after_epoch: None,
            }
        }
    };
    let dto = E911StatusDto {
        profile_id: view.profile_id,
        provider_kind: view.provider_kind.as_str().to_string(),
        state: view.state.as_str().to_string(),
        source: view.source.as_str().to_string(),
        operator_requires: view.operator_requires,
        address_saved_locally: view.address_saved_locally,
        operator_confirmed: view.operator_confirmed,
        emergency_unverified: view.emergency_unverified,
        needs_user_action: view.needs_user_action,
        needs_reconfirm: view.needs_reconfirm,
        retry_after_epoch: view.retry_after_epoch,
    };
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", dto)),
    )
}

/// POST /api/ims/lines/{line_id}/e911/query
/// Read-only entitlement query. Never opens a websheet and never writes the
/// user override file; only the E911 state store is updated.
pub async fn post_e911_query_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<E911StatusDto>>) {
    let (key, binding, catalog, override_) = match resolve_e911_context(&app, &line_id).await {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<E911StatusDto>::error(reason)),
            );
        }
    };
    let provider = crate::services::e911::registry::provider_from_profile(catalog);
    if !provider.may_query() {
        return (
            StatusCode::OK,
            Json(ApiResponse::<E911StatusDto>::error(
                crate::services::e911::orchestrator::ERR_UNSUPPORTED,
            )),
        );
    }
    let sim_identity =
        match sim_identity_for_modem(app.dbus_conn.as_ref(), &binding.modem_path).await {
            Some(identity) if !identity.imsi.is_empty() => identity,
            _ => {
                return (
                    StatusCode::OK,
                    Json(ApiResponse::<E911StatusDto>::error(
                        crate::services::e911::orchestrator::ERR_NOT_READY,
                    )),
                );
            }
        };
    let context = e911_request_context(&binding, catalog, &override_, sim_identity.imsi);
    let sim_auth = LineE911AkaProvider {
        qmi_device: binding.qmi_device.clone().unwrap_or_default(),
        uim_slot: binding.uim_slot,
        proxy_socket: std::env::var("SIMADMIN_VOWIFI_QMI_PROXY_SOCKET")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "@qmi-proxy".to_string()),
    };
    let secrets = app.e911.store().load_secrets(&key).unwrap_or_default();
    match app
        .e911
        .query_with_provider(&provider, &key, &context, &secrets, &sim_auth)
        .await
    {
        Ok(_outcome) => {
            let view = app
                .e911
                .status_with_provider(&provider, &key, override_.emergency.e911_address.is_some())
                .unwrap();
            let dto = E911StatusDto {
                profile_id: view.profile_id,
                provider_kind: view.provider_kind.as_str().to_string(),
                state: view.state.as_str().to_string(),
                source: view.source.as_str().to_string(),
                operator_requires: view.operator_requires,
                address_saved_locally: view.address_saved_locally,
                operator_confirmed: view.operator_confirmed,
                emergency_unverified: view.emergency_unverified,
                needs_user_action: view.needs_user_action,
                needs_reconfirm: view.needs_reconfirm,
                retry_after_epoch: view.retry_after_epoch,
            };
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message("Success", dto)),
            )
        }
        Err(reason) => (
            StatusCode::OK,
            Json(ApiResponse::<E911StatusDto>::error(reason)),
        ),
    }
}

/// POST /api/ims/lines/{line_id}/e911/operations
/// Create a websheet operation from the current stored state. The URL must have
/// already been SSRF-checked by the transport; if the current state carries no
/// websheet directive we refuse rather than guessing.
pub async fn create_e911_operation_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<E911OperationDto>>) {
    let (key, _binding, catalog, override_) = match resolve_e911_context(&app, &line_id).await {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<E911OperationDto>::error(reason)),
            );
        }
    };
    let provider = crate::services::e911::registry::provider_from_profile(catalog);
    // Only create an operation when the carrier asked for a websheet (a
    // metadata-only provider or a provider that never gave us a flow URL has
    // nothing to open).
    let secrets = app.e911.store().load_secrets(&key).unwrap_or_default();
    let record = match app.e911.store().load(&key) {
        Ok(record) => record,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<E911OperationDto>::error(error.to_string())),
            );
        }
    };
    let needs_websheet = matches!(
        record.state,
        crate::connectivity::core::entitlement::E911State::NeedsTerms
            | crate::connectivity::core::entitlement::E911State::NeedsAddress
            | crate::connectivity::core::entitlement::E911State::NeedsUserAction
    ) && provider.kind.may_query();
    if !needs_websheet {
        return (
            StatusCode::OK,
            Json(ApiResponse::<E911OperationDto>::error(
                crate::services::e911::orchestrator::ERR_UNCONFIGURED,
            )),
        );
    }
    // We need a real server-flow URL. The transport stores it only in the
    // secret store; without it there is nothing safe to open.
    let server_flow_url = secrets.server_flow_url.unwrap_or_default();
    if server_flow_url.is_empty() {
        return (
            StatusCode::OK,
            Json(ApiResponse::<E911OperationDto>::error(
                crate::services::e911::orchestrator::ERR_UNCONFIGURED,
            )),
        );
    }
    let operation = app
        .e911
        .create_operation(&line_id, &key, &server_flow_url, 600)
        .await
        .unwrap();
    let _ = override_;
    let launch_url = operation.launch_path();
    let dto = E911OperationDto {
        operation_id: operation.operation_id,
        line_id: operation.line_id,
        launch_url,
        server_flow_url: operation.server_flow_url,
        expires_epoch: operation.expires_epoch,
        state: operation.state.as_str().to_string(),
    };
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message("Success", dto)),
    )
}

/// GET /api/ims/lines/{line_id}/e911/operations/{operation_id}
pub async fn get_e911_operation_handler(
    State(app): State<AppState>,
    Path((line_id, operation_id)): Path<(String, String)>,
) -> (StatusCode, Json<ApiResponse<E911OperationDto>>) {
    let (binding, _) = match resolve_ims_binding(&app, &line_id).await {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<E911OperationDto>::error(reason)),
            )
        }
    };
    match app
        .e911
        .get_operation_for_binding(&line_id, &operation_id, &binding)
        .await
    {
        Ok(operation) => {
            let launch_url = operation.launch_path();
            (
                StatusCode::OK,
                Json(ApiResponse::success_with_message(
                    "Success",
                    E911OperationDto {
                        operation_id: operation.operation_id,
                        line_id: operation.line_id,
                        launch_url,
                        server_flow_url: operation.server_flow_url,
                        expires_epoch: operation.expires_epoch,
                        state: operation.state.as_str().to_string(),
                    },
                )),
            )
        }
        Err(reason) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<E911OperationDto>::error(reason)),
        ),
    }
}

/// GET /api/ims/lines/{line_id}/e911/operations/{operation_id}/launch
///
/// This is intentionally a same-origin HTML response instead of another JSON
/// endpoint. TS.43 calls for `ServiceFlow_UserData` to be sent as the POST
/// body. A short-lived operation is the only capability needed to render it;
/// no token, cookie or callback state is returned to JavaScript/API callers.
///
/// Most carrier flows use URL-encoded user data. If a carrier supplies a
/// different body format, the page refuses to guess and gives the user a
/// direct link rather than silently sending a malformed provisioning request.
pub async fn launch_e911_operation_handler(
    State(app): State<AppState>,
    Path((line_id, operation_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let (binding, _) = match resolve_ims_binding(&app, &line_id).await {
        Ok(resolved) => resolved,
        Err(reason) => return (StatusCode::NOT_FOUND, Html(error_html(&reason))).into_response(),
    };
    let operation = match app
        .e911
        .get_operation_for_binding(&line_id, &operation_id, &binding)
        .await
    {
        Ok(operation) => operation,
        Err(reason) => return (StatusCode::NOT_FOUND, Html(error_html(&reason))).into_response(),
    };
    if operation.state != crate::services::e911::orchestrator::E911OperationState::Pending {
        return (
            StatusCode::GONE,
            Html(error_html("e911_operation_not_pending")),
        )
            .into_response();
    }

    let secrets = match app.e911.store().load_secrets(&operation.binding) {
        Ok(secrets) => secrets,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(error_html(error.code())),
            )
                .into_response()
        }
    };
    let target = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(operation.server_flow_url.as_bytes());
    let user_data = secrets.server_flow_user_data.unwrap_or_default();
    let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(user_data.as_bytes());
    let html = format!(
        "<!doctype html><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'unsafe-inline'; form-action https:\"><title>运营商 E911 流程</title><body><p id=\"status\">正在打开运营商 E911 流程…</p><noscript>请启用 JavaScript 后重试。</noscript><script>(()=>{{const decode=(v)=>{{const normalized=v.replace(/-/g,'+').replace(/_/g,'/');const padded=normalized+'='.repeat((4-normalized.length%4)%4);const s=atob(padded);const b=Uint8Array.from(s,c=>c.charCodeAt(0));return new TextDecoder().decode(b)}};const target=decode('{target}');const body=decode('{body}');const status=document.getElementById('status');if(!body){{location.replace(target);return}};if(!body.includes('=')){{status.textContent='运营商返回了非 URL-encoded 流程数据，请使用直接链接继续。';const a=document.createElement('a');a.href=target;a.textContent='打开运营商页面';a.rel='noreferrer';document.body.append(a);return}};const form=document.createElement('form');form.method='POST';form.action=target;form.enctype='application/x-www-form-urlencoded';for(const [name,value] of new URLSearchParams(body)){{const input=document.createElement('input');input.type='hidden';input.name=name;input.value=value;form.append(input)}};document.body.append(form);form.submit()}})()</script></body>",
    );
    (StatusCode::OK, Html(html)).into_response()
}

fn error_html(message: &str) -> String {
    let escaped = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    format!("<!doctype html><meta charset=\"utf-8\"><title>E911 流程不可用</title><p>{escaped}</p>")
}

/// POST /api/ims/lines/{line_id}/e911/operations/{operation_id}/callback
/// Called when the websheet completes in the browser. Requires the secret
/// callback state. This only marks the operation completed; the client must
/// then call POST .../query again, and only a confirming re-query moves the
/// line to `provisioned`.
pub async fn callback_e911_operation_handler(
    State(app): State<AppState>,
    Path((line_id, operation_id)): Path<(String, String)>,
    Json(payload): Json<serde_json::Value>,
) -> (StatusCode, Json<ApiResponse<()>>) {
    let (binding, _) = match resolve_ims_binding(&app, &line_id).await {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<()>::error(reason)),
            )
        }
    };
    let callback_state = payload
        .get("callback_state")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    match app
        .e911
        .complete_operation_for_binding(&line_id, &operation_id, &binding, &callback_state)
        .await
    {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", ())),
        ),
        Err(reason) => (StatusCode::OK, Json(ApiResponse::<()>::error(reason))),
    }
}

/// POST /api/ims/lines/{line_id}/e911/operations/{operation_id}/cancel
pub async fn cancel_e911_operation_handler(
    State(app): State<AppState>,
    Path((line_id, operation_id)): Path<(String, String)>,
) -> (StatusCode, Json<ApiResponse<()>>) {
    match app.e911.cancel_operation(&line_id, &operation_id).await {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", ())),
        ),
        Err(reason) => (StatusCode::OK, Json(ApiResponse::<()>::error(reason))),
    }
}

/// GET /api/ims/lines/{line_id}/e911/address
/// The full address is returned only on this authenticated edit endpoint, per
/// the research doc §12.4.
pub async fn get_e911_address_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let (key, _binding, _catalog, override_) = match resolve_e911_context(&app, &line_id).await {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            );
        }
    };
    let address = override_.emergency.e911_address;
    let _ = key;
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            json!({ "e911_address": address }),
        )),
    )
}

/// PUT /api/ims/lines/{line_id}/e911/address
/// Saves the user-entered civic address into the SIM override. Re-confirms the
/// binding before writing, exactly like the override PATCH path.
pub async fn put_e911_address_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let (_key, _binding, _catalog, mut override_) = match resolve_e911_context(&app, &line_id).await
    {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            );
        }
    };
    let raw = payload.get("e911_address").and_then(|value| value.as_str());
    override_.emergency.e911_address = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let problems =
        crate::connectivity::modems::ims::effective_profile::validate_override(&override_);
    if !problems.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ApiResponse::<serde_json::Value>::error(problems.join(","))),
        );
    }
    // Re-confirm the binding right before writing.
    let (key, _binding) = match resolve_ims_binding(&app, &line_id).await {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            );
        }
    };
    if let Err(error) = app.sim_overrides.save(&key, &override_) {
        return (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(error.to_string())),
        );
    }
    // Saving an address never confirms it: mark the entitlement stale so the
    // UI shows needs-reconfirm and a query is required.
    let mut record = match app.e911.store().load(&key) {
        Ok(record) => record,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<serde_json::Value>::error(error.to_string())),
            );
        }
    };
    record.invalidate();
    let _ = app.e911.store().save(&key, &record);
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            json!({ "e911_address": override_.emergency.e911_address }),
        )),
    )
}

/// DELETE /api/ims/lines/{line_id}/e911/address
pub async fn delete_e911_address_handler(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    let (_key, _binding, _catalog, mut override_) = match resolve_e911_context(&app, &line_id).await
    {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            );
        }
    };
    override_.emergency.e911_address = None;
    let (key, _binding) = match resolve_ims_binding(&app, &line_id).await {
        Ok(resolved) => resolved,
        Err(reason) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<serde_json::Value>::error(reason)),
            );
        }
    };
    if let Err(error) = app.sim_overrides.save(&key, &override_) {
        return (
            StatusCode::OK,
            Json(ApiResponse::<serde_json::Value>::error(error.to_string())),
        );
    }
    let mut record = match app.e911.store().load(&key) {
        Ok(record) => record,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<serde_json::Value>::error(error.to_string())),
            );
        }
    };
    record.invalidate();
    let _ = app.e911.store().save(&key, &record);
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            json!({ "e911_address": None::<String> }),
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::cellular::modem_manager::SimIdentity;

    #[test]
    fn call_monitor_requires_consecutive_missing_polls() {
        let mut record = crate::state::ActiveCallRecord {
            id: 1,
            line_id: "line-a".to_string(),
            direction: "incoming".to_string(),
            answered_at: None,
            answered: false,
            missing_polls: 0,
        };

        assert!(!call_poll_marks_finished(&mut record, false));
        assert_eq!(record.missing_polls, 1);
        assert!(!call_poll_marks_finished(&mut record, true));
        assert_eq!(record.missing_polls, 0);
        assert!(!call_poll_marks_finished(&mut record, false));
        assert!(call_poll_marks_finished(&mut record, false));
    }

    #[test]
    fn enabled_line_participates_in_vowifi_restore() {
        let mut offline = LineProfileConfig::for_line("line-0123456789abcdef0123456789abcdef");
        offline.vowifi.enabled = true;

        assert!(line_vowifi_restore_enabled(&offline));
    }

    #[test]
    fn disabled_line_does_not_participate_in_vowifi_restore() {
        let mut disabled = LineProfileConfig::for_line("line-0123456789abcdef0123456789abcdef");
        disabled.enabled = false;
        disabled.vowifi.enabled = true;

        assert!(!line_vowifi_restore_enabled(&disabled));
    }

    #[test]
    fn volte_voice_status_uses_only_the_requested_line() {
        let enabled = VolteVoiceStatusResponse::build(
            "line-0123456789abcdef0123456789abcdef".to_string(),
            true,
            true,
            true,
        );
        let disabled = VolteVoiceStatusResponse::build(
            "line-fedcba9876543210fedcba9876543210".to_string(),
            true,
            false,
            false,
        );

        assert!(enabled.enabled);
        assert!(enabled.registered);
        assert!(!disabled.enabled);
        assert!(disabled.ims_connection_enabled);
        assert!(!disabled.voice_enabled);
        assert!(!disabled.registered);
        assert_ne!(enabled.line_id, disabled.line_id);
    }

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
    fn vowifi_boot_restore_keeps_the_selected_line_scope() {
        let workflow = VowifiRestoreWorkflow::boot_auto_restore(
            &AutoRestoreConfig::default(),
            "line-b".to_string(),
        );
        assert_eq!(workflow.line_id, "line-b");
        assert_eq!(workflow.label(), "boot_auto_restore");
    }

    #[test]
    fn vowifi_diagnostics_use_only_the_path_line() {
        let lines = vec!["line-a".to_string(), "line-b".to_string()];

        assert_eq!(
            select_vowifi_diagnostic_line_id("", &lines),
            Err("vowifi_line_id_required".to_string())
        );
        assert_eq!(
            select_vowifi_diagnostic_line_id(" line-b ", &lines),
            Ok("line-b".to_string())
        );
        assert_eq!(
            select_vowifi_diagnostic_line_id("line-c", &lines),
            Err("vowifi_line_not_found".to_string())
        );
        assert_eq!(
            select_vowifi_diagnostic_line_id("", &["line-a".to_string()]),
            Err("vowifi_line_id_required".to_string())
        );
        assert_eq!(
            select_vowifi_diagnostic_line_id("line-a", &[]),
            Err("vowifi_line_not_found".to_string())
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
        let first = crate::connectivity::modems::ims::vowifi::sms::MoSmsSipOutcome {
            trace_id: "trace-a".to_string(),
            message_id: "mo-a".to_string(),
            sip_status: 202,
            rpdu_ack: crate::connectivity::modems::ims::vowifi::sms::RpduAckState::None,
            delivery_state:
                crate::connectivity::modems::ims::vowifi::sms::SmsDeliveryState::Accepted,
            failure_cause: None,
            mt_deliveries: Vec::new(),
        };
        let mut second = first.clone();
        second.trace_id = "trace-b".to_string();
        second.message_id = "mo-b".to_string();

        let first_key = vowifi_mt_storage_key(
            "line-a",
            &first,
            "10086",
            "You don't have any credit balance",
        );
        let second_key = vowifi_mt_storage_key(
            "line-a",
            &second,
            "10086",
            "You don't have any credit balance",
        );
        let other_line_key = vowifi_mt_storage_key(
            "line-b",
            &first,
            "10086",
            "You don't have any credit balance",
        );

        assert_ne!(first_key, second_key);
        assert_ne!(first_key, other_line_key);
    }

    #[test]
    fn vowifi_mt_delivery_persists_selected_line_identity() {
        let db =
            Database::new(std::path::PathBuf::from(":memory:")).expect("create in-memory database");
        let line_id = "line-0123456789abcdef0123456789abcdef";
        let outcome = crate::connectivity::modems::ims::vowifi::sms::MoSmsSipOutcome {
            trace_id: "trace-line".to_string(),
            message_id: "message-line".to_string(),
            sip_status: 202,
            rpdu_ack: crate::connectivity::modems::ims::vowifi::sms::RpduAckState::Acked,
            delivery_state:
                crate::connectivity::modems::ims::vowifi::sms::SmsDeliveryState::Delivered,
            failure_cause: None,
            mt_deliveries: vec![
                crate::connectivity::modems::ims::vowifi::sms::MtSmsDeliver {
                    rp_message_reference: 1,
                    originator: "10086".to_string(),
                    text: "line reply".to_string(),
                    user_data_bytes: 10,
                    service_center_timestamp: "2026-08-04 21:30:00".to_string(),
                    segment_reference: None,
                    segment_sequence: 1,
                    segment_total: 1,
                },
            ],
        };

        let inserted = persist_vowifi_mt_deliveries(&db, line_id, &outcome, true);

        assert_eq!(inserted.len(), 1);
        assert_eq!(inserted[0].line_id.as_deref(), Some(line_id));
        let stored = db
            .get_sms_messages_for_channel(10, 0, None, Some(line_id))
            .expect("read line SMS messages");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].line_id.as_deref(), Some(line_id));
        assert!(db
            .get_sms_messages_for_channel(10, 0, None, Some("line-other"))
            .expect("read other line SMS messages")
            .is_empty());
    }

    #[test]
    fn vowifi_mt_cross_transport_dedup_is_isolated_by_line() {
        let db =
            Database::new(std::path::PathBuf::from(":memory:")).expect("create in-memory database");
        let line_a = "line-0123456789abcdef0123456789abcdef";
        let line_b = "line-fedcba9876543210fedcba9876543210";
        let outcome = crate::connectivity::modems::ims::vowifi::sms::MoSmsSipOutcome {
            trace_id: "trace-dedup".to_string(),
            message_id: "message-dedup".to_string(),
            sip_status: 202,
            rpdu_ack: crate::connectivity::modems::ims::vowifi::sms::RpduAckState::Acked,
            delivery_state:
                crate::connectivity::modems::ims::vowifi::sms::SmsDeliveryState::Delivered,
            failure_cause: None,
            mt_deliveries: vec![
                crate::connectivity::modems::ims::vowifi::sms::MtSmsDeliver {
                    rp_message_reference: 1,
                    originator: "10086".to_string(),
                    text: "same delivery".to_string(),
                    user_data_bytes: 13,
                    service_center_timestamp: "2026-08-04 21:30:00".to_string(),
                    segment_reference: None,
                    segment_sequence: 1,
                    segment_total: 1,
                },
            ],
        };
        let fingerprint = crate::services::orchestrator::message_fingerprint(
            &crate::services::orchestrator::MessageFingerprintInput {
                service_center_timestamp: "2026-08-04 21:30:00",
                originator: "10086",
                text: "same delivery",
                segment_reference: None,
                segment_sequence: 1,
                segment_total: 1,
            },
        );
        assert!(db
            .claim_sms_dedup(line_a, &fingerprint, "modem")
            .expect("seed line-a modem claim"));

        assert!(persist_vowifi_mt_deliveries(&db, line_a, &outcome, true).is_empty());
        assert_eq!(
            db.get_sms_messages_for_channel(10, 0, None, Some(line_a))
                .unwrap()
                .len(),
            0
        );

        let line_b_messages = persist_vowifi_mt_deliveries(&db, line_b, &outcome, true);
        assert_eq!(line_b_messages.len(), 1);
        assert_eq!(line_b_messages[0].line_id.as_deref(), Some(line_b));
    }

    #[test]
    fn vowifi_mt_complete_group_count_collapses_segments() {
        let outcome = crate::connectivity::modems::ims::vowifi::sms::MoSmsSipOutcome {
            trace_id: "trace-a".to_string(),
            message_id: "mo-a".to_string(),
            sip_status: 202,
            rpdu_ack: crate::connectivity::modems::ims::vowifi::sms::RpduAckState::None,
            delivery_state:
                crate::connectivity::modems::ims::vowifi::sms::SmsDeliveryState::Accepted,
            failure_cause: None,
            mt_deliveries: vec![
                crate::connectivity::modems::ims::vowifi::sms::MtSmsDeliver {
                    rp_message_reference: 1,
                    originator: "10086".to_string(),
                    text: "part1".to_string(),
                    user_data_bytes: 5,
                    service_center_timestamp: "2026-06-22 13:13:59".to_string(),
                    segment_reference: Some(7),
                    segment_sequence: 1,
                    segment_total: 2,
                },
                crate::connectivity::modems::ims::vowifi::sms::MtSmsDeliver {
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

    #[test]
    fn effective_services_reflects_override_entries() {
        let mut override_ = SimOverride::default();
        override_.services.call_waiting = Some(true);
        let services = EffectiveServices::from_override(Some(&override_));
        assert_eq!(services.call_waiting, Some(true));
        assert_eq!(
            services.call_waiting_source,
            Some(OverrideSource::SimOverride)
        );
        assert_eq!(services.caller_id_restriction, None);

        let none = EffectiveServices::from_override(None);
        assert_eq!(none.call_waiting, None);
        assert_eq!(none.call_waiting_source, None);
    }

    #[test]
    fn effective_services_ignores_unset_booleans() {
        let override_ = SimOverride::default();
        let services = EffectiveServices::from_override(Some(&override_));
        assert_eq!(services.call_waiting, None);
        assert_eq!(services.caller_id_restriction, None);
    }

    #[test]
    fn binding_dto_masks_iccid_to_last4() {
        let key = SimBindingKey::Plain {
            iccid: "89012345678901234567".to_string(),
        };
        let dto = binding_dto(&key);
        assert_eq!(dto.kind, "plain");
        assert_eq!(dto.iccid_last4.as_deref(), Some("4567"));
    }

    #[test]
    fn binding_dto_masks_euicc_binding() {
        let key = SimBindingKey::Euicc {
            eid: "89049032023442222222555555555555".to_string(),
            profile_iccid: "89012345678901234567".to_string(),
        };
        let dto = binding_dto(&key);
        assert_eq!(dto.kind, "euicc");
        assert_eq!(dto.iccid_last4.as_deref(), Some("4567"));
    }

    #[test]
    fn binding_dto_handles_short_iccid() {
        let key = SimBindingKey::Plain {
            iccid: "123".to_string(),
        };
        let dto = binding_dto(&key);
        assert_eq!(dto.iccid_last4.as_deref(), Some("123"));
    }

    #[test]
    fn source_str_maps_all_origins() {
        assert_eq!(source_str(OverrideSource::Catalog), "catalog");
        assert_eq!(source_str(OverrideSource::SimOverride), "sim_override");
        assert_eq!(source_str(OverrideSource::Modem), "modem");
        assert_eq!(source_str(OverrideSource::Network), "network");
    }

    #[test]
    fn device_identity_source_str_uses_custom_modem_unavailable_vocabulary() {
        assert_eq!(
            device_identity_source_str(OverrideSource::SimOverride),
            "custom"
        );
        assert_eq!(device_identity_source_str(OverrideSource::Modem), "modem");
        assert_eq!(
            device_identity_source_str(OverrideSource::Catalog),
            "unavailable"
        );
        assert_eq!(
            device_identity_source_str(OverrideSource::Network),
            "unavailable"
        );
    }

    #[test]
    fn normalize_payload_drops_whitespace_only_custom_imei() {
        let mut payload = SimOverride::default();
        payload.ims_common.custom_imei = Some("   ".to_string());
        let normalized = normalize_ims_override_payload(payload);
        assert_eq!(normalized.ims_common.custom_imei, None);
    }

    #[test]
    fn normalize_payload_trims_custom_imei_and_keeps_value() {
        let mut payload = SimOverride::default();
        payload.ims_common.custom_imei = Some("  490154203237518  ".to_string());
        let normalized = normalize_ims_override_payload(payload);
        assert_eq!(
            normalized.ims_common.custom_imei.as_deref(),
            Some("490154203237518")
        );
    }

    #[test]
    fn parses_only_the_csvm_quoted_voicemail_number() {
        assert_eq!(
            parse_csvm_voicemail_number("response: '+CSVM: 1,\"*86\",129'\n").as_deref(),
            Some("*86")
        );
        assert_eq!(
            parse_csvm_voicemail_number("response: '+CSVM: 1,\"+60123456789\",145'\n").as_deref(),
            Some("+60123456789")
        );
        assert!(parse_csvm_voicemail_number("response: '+CSVM: 1,\"1; reboot\",129'").is_none());
    }

    #[test]
    fn normalize_payload_drops_whitespace_only_voicemail() {
        let mut payload = SimOverride::default();
        payload.ims_common.voicemail_number = Some("  ".to_string());
        let normalized = normalize_ims_override_payload(payload);
        assert_eq!(normalized.ims_common.voicemail_number, None);
    }

    #[test]
    fn normalize_payload_preserves_empty_override() {
        let payload = SimOverride::default();
        let normalized = normalize_ims_override_payload(payload);
        assert_eq!(normalized, SimOverride::default());
    }

    #[test]
    fn two_lines_resolve_distinct_custom_imei_without_crossing() {
        let first = SimOverride {
            ims_common: crate::connectivity::modems::ims::profile_override::ImsCommonOverride {
                custom_imei: Some("490154203237518".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let second = SimOverride {
            ims_common: crate::connectivity::modems::ims::profile_override::ImsCommonOverride {
                custom_imei: Some("351234567890124".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let first_identity =
            crate::connectivity::modems::ims::effective_profile::resolve_effective_device_identity(
                Some(&first),
                Some("999999999999999"),
            );
        let second_identity =
            crate::connectivity::modems::ims::effective_profile::resolve_effective_device_identity(
                Some(&second),
                Some("999999999999999"),
            );
        assert_eq!(first_identity.imei.as_deref(), Some("490154203237518"));
        assert_eq!(second_identity.imei.as_deref(), Some("351234567890124"));
        assert_ne!(first_identity.imei, second_identity.imei);
    }
}
