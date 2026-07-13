//! Single-admin authentication for the SimAdmin web console.

use std::io::{self, Write};
use std::num::NonZeroU32;

use anyhow::{bail, Result};
use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ring::{
    digest, pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    config::SecurityConfig,
    db::Database,
    models::ApiResponse,
    state::AppState,
    system_event::{
        codes as system_event_codes, severity as system_event_severity,
        status as system_event_status,
    },
};

const PASSWORD_KEY: &str = "admin_password_hash";
const PASSWORD_ALGORITHM: &str = "pbkdf2_sha256";
const PBKDF2_ITERATIONS: u32 = 210_000;
const PASSWORD_SALT_LEN: usize = 16;
const PASSWORD_HASH_LEN: usize = 32;
const PASSWORD_MAX_LENGTH: usize = 64;
const PASSWORD_MIN_LENGTH_MIN: u8 = 1;
const PASSWORD_MIN_LENGTH_MAX: u8 = PASSWORD_MAX_LENGTH as u8;
const SESSION_TOKEN_LEN: usize = 32;
const SESSION_TTL_NEVER_SECONDS: i64 = 100 * 365 * 24 * 60 * 60;
const SESSION_COOKIE: &str = "simadmin_session";
const SESSION_TTL_OPTIONS: [i64; 5] = [
    24 * 60 * 60,
    7 * 24 * 60 * 60,
    14 * 24 * 60 * 60,
    30 * 24 * 60 * 60,
    -1,
];
const IDLE_TIMEOUT_OPTIONS: [i64; 6] = [30 * 60, 60 * 60, 2 * 60 * 60, 3 * 60 * 60, 6 * 60 * 60, 0];

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub new_password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthStatusResponse {
    pub configured: bool,
    pub authenticated: bool,
    pub settings: SecurityConfig,
}

#[derive(Debug, Serialize)]
pub struct AuthSettingsResponse {
    pub configured: bool,
    pub settings: SecurityConfig,
}

#[derive(Debug)]
struct SessionToken {
    token: String,
    hash: String,
}

fn normalize_security_settings(mut settings: SecurityConfig) -> SecurityConfig {
    if !(PASSWORD_MIN_LENGTH_MIN..=PASSWORD_MIN_LENGTH_MAX).contains(&settings.password_min_length)
    {
        settings.password_min_length = SecurityConfig::default().password_min_length;
    }
    if !SESSION_TTL_OPTIONS.contains(&settings.session_ttl_seconds) {
        settings.session_ttl_seconds = SecurityConfig::default().session_ttl_seconds;
    }
    if !IDLE_TIMEOUT_OPTIONS.contains(&settings.idle_timeout_seconds) {
        settings.idle_timeout_seconds = SecurityConfig::default().idle_timeout_seconds;
    }
    if !settings.password_require_letters
        && !settings.password_require_digits
        && !settings.password_require_symbols
    {
        settings.password_require_letters = true;
    }
    settings
}

fn validate_security_settings(settings: &SecurityConfig) -> Result<()> {
    if !(PASSWORD_MIN_LENGTH_MIN..=PASSWORD_MIN_LENGTH_MAX).contains(&settings.password_min_length)
    {
        bail!("密码最小长度需为 1-64 之间的整数");
    }
    if !settings.password_require_letters
        && !settings.password_require_digits
        && !settings.password_require_symbols
    {
        bail!("字符类型要求至少需要选择一项");
    }
    if !SESSION_TTL_OPTIONS.contains(&settings.session_ttl_seconds) {
        bail!("会话有效期只能选择 1 天、7 天、14 天、30 天或永不过期");
    }
    if !IDLE_TIMEOUT_OPTIONS.contains(&settings.idle_timeout_seconds) {
        bail!("空闲超时只能选择 30 分钟、1 小时、2 小时、3 小时、6 小时或关闭");
    }
    Ok(())
}

fn configured_session_ttl_seconds(settings: &SecurityConfig) -> i64 {
    if settings.session_ttl_seconds < 0 {
        SESSION_TTL_NEVER_SECONDS
    } else {
        settings.session_ttl_seconds
    }
}

fn enabled_password_types_text(settings: &SecurityConfig) -> &'static str {
    match (
        settings.password_require_letters,
        settings.password_require_digits,
        settings.password_require_symbols,
    ) {
        (true, true, true) => "英文字母、数字和符号",
        (true, true, false) => "英文字母和数字",
        (true, false, true) => "英文字母和符号",
        (false, true, true) => "数字和符号",
        (true, false, false) => "英文字母",
        (false, true, false) => "数字",
        (false, false, true) => "符号",
        (false, false, false) => "英文字母、数字和符号",
    }
}

fn password_byte_allowed(byte: u8, settings: &SecurityConfig) -> bool {
    byte.is_ascii_graphic()
        && ((settings.password_require_letters && byte.is_ascii_alphabetic())
            || (settings.password_require_digits && byte.is_ascii_digit())
            || (settings.password_require_symbols
                && byte.is_ascii_graphic()
                && !byte.is_ascii_alphanumeric()))
}

pub fn validate_admin_password(password: &str, settings: &SecurityConfig) -> Result<()> {
    let settings = normalize_security_settings(settings.clone());
    if !password
        .bytes()
        .all(|byte| password_byte_allowed(byte, &settings))
    {
        bail!(
            "密码只能包含{}，不能包含空格、中文或未启用的字符类型",
            enabled_password_types_text(&settings)
        );
    }
    if !((settings.password_min_length as usize)..=PASSWORD_MAX_LENGTH).contains(&password.len()) {
        bail!(
            "密码长度需为 {}-{} 个字符",
            settings.password_min_length,
            PASSWORD_MAX_LENGTH
        );
    }

    if settings.password_require_letters && !password.bytes().any(|byte| byte.is_ascii_alphabetic())
    {
        bail!("密码需包含英文字母");
    }
    if settings.password_require_digits && !password.bytes().any(|byte| byte.is_ascii_digit()) {
        bail!("密码需包含数字");
    }
    if settings.password_require_symbols
        && !password
            .bytes()
            .any(|byte| byte.is_ascii_graphic() && !byte.is_ascii_alphanumeric())
    {
        bail!("密码需包含符号");
    }
    Ok(())
}

pub fn hash_password(password: &str, settings: &SecurityConfig) -> Result<String> {
    validate_admin_password(password, settings)?;

    let rng = SystemRandom::new();
    let mut salt = [0u8; PASSWORD_SALT_LEN];
    rng.fill(&mut salt)
        .map_err(|_| anyhow::anyhow!("Failed to generate password salt"))?;

    let mut output = [0u8; PASSWORD_HASH_LEN];
    let iterations = NonZeroU32::new(PBKDF2_ITERATIONS).expect("non-zero iterations");
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        iterations,
        &salt,
        password.as_bytes(),
        &mut output,
    );

    Ok(format!(
        "{}${}${}${}",
        PASSWORD_ALGORITHM,
        PBKDF2_ITERATIONS,
        URL_SAFE_NO_PAD.encode(salt),
        URL_SAFE_NO_PAD.encode(output)
    ))
}

fn verify_password(password: &str, encoded_hash: &str) -> Result<bool> {
    let parts: Vec<&str> = encoded_hash.split('$').collect();
    if parts.len() != 4 || parts[0] != PASSWORD_ALGORITHM {
        bail!("Unsupported password hash format");
    }

    let iterations = parts[1].parse::<u32>()?;
    let iterations = NonZeroU32::new(iterations).ok_or_else(|| anyhow::anyhow!("Invalid hash"))?;
    let salt = URL_SAFE_NO_PAD.decode(parts[2])?;
    let expected = URL_SAFE_NO_PAD.decode(parts[3])?;

    Ok(pbkdf2::verify(
        pbkdf2::PBKDF2_HMAC_SHA256,
        iterations,
        &salt,
        password.as_bytes(),
        &expected,
    )
    .is_ok())
}

fn generate_session_token() -> Result<SessionToken> {
    let rng = SystemRandom::new();
    let mut raw = [0u8; SESSION_TOKEN_LEN];
    rng.fill(&mut raw)
        .map_err(|_| anyhow::anyhow!("Failed to generate session token"))?;
    let token = URL_SAFE_NO_PAD.encode(raw);
    let hash = hash_session_token(&token);
    Ok(SessionToken { token, hash })
}

fn hash_session_token(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(digest::digest(&digest::SHA256, token.as_bytes()).as_ref())
}

fn session_cookie(token: &str, settings: &SecurityConfig) -> String {
    let max_age = configured_session_ttl_seconds(settings);
    format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}")
}

fn expired_session_cookie() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == SESSION_COOKIE).then(|| value.to_string())
    })
}

fn wants_login_redirect(headers: &HeaderMap) -> bool {
    let accepts_html = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("text/html"))
        .unwrap_or(false);
    let is_navigation = headers
        .get("sec-fetch-mode")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.eq_ignore_ascii_case("navigate"))
        .unwrap_or(false);
    accepts_html || is_navigation
}

fn unauthorized_response(headers: &HeaderMap, message: impl Into<String>) -> Response {
    if wants_login_redirect(headers) {
        return (StatusCode::SEE_OTHER, [(header::LOCATION, "/login")]).into_response();
    }

    (
        StatusCode::UNAUTHORIZED,
        Json(ApiResponse::<Value>::error(message.into())),
    )
        .into_response()
}

fn response_with_session<T: Serialize>(
    payload: ApiResponse<T>,
    token: &str,
    settings: &SecurityConfig,
) -> Response {
    let mut response = Json(payload).into_response();
    let cookie = session_cookie(token, settings);
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

fn is_authenticated(database: &Database, headers: &HeaderMap) -> bool {
    let Some(token) = cookie_token(headers) else {
        return false;
    };
    database
        .auth_session_valid(&hash_session_token(&token))
        .unwrap_or(false)
}

pub async fn auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if request.method() == Method::OPTIONS {
        return next.run(request).await;
    }

    let settings = normalize_security_settings(state.config_manager.get_security());
    if !settings.password_protection_enabled {
        return next.run(request).await;
    }

    if !state.database.auth_is_configured().unwrap_or(false) {
        return unauthorized_response(&headers, "管理员密码尚未设置");
    }

    if !is_authenticated(&state.database, &headers) {
        return unauthorized_response(&headers, "请先登录");
    }

    next.run(request).await
}

pub async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> (StatusCode, Json<ApiResponse<AuthStatusResponse>>) {
    let settings = normalize_security_settings(state.config_manager.get_security());
    let configured = state.database.auth_is_configured().unwrap_or(false);
    let authenticated = !settings.password_protection_enabled
        || (configured && is_authenticated(&state.database, &headers));
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            AuthStatusResponse {
                configured,
                authenticated,
                settings,
            },
        )),
    )
}

pub async fn setup(State(state): State<AppState>, Json(payload): Json<LoginRequest>) -> Response {
    let settings = normalize_security_settings(state.config_manager.get_security());
    if state.database.auth_is_configured().unwrap_or(false) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<Value>::error("管理员密码已设置")),
        )
            .into_response();
    }

    let password_hash = match hash_password(&payload.password, &settings) {
        Ok(hash) => hash,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<Value>::error(err.to_string())),
            )
                .into_response()
        }
    };

    if let Err(err) = state
        .database
        .set_auth_config_value(PASSWORD_KEY, &password_hash)
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Value>::error(format!(
                "保存管理员密码失败: {err}"
            ))),
        )
            .into_response();
    }

    let session = match generate_session_token() {
        Ok(session) => session,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<Value>::error(err.to_string())),
            )
                .into_response()
        }
    };

    if let Err(err) = state
        .database
        .insert_auth_session(&session.hash, configured_session_ttl_seconds(&settings))
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Value>::error(format!("创建会话失败: {err}"))),
        )
            .into_response();
    }

    response_with_session(
        ApiResponse::success_with_message("Admin password configured", Value::Null),
        &session.token,
        &settings,
    )
}

pub async fn login(State(state): State<AppState>, Json(payload): Json<LoginRequest>) -> Response {
    let settings = normalize_security_settings(state.config_manager.get_security());
    let Some(password_hash) = state
        .database
        .get_auth_config_value(PASSWORD_KEY)
        .unwrap_or(None)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<Value>::error("管理员密码尚未设置")),
        )
            .into_response();
    };

    match verify_password(&payload.password, &password_hash) {
        Ok(true) => {}
        Ok(false) => {
            state.system_event_emitter.record_login_failure().await;
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<Value>::error("管理员密码不正确")),
            )
                .into_response();
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<Value>::error(format!("验证密码失败: {err}"))),
            )
                .into_response()
        }
    }

    let session = match generate_session_token() {
        Ok(session) => session,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<Value>::error(err.to_string())),
            )
                .into_response()
        }
    };

    if let Err(err) = state
        .database
        .insert_auth_session(&session.hash, configured_session_ttl_seconds(&settings))
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Value>::error(format!("创建会话失败: {err}"))),
        )
            .into_response();
    }

    response_with_session(
        ApiResponse::success_with_message("Logged in", Value::Null),
        &session.token,
        &settings,
    )
}

pub async fn change_password(
    State(state): State<AppState>,
    Json(payload): Json<ChangePasswordRequest>,
) -> Response {
    let settings = normalize_security_settings(state.config_manager.get_security());
    let new_hash = match hash_password(&payload.new_password, &settings) {
        Ok(hash) => hash,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<Value>::error(err.to_string())),
            )
                .into_response()
        }
    };

    if let Err(err) = state.database.replace_admin_password_hash(&new_hash) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Value>::error(format!("更新密码失败: {err}"))),
        )
            .into_response();
    }

    let session = match generate_session_token() {
        Ok(session) => session,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<Value>::error(err.to_string())),
            )
                .into_response()
        }
    };

    if let Err(err) = state
        .database
        .insert_auth_session(&session.hash, configured_session_ttl_seconds(&settings))
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Value>::error(format!("创建会话失败: {err}"))),
        )
            .into_response();
    }

    state
        .system_event_emitter
        .emit_code(
            system_event_codes::SECURITY_PASSWORD_CHANGED,
            system_event_severity::WARNING,
            system_event_status::CHANGED,
            "admin",
            "管理员密码已修改",
        )
        .await;

    response_with_session(
        ApiResponse::success_with_message("Password updated", Value::Null),
        &session.token,
        &settings,
    )
}

pub async fn get_settings(
    State(state): State<AppState>,
) -> (StatusCode, Json<ApiResponse<AuthSettingsResponse>>) {
    let settings = normalize_security_settings(state.config_manager.get_security());
    let configured = state.database.auth_is_configured().unwrap_or(false);
    (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Success",
            AuthSettingsResponse {
                configured,
                settings,
            },
        )),
    )
}

pub async fn set_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<SecurityConfig>,
) -> Response {
    let previous_settings = normalize_security_settings(state.config_manager.get_security());
    if let Err(err) = validate_security_settings(&payload) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<Value>::error(err.to_string())),
        )
            .into_response();
    }
    let settings = normalize_security_settings(payload);
    if let Err(err) = state.config_manager.set_security(settings.clone()) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Value>::error(format!(
                "保存安全设置失败: {err}"
            ))),
        )
            .into_response();
    }

    if previous_settings.password_protection_enabled && !settings.password_protection_enabled {
        state
            .system_event_emitter
            .emit_code(
                system_event_codes::SECURITY_PASSWORD_PROTECTION_DISABLED,
                system_event_severity::CRITICAL,
                system_event_status::CHANGED,
                "security",
                "密码保护已关闭",
            )
            .await;
    } else if previous_settings != settings {
        state
            .system_event_emitter
            .emit_code(
                system_event_codes::SECURITY_POLICY_CHANGED,
                system_event_severity::WARNING,
                system_event_status::CHANGED,
                "security",
                "安全策略已变更",
            )
            .await;
    }

    let mut response = (
        StatusCode::OK,
        Json(ApiResponse::success_with_message(
            "Security settings saved",
            settings.clone(),
        )),
    )
        .into_response();

    if let Some(token) = cookie_token(&headers) {
        let _ = state.database.refresh_auth_session(
            &hash_session_token(&token),
            configured_session_ttl_seconds(&settings),
        );
        if let Ok(value) = HeaderValue::from_str(&session_cookie(&token, &settings)) {
            response.headers_mut().insert(header::SET_COOKIE, value);
        }
    }

    response
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = cookie_token(&headers) {
        let _ = state
            .database
            .delete_auth_session(&hash_session_token(&token));
    }

    let mut response =
        Json(ApiResponse::success_with_message("Logged out", Value::Null)).into_response();
    if let Ok(value) = HeaderValue::from_str(&expired_session_cookie()) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

pub fn reset_admin_password_interactive(
    database: &Database,
    settings: &SecurityConfig,
) -> Result<()> {
    let password = read_password_line("New admin password: ")?;
    let confirm = read_password_line("Confirm admin password: ")?;
    if password != confirm {
        bail!("Passwords do not match");
    }
    let hash = hash_password(&password, settings)?;
    database.replace_admin_password_hash(&hash)?;
    println!("Admin password updated and all web sessions were cleared.");
    Ok(())
}

pub fn clear_admin_auth(database: &Database) -> Result<()> {
    database.clear_admin_auth()?;
    println!("Admin password and all web sessions were cleared.");
    println!("Open the web UI to set a new admin password.");
    Ok(())
}

#[cfg(unix)]
fn read_password_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let _ = std::process::Command::new("stty").arg("-echo").status();
    let mut value = String::new();
    let result = io::stdin().read_line(&mut value);
    let _ = std::process::Command::new("stty").arg("echo").status();
    println!();
    result?;
    Ok(value.trim_end_matches(['\r', '\n']).to_string())
}

#[cfg(not(unix))]
fn read_password_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(value.trim_end_matches(['\r', '\n']).to_string())
}
