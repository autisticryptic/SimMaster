//! SMS Listener Module (ModemManager 版)
//!
//! 通过 D-Bus 信号监听 ModemManager 的短信接收事件，并增加轮询兜底，
//! 以便在部分 eSIM/国际运营商场景下尽量减少漏收。
use crate::services::line_registry::LineRuntimeRegistry;
use crate::hardware::cellular::modem_manager::{
    cache_smsc_for_identity, find_modem_path, list_modem_paths, sim_identity_for_modem,
};
use crate::platform::config::ConfigManager;
use crate::platform::db::{
    beijing_sms_now_string, normalize_sms_timestamp_for_display, Database, SmsMessage,
};
use crate::services::notify::notification::NotificationSender;
use crate::services::orchestrator::{message_fingerprint, MessageFingerprintInput};
use futures_util::StreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{Duration, MissedTickBehavior};
use tracing::{debug, info, warn};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{Connection, MessageStream, Proxy};

/// ModemManager 常量
const MM_SERVICE: &str = "org.freedesktop.ModemManager1";
const MM_MESSAGING: &str = "org.freedesktop.ModemManager1.Modem.Messaging";
const MM_SMS: &str = "org.freedesktop.ModemManager1.Sms";
const DBUS_PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const MM_SMS_STATE_RECEIVED: u32 = 3;
const SMS_DELETE_DELAY_SECS: u64 = 5;
const MODEM_RETRY_DELAY_SECS: u64 = 5;
const SMS_POLL_INTERVAL_SECS: u64 = 15;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SmsIngestMode {
    Live,
    Reconcile,
}

#[derive(Clone)]
pub struct SmsResyncHandle {
    sender: mpsc::UnboundedSender<SmsResyncRequest>,
}

#[derive(Debug)]
pub struct SmsResyncRequest {
    reason: String,
}

pub type SmsResyncReceiver = mpsc::UnboundedReceiver<SmsResyncRequest>;

pub fn sms_resync_channel() -> (SmsResyncHandle, SmsResyncReceiver) {
    let (sender, receiver) = mpsc::unbounded_channel();
    (SmsResyncHandle { sender }, receiver)
}

impl SmsResyncHandle {
    pub fn request_scan(&self, reason: impl Into<String>) -> bool {
        self.sender
            .send(SmsResyncRequest {
                reason: reason.into(),
            })
            .is_ok()
    }
}

#[derive(Debug)]
struct IncomingSms {
    path: String,
    number: String,
    content: String,
    timestamp: String,
    smsc: String,
}

fn decode_sms_data(value: &OwnedValue) -> Option<String> {
    let bytes = Vec::<u8>::try_from(value.clone()).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn sms_marker(incoming: &IncomingSms) -> String {
    let raw = if incoming.timestamp.is_empty() {
        format!(
            "{}\n{}\n{}",
            incoming.path, incoming.number, incoming.content
        )
    } else {
        format!(
            "{}\n{}\n{}",
            incoming.number, incoming.timestamp, incoming.content
        )
    };
    format!("mmfp:{:x}", md5::compute(raw))
}

fn sms_timestamp(incoming: &IncomingSms, mode: SmsIngestMode) -> String {
    match mode {
        SmsIngestMode::Live => normalize_sms_timestamp_for_display(&incoming.timestamp)
            .unwrap_or_else(beijing_sms_now_string),
        SmsIngestMode::Reconcile => normalize_sms_timestamp_for_display(&incoming.timestamp)
            .unwrap_or_else(beijing_sms_now_string),
    }
}

fn should_forward_after_insert(mode: SmsIngestMode, forward_reconciled_new_sms: bool) -> bool {
    mode == SmsIngestMode::Live || forward_reconciled_new_sms
}

/// 从 SMS 对象路径读取短信内容
async fn read_sms_content(conn: &Connection, sms_path: &str) -> Option<IncomingSms> {
    let proxy = Proxy::new(conn, MM_SERVICE, sms_path, DBUS_PROPERTIES)
        .await
        .ok()?;

    let props: std::collections::HashMap<String, OwnedValue> =
        proxy.call("GetAll", &(MM_SMS,)).await.ok()?;

    let number = props
        .get("Number")
        .and_then(|v| String::try_from(v.clone()).ok())
        .unwrap_or_else(|| "Unknown".to_string());

    let text = props
        .get("Text")
        .and_then(|v| String::try_from(v.clone()).ok())
        .unwrap_or_default();
    let data = props.get("Data").and_then(decode_sms_data);
    let smsc = ["SMSC", "Smsc", "SmsCenter"]
        .iter()
        .find_map(|key| {
            props
                .get(*key)
                .and_then(|v| String::try_from(v.clone()).ok())
        })
        .unwrap_or_default();
    let timestamp = ["Timestamp", "Time", "ReceivedTimestamp"]
        .iter()
        .find_map(|key| {
            props
                .get(*key)
                .and_then(|v| String::try_from(v.clone()).ok())
        })
        .unwrap_or_default();

    let state = props
        .get("State")
        .and_then(|v| u32::try_from(v.clone()).ok())
        .unwrap_or(0);

    if state != MM_SMS_STATE_RECEIVED {
        return None;
    }

    let content = if text.is_empty() {
        data.unwrap_or_default()
    } else {
        text
    };

    Some(IncomingSms {
        path: sms_path.to_string(),
        number,
        content,
        timestamp,
        smsc,
    })
}

fn schedule_sms_delete(conn: &Connection, modem_path: &str, sms_path: String) {
    let conn_clone = conn.clone();
    let modem_path = modem_path.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(SMS_DELETE_DELAY_SECS)).await;
        let proxy = Proxy::new(&conn_clone, MM_SERVICE, modem_path.as_str(), MM_MESSAGING).await;
        match proxy {
            Ok(proxy) => {
                let sms_path_obj = zbus::zvariant::ObjectPath::try_from(sms_path.as_str());
                match sms_path_obj {
                    Ok(path) => {
                        if let Err(e) = proxy.call::<_, _, ()>("Delete", &(path,)).await {
                            warn!(error = %e, path = %sms_path, "Failed to delete processed SMS from ModemManager");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, path = %sms_path, "Invalid SMS path for deletion");
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, path = %sms_path, "Failed to create Messaging proxy for SMS deletion");
            }
        }
    });
}

struct SmsIngestContext<'a> {
    conn: &'a Connection,
    db: &'a Database,
    notification_sender: &'a Arc<NotificationSender>,
    modem_path: &'a str,
    line_id: Option<&'a str>,
    config_manager: &'a ConfigManager,
}

#[derive(Clone, Copy)]
struct SmsScanContext<'a> {
    conn: &'a Connection,
    db: &'a Database,
    notification_sender: &'a Arc<NotificationSender>,
    config_manager: &'a ConfigManager,
    line_registry: &'a LineRuntimeRegistry,
}

async fn process_sms_path(
    context: SmsIngestContext<'_>,
    sms_path: &str,
    mode: SmsIngestMode,
    forward_reconciled_new_sms: bool,
) {
    let SmsIngestContext {
        conn,
        db,
        notification_sender,
        modem_path,
        line_id,
        config_manager,
    } = context;
    let Some(incoming) = read_sms_content(conn, sms_path).await else {
        return;
    };

    let marker = sms_marker(&incoming);
    let timestamp = sms_timestamp(&incoming, mode);
    match db.sms_exists_by_pdu(&marker) {
        Ok(true) => {
            schedule_sms_delete(conn, modem_path, incoming.path);
            return;
        }
        Ok(false) => {}
        Err(e) => {
            warn!(error = %e, marker = %marker, "Failed to check SMS dedupe marker");
            return;
        }
    }

    if mode == SmsIngestMode::Reconcile {
        match db.incoming_sms_exists_by_timestamp(&incoming.number, &incoming.content, &timestamp) {
            Ok(true) => {
                schedule_sms_delete(conn, modem_path, incoming.path);
                return;
            }
            Ok(false) => {}
            Err(e) => {
                warn!(error = %e, marker = %marker, "Failed to check SMS timestamp identity");
                return;
            }
        }

        match db.incoming_sms_exists_by_legacy_content(&incoming.number, &incoming.content) {
            Ok(true) => {
                schedule_sms_delete(conn, modem_path, incoming.path);
                return;
            }
            Ok(false) => {}
            Err(e) => {
                warn!(error = %e, marker = %marker, "Failed to check legacy SMS identity");
                return;
            }
        }
    }

    info!(
        path = %incoming.path,
        from = %incoming.number,
        len = incoming.content.len(),
        "SMS content read"
    );

    if !incoming.smsc.is_empty() {
        if let Some(identity) = sim_identity_for_modem(conn, modem_path).await {
            cache_smsc_for_identity(db, &identity, &incoming.smsc, "sms_object");
        }
    }

    let policy = config_manager.get_sms_path_policy();
    if policy.dedupe_enabled {
        let fingerprint = message_fingerprint(&MessageFingerprintInput {
            service_center_timestamp: &timestamp,
            originator: &incoming.number,
            text: &incoming.content,
            segment_reference: None,
            segment_sequence: 1,
            segment_total: 1,
        });
        match db.claim_sms_dedup(&fingerprint, "modem") {
            Ok(true) => {}
            Ok(false) => {
                schedule_sms_delete(conn, modem_path, incoming.path);
                return;
            }
            Err(error) => {
                warn!(error = %error, "Failed to claim cross-transport SMS fingerprint");
                return;
            }
        }
    }

    match db.insert_sms_at_with_transport_for_line(
        "incoming",
        &incoming.number,
        &incoming.content,
        &timestamp,
        "received",
        Some(&marker),
        "modem",
        line_id,
    ) {
        Ok(id) => {
            let sms = SmsMessage {
                id,
                direction: "incoming".to_string(),
                phone_number: incoming.number,
                content: incoming.content,
                timestamp,
                status: "received".to_string(),
                pdu: Some(marker),
                transport: "modem".to_string(),
                line_id: line_id.map(str::to_string),
            };
            if should_forward_after_insert(mode, forward_reconciled_new_sms) {
                let notification_sender = Arc::clone(notification_sender);
                tokio::spawn(async move {
                    let _ = notification_sender.forward_sms(&sms).await;
                });
            }

            schedule_sms_delete(conn, modem_path, incoming.path);
        }
        Err(e) => {
            warn!(error = %e, path = %incoming.path, "Failed to store incoming SMS");
        }
    }
}

async fn list_sms_paths(conn: &Connection, modem_path: &str) -> zbus::Result<Vec<String>> {
    let proxy = Proxy::new(conn, MM_SERVICE, modem_path, MM_MESSAGING).await?;
    let paths: Vec<OwnedObjectPath> = proxy.call("List", &()).await?;
    Ok(paths.into_iter().map(|path| path.to_string()).collect())
}

async fn scan_sms_paths(
    context: SmsScanContext<'_>,
    modem_path: &str,
    reason: &str,
    forward_new_sms: bool,
    line_id: Option<&str>,
) {
    let SmsScanContext {
        conn,
        db,
        notification_sender,
        config_manager,
        ..
    } = context;
    match list_sms_paths(conn, modem_path).await {
        Ok(paths) => {
            if !paths.is_empty() {
                info!(
                    modem_path = %modem_path,
                    count = paths.len(),
                    reason = %reason,
                    "Scanning ModemManager SMS objects"
                );
            }
            for sms_path in paths {
                process_sms_path(
                    SmsIngestContext {
                        conn,
                        db,
                        notification_sender,
                        modem_path,
                        line_id,
                        config_manager,
                    },
                    &sms_path,
                    SmsIngestMode::Reconcile,
                    forward_new_sms,
                )
                .await;
            }
        }
        Err(e) => {
            warn!(
                error = %e,
                modem_path = %modem_path,
                reason = %reason,
                "Failed to scan ModemManager SMS objects"
            );
        }
    }
}

fn modem_sms_paused_for_vowifi(config_manager: &ConfigManager) -> bool {
    let config = config_manager.get_vowifi_config();
    config.feature_enabled && config.connection_enabled
}

/// Whether the ModemManager SMS scan should be suppressed because an IMS SMS
/// path (VoWiFi or VoLTE) has taken over reception. Mirrors the reference
/// behavior "SMS listener paused while VoLTE IMS SMS path is registered": once
/// an IMS leg owns MT SMS, the CS/modem scan must stand down to avoid duplicate
/// delivery.
async fn modem_sms_paused_for_ims(
    config_manager: &ConfigManager,
    line_registry: &LineRuntimeRegistry,
    modem_path: &str,
) -> bool {
    if modem_sms_paused_for_vowifi(config_manager) {
        return true;
    }
    line_registry
        .for_modem_path(modem_path)
        .await
        .is_some_and(|line| {
            let profile = config_manager.get_line_profile(&line.binding().line_id);
            profile.enabled && profile.volte_connection_enabled
        })
}

async fn maybe_scan_sms_paths(
    context: SmsScanContext<'_>,
    modem_path: &str,
    reason: &str,
    forward_new_sms: bool,
) {
    let SmsScanContext {
        config_manager,
        line_registry,
        ..
    } = context;
    if modem_sms_paused_for_ims(config_manager, line_registry, modem_path).await
        && !config_manager.get_sms_path_policy().cs_fallback_receiver
    {
        debug!(
            reason = %reason,
            "Skipping ModemManager SMS scan while an IMS (VoWiFi/VoLTE) SMS path is active"
        );
        return;
    }
    let line = line_registry.for_modem_path(modem_path).await;
    let line_id = line.as_ref().map(|line| line.binding().line_id);
    scan_sms_paths(
        context,
        modem_path,
        reason,
        forward_new_sms,
        line_id.as_deref(),
    )
    .await;
}

async fn scan_all_modems_or_rebind(
    context: SmsScanContext<'_>,
    modem_paths: &[String],
    reason: &str,
    forward_new_sms: bool,
) -> bool {
    let conn = context.conn;
    match list_modem_paths(conn).await {
        Ok(current_paths) if current_paths == modem_paths => {
            for modem_path in modem_paths {
                maybe_scan_sms_paths(context, modem_path, reason, forward_new_sms).await;
            }
            true
        }
        Ok(current_paths) => {
            info!(
                old_modem_paths = ?modem_paths,
                new_modem_paths = ?current_paths,
                reason = %reason,
                "SMS listener detected modem inventory change"
            );
            false
        }
        Err(e) => {
            warn!(
                error = %e,
                reason = %reason,
                "SMS listener lost modem while scanning"
            );
            false
        }
    }
}

async fn call_dbus_match(conn: &Connection, method: &str, rule: &str) -> zbus::Result<()> {
    let dbus_proxy = Proxy::new(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await?;
    dbus_proxy.call::<_, _, ()>(method, &(rule,)).await
}

/// Start SMS listener (ModemManager 版)
///
/// 监听 ModemManager 的 Messaging.Added 信号。
pub async fn start_sms_listener(
    conn: Connection,
    db: Arc<Database>,
    notification_sender: Arc<NotificationSender>,
    config_manager: Arc<ConfigManager>,
    line_registry: Arc<LineRuntimeRegistry>,
    mut resync_receiver: SmsResyncReceiver,
) -> zbus::Result<()> {
    info!("Starting SMS listener (ModemManager mode)");
    loop {
        let modem_paths = loop {
            match list_modem_paths(&conn).await {
                Ok(paths) if !paths.is_empty() => break paths,
                Ok(_) => {
                    // Preserve the existing recovery scan/caching behavior.
                    let _ = find_modem_path(&conn).await;
                    tokio::time::sleep(Duration::from_secs(MODEM_RETRY_DELAY_SECS)).await;
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        retry_after_secs = MODEM_RETRY_DELAY_SECS,
                        "SMS listener waiting for modem"
                    );
                    tokio::time::sleep(Duration::from_secs(MODEM_RETRY_DELAY_SECS)).await;
                }
            }
        };

        let mut rules = Vec::with_capacity(modem_paths.len());
        let mut registration_failed = false;
        for modem_path in &modem_paths {
            let rule = format!(
                "type='signal',sender='{}',interface='{}',member='Added',path='{}'",
                MM_SERVICE, MM_MESSAGING, modem_path
            );
            if let Err(e) = call_dbus_match(&conn, "AddMatch", rule.as_str()).await {
                warn!(
                    error = %e,
                    modem_path = %modem_path,
                    retry_after_secs = MODEM_RETRY_DELAY_SECS,
                    "Failed to register SMS listener match"
                );
                registration_failed = true;
                break;
            }
            rules.push(rule);
        }
        if registration_failed {
            for rule in &rules {
                let _ = call_dbus_match(&conn, "RemoveMatch", rule).await;
            }
            tokio::time::sleep(Duration::from_secs(MODEM_RETRY_DELAY_SECS)).await;
            continue;
        }

        info!(modem_paths = ?modem_paths, "SMS listeners registered, waiting for messages...");

        let scan_context = SmsScanContext {
            conn: &conn,
            db: db.as_ref(),
            notification_sender: &notification_sender,
            config_manager: config_manager.as_ref(),
            line_registry: line_registry.as_ref(),
        };
        for modem_path in &modem_paths {
            maybe_scan_sms_paths(scan_context, modem_path, "initial", false).await;
        }

        let mut stream = MessageStream::from(&conn);
        let mut poll_interval = tokio::time::interval(Duration::from_secs(SMS_POLL_INTERVAL_SECS));
        poll_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        poll_interval.tick().await;

        loop {
            tokio::select! {
                maybe_msg = stream.next() => {
                    let msg = match maybe_msg {
                        Some(Ok(msg)) => msg,
                        Some(Err(e)) => {
                            warn!(error = %e, "SMS listener stream error");
                            break;
                        }
                        None => break,
                    };

                    if let Some(member) = msg.header().member() {
                        if member.as_str() == "Added" {
                            let Some(signal_path) = msg.header().path().map(|path| path.to_string()) else {
                                continue;
                            };
                            if !modem_paths.contains(&signal_path) {
                                continue;
                            }
                            if let Ok((sms_path, received)) = msg
                                .body()
                                .deserialize::<(zbus::zvariant::ObjectPath, bool)>()
                            {
                                if !received {
                                    continue;
                                }

                                if modem_sms_paused_for_ims(
                                    &config_manager,
                                    &line_registry,
                                    &signal_path,
                                ).await
                                    && !config_manager
                                        .get_sms_path_policy()
                                        .cs_fallback_receiver
                                {
                                    debug!("Ignoring ModemManager SMS event while IMS owns reception");
                                    continue;
                                }

                                let sms_path_str = sms_path.to_string();
                                info!(path = %sms_path_str, "New SMS received");
                                let signal_line = line_registry.for_modem_path(&signal_path).await;
                                let signal_line_id =
                                    signal_line.as_ref().map(|line| line.binding().line_id);

                                // Give ModemManager a short moment to assemble multipart SMS content.
                                tokio::time::sleep(Duration::from_millis(500)).await;
                                process_sms_path(
                                    SmsIngestContext {
                                        conn: &conn,
                                        db: &db,
                                        notification_sender: &notification_sender,
                                        modem_path: signal_path.as_str(),
                                        line_id: signal_line_id.as_deref(),
                                        config_manager: &config_manager,
                                    },
                                    &sms_path_str,
                                    SmsIngestMode::Live,
                                    false,
                                )
                                .await;
                            }
                        }
                    }
                }
                _ = poll_interval.tick() => {
                    if !scan_all_modems_or_rebind(
                        scan_context,
                        &modem_paths,
                        "poll",
                        true,
                    ).await {
                        break;
                    }
                }
                Some(request) = resync_receiver.recv() => {
                    info!(reason = %request.reason, "SMS resync requested");
                    if !scan_all_modems_or_rebind(
                        scan_context,
                        &modem_paths,
                        request.reason.as_str(),
                        false,
                    ).await {
                        break;
                    }
                }
            }
        }

        for rule in &rules {
            if let Err(e) = call_dbus_match(&conn, "RemoveMatch", rule).await {
                warn!(error = %e, "Failed to remove SMS listener match");
            }
        }

        warn!(
            retry_after_secs = MODEM_RETRY_DELAY_SECS,
            "SMS listener stream ended, re-registering after delay"
        );
        tokio::time::sleep(Duration::from_secs(MODEM_RETRY_DELAY_SECS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pauses_modem_sms_when_vowifi_connection_is_enabled() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-sms-listener-vowifi-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let manager = ConfigManager::new(path.clone());

        assert!(!modem_sms_paused_for_vowifi(&manager));
        manager.set_vowifi_feature_enabled(true).unwrap();
        assert!(!modem_sms_paused_for_vowifi(&manager));
        manager.set_vowifi_connection_enabled(true).unwrap();
        assert!(modem_sms_paused_for_vowifi(&manager));
        manager.set_vowifi_connection_enabled(false).unwrap();
        assert!(!modem_sms_paused_for_vowifi(&manager));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn normalizes_sms_timestamp_with_short_timezone() {
        assert_eq!(
            normalize_sms_timestamp_for_display("2026-05-19 20:17:25+08").as_deref(),
            Some("2026-05-19 20:17:25")
        );
    }

    #[test]
    fn keeps_naive_sms_timestamp_frontend_parseable() {
        assert_eq!(
            normalize_sms_timestamp_for_display("2026-05-19 16:50:26").as_deref(),
            Some("2026-05-19 16:50:26")
        );
    }

    #[test]
    fn rejects_unparseable_sms_timestamp() {
        assert_eq!(normalize_sms_timestamp_for_display("not-a-date"), None);
    }

    #[test]
    fn forwards_live_sms_after_insert() {
        assert!(should_forward_after_insert(SmsIngestMode::Live, false));
    }

    #[test]
    fn forwards_reconciled_sms_only_when_enabled_for_scan() {
        assert!(should_forward_after_insert(SmsIngestMode::Reconcile, true));
        assert!(!should_forward_after_insert(
            SmsIngestMode::Reconcile,
            false
        ));
    }
}
