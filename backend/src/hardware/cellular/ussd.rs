//! USSD/USSI support for modem-backed lines.
//!
//! USSI is the user-facing name for supplementary service dialogues; Quectel
//! EC20/EC25/EG25-class modules expose the transaction through the 3GPP
//! `AT+CUSD` command. The modem helper deliberately waits for the asynchronous
//! `+CUSD:` URC instead of treating the preceding `OK` as the complete result.

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, OnceLock,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;
use zbus::Connection;

use crate::api::models::UssdResponse;
use crate::hardware::cellular::modem_manager;

const SESSION_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone)]
struct UssdSession {
    line_id: String,
    modem_path: String,
    last_activity_at: Instant,
    /// The same lock is held by the active-modem entry. Keeping it in the
    /// session means continue/cancel requests for one dialog cannot interleave
    /// AT+CUSD transactions, while other modems remain independent.
    operation: Arc<Mutex<()>>,
}

#[derive(Clone)]
struct ActiveModem {
    /// `None` means a start request is in flight. `Some` means that the modem
    /// remains reserved by the interactive USSD session with this id.
    session_id: Option<String>,
    operation: Arc<Mutex<()>>,
}

static SESSIONS: OnceLock<Mutex<HashMap<String, UssdSession>>> = OnceLock::new();
static ACTIVE_MODEMS: OnceLock<Mutex<HashMap<String, ActiveModem>>> = OnceLock::new();
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

fn sessions() -> &'static Mutex<HashMap<String, UssdSession>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn active_modems() -> &'static Mutex<HashMap<String, ActiveModem>> {
    ACTIVE_MODEMS.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn reserve_modem(modem_path: &str) -> Result<Arc<Mutex<()>>, String> {
    let mut active = active_modems().lock().await;
    if active.contains_key(modem_path) {
        return Err("该 modem 当前正在处理其他 USSD/USSI 请求，请稍后重试".to_string());
    }
    let operation = Arc::new(Mutex::new(()));
    active.insert(
        modem_path.to_string(),
        ActiveModem {
            session_id: None,
            operation: operation.clone(),
        },
    );
    Ok(operation)
}

async fn release_start(modem_path: &str) {
    let mut active = active_modems().lock().await;
    if active
        .get(modem_path)
        .is_some_and(|entry| entry.session_id.is_none())
    {
        active.remove(modem_path);
    }
}

async fn bind_session(modem_path: &str, session_id: &str, operation: &Arc<Mutex<()>>) -> bool {
    let mut active = active_modems().lock().await;
    let Some(entry) = active.get_mut(modem_path) else {
        return false;
    };
    if entry.session_id.is_some() || !Arc::ptr_eq(&entry.operation, operation) {
        return false;
    }
    entry.session_id = Some(session_id.to_string());
    true
}

async fn release_session(modem_path: &str, session_id: &str) {
    let mut active = active_modems().lock().await;
    if active
        .get(modem_path)
        .is_some_and(|entry| entry.session_id.as_deref() == Some(session_id))
    {
        active.remove(modem_path);
    }
}

async fn reap_expired_sessions(conn: &Connection) {
    // Remove expired sessions from the public map first, but keep the modem
    // reservation until AT+CUSD=2 has been attempted. This prevents a new
    // request from racing with the cleanup command and mirrors VoCat's
    // per-port serialization rule.
    let expired = {
        let mut map = sessions().lock().await;
        let expired = map
            .iter()
            .filter(|(_, session)| session.last_activity_at.elapsed() >= SESSION_TTL)
            .map(|(id, session)| {
                (
                    id.clone(),
                    session.modem_path.clone(),
                    session.operation.clone(),
                )
            })
            .collect::<Vec<_>>();
        for (id, _, _) in &expired {
            map.remove(id);
        }
        expired
    };

    for (session_id, modem_path, operation) in expired {
        let still_reserved = {
            let active = active_modems().lock().await;
            active.get(&modem_path).is_some_and(|entry| {
                entry.session_id.as_deref() == Some(session_id.as_str())
                    && Arc::ptr_eq(&entry.operation, &operation)
            })
        };
        if !still_reserved {
            continue;
        }

        // Do not hold either global map lock while waiting for the per-modem
        // operation or while doing I/O. A continue/cancel already in flight
        // gets to finish first; only then do we terminate the stale dialog.
        let _operation = operation.lock().await;
        let still_reserved = {
            let active = active_modems().lock().await;
            active.get(&modem_path).is_some_and(|entry| {
                entry.session_id.as_deref() == Some(session_id.as_str())
                    && Arc::ptr_eq(&entry.operation, &operation)
            })
        };
        if still_reserved {
            if let Err(error) =
                modem_manager::cancel_ussd_at_command_for_modem(conn, &modem_path).await
            {
                tracing::warn!(
                    modem_path = %modem_path,
                    session_id = %session_id,
                    error = %error,
                    "Failed to cancel expired USSD session"
                );
            }
            release_session(&modem_path, &session_id).await;
        }
    }
}

async fn get_session(conn: &Connection, session_id: &str) -> Option<UssdSession> {
    reap_expired_sessions(conn).await;
    sessions().lock().await.get(session_id).cloned()
}

async fn touch_session(session_id: &str, session: &UssdSession) -> bool {
    let mut map = sessions().lock().await;
    let Some(current) = map.get_mut(session_id) else {
        return false;
    };
    if current.modem_path != session.modem_path || current.line_id != session.line_id {
        return false;
    }
    current.last_activity_at = Instant::now();
    true
}

async fn session_still_active(session_id: &str, session: &UssdSession) -> bool {
    let map = sessions().lock().await;
    map.get(session_id).is_some_and(|current| {
        current.modem_path == session.modem_path && current.line_id == session.line_id
    })
}

async fn remove_session(session_id: &str, session: &UssdSession) {
    let removed = {
        let mut map = sessions().lock().await;
        map.get(session_id)
            .is_some_and(|current| current.modem_path == session.modem_path)
            .then(|| map.remove(session_id).is_some())
            .unwrap_or(false)
    };
    if removed {
        release_session(&session.modem_path, session_id).await;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UssdTransaction {
    pub text: String,
    pub raw: String,
    pub status: &'static str,
}

fn make_session_id() -> String {
    let sequence = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("ussd-{millis:x}-{sequence:x}")
}

fn validate_code(code: &str) -> Result<String, String> {
    let code = code.trim();
    if code.is_empty() || code.len() > 40 {
        return Err("USSD service code must be 1-40 characters".to_string());
    }
    if !code
        .chars()
        .all(|ch| ch.is_ascii_digit() || matches!(ch, '*' | '#' | '+'))
    {
        return Err("USSD service code contains an invalid character".to_string());
    }
    Ok(code.to_string())
}

fn validate_input(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() || input.len() > 182 {
        return Err("USSD input must be 1-182 characters".to_string());
    }
    if input
        .chars()
        .any(|ch| ch == '"' || ch == '\r' || ch == '\n' || ch.is_control())
    {
        return Err("USSD input contains a control character or quote".to_string());
    }
    Ok(input.to_string())
}

pub(crate) async fn start(
    conn: &Connection,
    line_id: &str,
    modem_path: &str,
    code: &str,
) -> Result<UssdResponse, String> {
    let code = validate_code(code)?;
    let operation = reserve_modem(modem_path).await?;
    let raw = modem_manager::run_ussd_at_command_for_modem(
        conn,
        modem_path,
        &format!(r#"AT+CUSD=1,"{code}",15"#),
    )
    .await;
    let raw = match raw {
        Ok(raw) => raw,
        Err(error) => {
            release_start(modem_path).await;
            return Err(error);
        }
    };
    if raw.trim().is_empty() {
        release_start(modem_path).await;
        return Err("modem returned an empty USSD response".to_string());
    }
    let transaction = match parse_ussd_response(&raw) {
        Ok(transaction) => transaction,
        Err(error) => {
            release_start(modem_path).await;
            return Err(error);
        }
    };

    let session_id = if transaction.status == "awaiting_input" {
        let id = make_session_id();
        reap_expired_sessions(conn).await;
        {
            let mut map = sessions().lock().await;
            map.insert(
                id.clone(),
                UssdSession {
                    line_id: line_id.to_string(),
                    modem_path: modem_path.to_string(),
                    last_activity_at: Instant::now(),
                    operation: operation.clone(),
                },
            );
        }
        if !bind_session(modem_path, &id, &operation).await {
            sessions().lock().await.remove(&id);
            release_start(modem_path).await;
            return Err("USSD session could not reserve the modem".to_string());
        }
        Some(id)
    } else {
        release_start(modem_path).await;
        None
    };
    Ok(to_response(line_id, transaction, session_id))
}

pub(crate) async fn continue_session(
    conn: &Connection,
    line_id: &str,
    modem_path: &str,
    session_id: &str,
    input: &str,
) -> Result<UssdResponse, String> {
    let input = validate_input(input)?;
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("USSD session id is required".to_string());
    }
    let session = get_session(conn, session_id)
        .await
        .ok_or_else(|| "USSD session does not exist or has expired".to_string())?;
    ensure_session_owner(&session, line_id, modem_path)?;

    let _operation = session.operation.lock().await;
    if !session_still_active(session_id, &session).await {
        return Err("USSD session does not exist or has expired".to_string());
    }
    let raw = modem_manager::run_ussd_at_command_for_modem(
        conn,
        modem_path,
        &format!(r#"AT+CUSD=1,"{input}",15"#),
    )
    .await;
    let raw = match raw {
        Ok(raw) => raw,
        Err(error) => {
            remove_session(session_id, &session).await;
            return Err(error);
        }
    };
    let transaction = match parse_ussd_response(&raw) {
        Ok(transaction) => transaction,
        Err(error) => {
            remove_session(session_id, &session).await;
            return Err(error);
        }
    };
    let keep_session = transaction.status == "awaiting_input";
    if keep_session {
        if !touch_session(session_id, &session).await {
            return Err("USSD session does not exist or has expired".to_string());
        }
    } else {
        remove_session(session_id, &session).await;
    }
    Ok(to_response(
        line_id,
        transaction,
        keep_session.then(|| session_id.to_string()),
    ))
}

pub(crate) async fn cancel_session(
    conn: &Connection,
    line_id: &str,
    modem_path: &str,
    session_id: &str,
) -> Result<UssdResponse, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("USSD session id is required".to_string());
    }
    let session = get_session(conn, session_id)
        .await
        .ok_or_else(|| "USSD session does not exist or has expired".to_string())?;
    ensure_session_owner(&session, line_id, modem_path)?;

    let _operation = session.operation.lock().await;
    if !session_still_active(session_id, &session).await {
        return Err("USSD session does not exist or has expired".to_string());
    }
    let raw = modem_manager::cancel_ussd_at_command_for_modem(conn, modem_path).await;
    remove_session(session_id, &session).await;
    raw.map(|raw| UssdResponse {
        line_id: line_id.to_string(),
        text: "USSD session cancelled".to_string(),
        raw,
        status: "terminated".to_string(),
        session_id: None,
        continueable: false,
    })
}

fn ensure_session_owner(
    session: &UssdSession,
    line_id: &str,
    modem_path: &str,
) -> Result<(), String> {
    if session.line_id != line_id || session.modem_path != modem_path {
        return Err("USSD session does not belong to the selected line or modem".to_string());
    }
    Ok(())
}

fn to_response(
    line_id: &str,
    transaction: UssdTransaction,
    session_id: Option<String>,
) -> UssdResponse {
    UssdResponse {
        line_id: line_id.to_string(),
        text: transaction.text,
        raw: transaction.raw,
        status: transaction.status.to_string(),
        continueable: session_id.is_some(),
        session_id,
    }
}

/// Parse the +CUSD line independently from serial ordering. `+CUSD` may be
/// delivered before or after the command's `OK`; this parser only depends on
/// the complete buffered transcript and never treats unrelated URCs as data.
pub(crate) fn parse_ussd_response(raw: &str) -> Result<UssdTransaction, String> {
    let upper = raw.to_ascii_uppercase();
    let start = upper
        .find("+CUSD:")
        .ok_or_else(|| "modem did not return a +CUSD response".to_string())?;
    let remaining = &raw[start..];
    let end = remaining.find(['\r', '\n']).unwrap_or(remaining.len());
    let line = remaining[..end].trim();
    let prefix_end = line
        .find(':')
        .ok_or_else(|| "+CUSD response is missing its colon".to_string())?;
    let fields = csv_fields(&line[prefix_end + 1..]);
    if fields.len() < 2 {
        return Err("+CUSD response is missing status or text".to_string());
    }
    let code = fields[0]
        .trim()
        .parse::<u8>()
        .map_err(|_| "invalid +CUSD status code".to_string())?;
    let payload = fields[1].trim().trim_matches('"');
    let dcs = fields
        .get(2)
        .and_then(|value| value.trim().parse::<u16>().ok());
    let text = if dcs == Some(72) || looks_like_ucs2(payload) {
        decode_ucs2(payload).unwrap_or_else(|| payload.to_string())
    } else {
        payload.to_string()
    };
    Ok(UssdTransaction {
        text,
        raw: line.to_string(),
        status: match code {
            0 => "final",
            1 => "awaiting_input",
            2 => "terminated",
            _ => "failed",
        },
    })
}

fn csv_fields(value: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                if quoted && chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    quoted = !quoted;
                }
            }
            ',' if !quoted => {
                fields.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    fields.push(current.trim().to_string());
    fields
}

fn looks_like_ucs2(value: &str) -> bool {
    value.len() >= 4
        && value.len() % 4 == 0
        && value.chars().all(|ch| ch.is_ascii_hexdigit())
        && (value.starts_with("00") || value.chars().any(|ch| ch.is_ascii_alphabetic()))
}

fn decode_ucs2(value: &str) -> Option<String> {
    if !looks_like_ucs2(value) {
        return None;
    }
    let mut units = Vec::with_capacity(value.len() / 4);
    for chunk in value.as_bytes().chunks_exact(4) {
        let text = std::str::from_utf8(chunk).ok()?;
        units.push(u16::from_str_radix(text, 16).ok()?);
    }
    Some(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    use super::parse_ussd_response;

    #[test]
    fn parses_plain_text_with_real_crlf() {
        let result = parse_ussd_response(
            r#"
+CUSD: 0,"Balance: 12,34",15
OK
"#,
        )
        .expect("CUSD response");
        assert_eq!(result.status, "final");
        assert_eq!(result.text, "Balance: 12,34");
    }

    #[test]
    fn accepts_ok_before_or_after_the_cusd_urc() {
        for raw in [
            r#"
OK

+CUSD: 0,"done",15
"#,
            r#"
+CUSD: 0,"done",15
OK
"#,
        ] {
            let result = parse_ussd_response(raw).expect("CUSD response");
            assert_eq!(result.status, "final");
            assert_eq!(result.text, "done");
        }
    }

    #[test]
    fn parses_a_response_reassembled_from_split_read_chunks() {
        let chunks = [
            r#"
+CUS"#,
            r#"D: 1,"1 or 2",15"#,
            r#"
OK
"#,
        ];
        let raw = chunks.concat();
        let result = parse_ussd_response(&raw).expect("CUSD response");
        assert_eq!(result.status, "awaiting_input");
        assert_eq!(result.text, "1 or 2");
    }

    #[test]
    fn parses_ucs2_and_interactive_status() {
        let result = parse_ussd_response(r#"+CUSD: 1,"8BF78BF7",72"#).expect("CUSD response");
        assert_eq!(result.status, "awaiting_input");
        assert_eq!(result.text, "\u{8bf7}\u{8bf7}");
    }

    #[test]
    fn ignores_other_urcs_and_requires_cusd() {
        let result = parse_ussd_response(
            r#"
+CMTI: "SM",1
OK
"#,
        )
        .expect_err("missing CUSD should fail");
        assert!(result.contains("+CUSD"));
    }

    #[test]
    fn maps_terminal_and_error_statuses() {
        assert_eq!(
            parse_ussd_response(r#"+CUSD: 2,"bye",15"#).unwrap().status,
            "terminated"
        );
        assert_eq!(
            parse_ussd_response(r#"+CUSD: 4,"unsupported",15"#)
                .unwrap()
                .status,
            "failed"
        );
    }
}
