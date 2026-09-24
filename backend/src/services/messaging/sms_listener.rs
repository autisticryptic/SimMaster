//! SMS reception shared by ModemManager and explicitly enabled native storage.
//!
//! 通过 D-Bus 信号监听 ModemManager 的短信接收事件，并增加轮询兜底，
//! 以便在部分 eSIM/国际运营商场景下尽量减少漏收。
use crate::hardware::cellular::control::{
    cache_smsc_for_identity, list_modem_paths, sim_identity_for_modem,
};
use crate::platform::config::{ConfigManager, LineProfileConfig};
use crate::platform::db::{
    normalize_sms_timestamp_for_display, utc_sms_now_string, Database, SmsMessage,
};
use crate::services::line_registry::LineRuntimeRegistry;
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
            .unwrap_or_else(utc_sms_now_string),
        SmsIngestMode::Reconcile => normalize_sms_timestamp_for_display(&incoming.timestamp)
            .unwrap_or_else(utc_sms_now_string),
    }
}

fn should_forward_after_insert(mode: SmsIngestMode, forward_reconciled_new_sms: bool) -> bool {
    mode == SmsIngestMode::Live || forward_reconciled_new_sms
}

/// 从 SMS 对象路径读取短信内容
async fn read_sms_content(conn: &Connection, sms_path: &str) -> Option<IncomingSms> {
    if let Some((selector, _)) = sms_path
        .split_once(":sms:")
        .filter(|(s, _)| s.starts_with("native:"))
    {
        let sms = crate::hardware::cellular::backends::native_device(selector)
            .ok()?
            .message(sms_path)
            .await
            .ok()?;
        return Some(IncomingSms {
            path: sms.path,
            number: sms.number,
            content: sms.content,
            timestamp: sms.timestamp,
            smsc: sms.smsc,
        });
    }
    if crate::hardware::cellular::backends::active_native().is_some() {
        return None;
    }
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

fn schedule_sms_delete(
    conn: &Connection,
    config_manager: &Arc<ConfigManager>,
    line_id: &str,
    modem_path: &str,
    sms_path: String,
) {
    let conn_clone = conn.clone();
    let config_manager = Arc::clone(config_manager);
    let line_id = line_id.to_string();
    let modem_path = modem_path.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(SMS_DELETE_DELAY_SECS)).await;
        // Disabling a line during the delay must preserve the stored message.
        if !config_manager.get_line_profile(&line_id).enabled {
            return;
        }
        if crate::hardware::cellular::backends::is_native_selector(&modem_path) {
            if let Ok(device) = crate::hardware::cellular::backends::native_device(&modem_path) {
                if let Err(error) = device.delete_message(&sms_path).await {
                    warn!(%error, "Native SMS deletion deferred; stored message was not blindly removed");
                }
            }
            return;
        }
        if crate::hardware::cellular::backends::active_native().is_some() {
            return;
        }
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
    line_id: &'a str,
    config_manager: &'a Arc<ConfigManager>,
    mt_sms: &'a tokio::sync::broadcast::Sender<SmsMessage>,
}

#[derive(Clone, Copy)]
struct SmsScanContext<'a> {
    conn: &'a Connection,
    db: &'a Database,
    notification_sender: &'a Arc<NotificationSender>,
    config_manager: &'a Arc<ConfigManager>,
    line_registry: &'a LineRuntimeRegistry,
    mt_sms: &'a tokio::sync::broadcast::Sender<SmsMessage>,
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
        mt_sms,
    } = context;
    if !config_manager.get_line_profile(line_id).enabled {
        return;
    }
    let Some(incoming) = read_sms_content(conn, sms_path).await else {
        return;
    };
    if !config_manager.get_line_profile(line_id).enabled {
        return;
    }

    let marker = sms_marker(&incoming);
    let timestamp = sms_timestamp(&incoming, mode);
    match db.sms_exists_by_pdu_for_line(line_id, &marker) {
        Ok(true) => {
            schedule_sms_delete(conn, config_manager, line_id, modem_path, incoming.path);
            return;
        }
        Ok(false) => {}
        Err(e) => {
            warn!(error = %e, marker = %marker, "Failed to check SMS dedupe marker");
            return;
        }
    }

    if mode == SmsIngestMode::Reconcile {
        match db.incoming_sms_exists_by_timestamp_for_line(
            line_id,
            &incoming.number,
            &incoming.content,
            &timestamp,
        ) {
            Ok(true) => {
                schedule_sms_delete(conn, config_manager, line_id, modem_path, incoming.path);
                return;
            }
            Ok(false) => {}
            Err(e) => {
                warn!(error = %e, marker = %marker, "Failed to check SMS timestamp identity");
                return;
            }
        }

        match db.incoming_sms_exists_by_legacy_content_for_line(
            line_id,
            &incoming.number,
            &incoming.content,
        ) {
            Ok(true) => {
                schedule_sms_delete(conn, config_manager, line_id, modem_path, incoming.path);
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

    let policy = config_manager.get_line_sms_path_policy(line_id);
    if policy.dedupe_enabled {
        let fingerprint = message_fingerprint(&MessageFingerprintInput {
            service_center_timestamp: &timestamp,
            originator: &incoming.number,
            text: &incoming.content,
            segment_reference: None,
            segment_sequence: 1,
            segment_total: 1,
        });
        match db.claim_sms_dedup(line_id, &fingerprint, "modem") {
            Ok(true) => {}
            Ok(false) => {
                schedule_sms_delete(conn, config_manager, line_id, modem_path, incoming.path);
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
        Some(line_id),
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
                line_id: Some(line_id.to_string()),
            };
            let _ = mt_sms.send(sms.clone());
            if should_forward_after_insert(mode, forward_reconciled_new_sms) {
                let notification_sender = Arc::clone(notification_sender);
                tokio::spawn(async move {
                    let _ = notification_sender.forward_sms(&sms).await;
                });
            }

            schedule_sms_delete(conn, config_manager, line_id, modem_path, incoming.path);
        }
        Err(e) => {
            warn!(error = %e, path = %incoming.path, "Failed to store incoming SMS");
        }
    }
}

async fn list_sms_paths(conn: &Connection, modem_path: &str) -> zbus::Result<Vec<String>> {
    if crate::hardware::cellular::backends::is_native_selector(modem_path) {
        let device = crate::hardware::cellular::backends::native_device(modem_path)
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        return device
            .messages()
            .await
            .map(|messages| messages.into_iter().map(|m| m.path).collect())
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()).into());
    }
    if crate::hardware::cellular::backends::active_native().is_some() {
        return Err(zbus::fdo::Error::Failed("native_sms_owner_mismatch".into()).into());
    }
    let proxy = Proxy::new(conn, MM_SERVICE, modem_path, MM_MESSAGING).await?;
    let paths: Vec<OwnedObjectPath> = proxy.call("List", &()).await?;
    Ok(paths.into_iter().map(|path| path.to_string()).collect())
}

async fn scan_sms_paths(
    context: SmsScanContext<'_>,
    modem_path: &str,
    reason: &str,
    forward_new_sms: bool,
    line_id: &str,
) {
    let SmsScanContext {
        conn,
        db,
        notification_sender,
        config_manager,
        mt_sms,
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
                        mt_sms,
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

fn ims_sms_owns_reception(
    profile: &LineProfileConfig,
    vowifi_sms_ready: bool,
    cellular_ims_registered: bool,
) -> bool {
    profile.enabled
        && ((profile.vowifi.enabled && vowifi_sms_ready)
            || (profile.cellular_ims_connection_enabled && cellular_ims_registered))
}

fn modem_sms_scan_allowed(
    profile: &LineProfileConfig,
    present: bool,
    ims_owns_reception: bool,
) -> bool {
    present && profile.enabled && (!ims_owns_reception || profile.sms_path.cs_fallback_receiver)
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
    let Some(line) = line_registry.for_modem_path(modem_path).await else {
        return false;
    };
    let line_id = line.binding().line_id;
    let profile = config_manager.get_line_profile(&line_id);
    let vowifi_sms_ready = line.vowifi.snapshot().await.readiness().sms_ready;
    let cellular_ims_registered = line.cellular_ims.status().await.registered;
    ims_sms_owns_reception(&profile, vowifi_sms_ready, cellular_ims_registered)
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
    let Some(line) = line_registry.for_modem_path(modem_path).await else {
        debug!(
            modem_path = %modem_path,
            reason = %reason,
            "Deferring ModemManager SMS scan until the modem is bound to a line"
        );
        return;
    };
    let binding = line.binding();
    let line_id = binding.line_id;
    let profile = config_manager.get_line_profile(&line_id);
    if !modem_sms_scan_allowed(
        &profile,
        binding.present,
        modem_sms_paused_for_ims(config_manager, line_registry, modem_path).await,
    ) {
        debug!(
            reason = %reason,
            "Skipping modem SMS scan: line disabled/absent or IMS owns reception"
        );
        return;
    }
    if crate::hardware::cellular::backends::is_native_selector(modem_path) {
        let Ok(device) = crate::hardware::cellular::backends::native_device(modem_path) else {
            return;
        };
        if !device.spec.sms_reception_enabled {
            return;
        }
        // Initialize only AFTER line/reception admission, also after SIM changes.
        if let Err(error) = device.initialize_sms().await {
            warn!(line_id, %error, "Native SMS initialization deferred");
            return;
        }
    }
    scan_sms_paths(context, modem_path, reason, forward_new_sms, &line_id).await;
}

async fn native_sms_events_pending(
    device: &Arc<crate::hardware::cellular::backends::native::NativeDevice>,
    config_manager: &ConfigManager,
    line_registry: &LineRuntimeRegistry,
) -> bool {
    if !device.spec.sms_reception_enabled {
        return false;
    }
    let selector = device.spec.selector();
    let Some(line) = line_registry.for_modem_path(&selector).await else {
        return false;
    };
    let binding = line.binding();
    let profile = config_manager.get_line_profile(&binding.line_id);
    if !modem_sms_scan_allowed(
        &profile,
        binding.present,
        modem_sms_paused_for_ims(config_manager, line_registry, &selector).await,
    ) {
        return false;
    }
    // No new reader/thread and no modem writes: native IO verifies the port
    // generation before and after a bounded read under the physical gate.
    // A hint only requests the existing durable-ingest/content-checked scan.
    match device.poll_sms_events().await {
        Ok(events) => events.needs_sms_scan(),
        Err(_) => false, // Periodic reconciliation remains the loss/error fallback.
    }
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
    mt_sms: tokio::sync::broadcast::Sender<SmsMessage>,
    mut resync_receiver: SmsResyncReceiver,
) -> zbus::Result<()> {
    if let Some(fleet) = crate::hardware::cellular::backends::active_native() {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut next_full_scan = tokio::time::Instant::now();
        loop {
            let reason = tokio::select! {
                _ = interval.tick() => {
                    if tokio::time::Instant::now() >= next_full_scan {
                        next_full_scan = tokio::time::Instant::now() + Duration::from_secs(SMS_POLL_INTERVAL_SECS);
                        "native_poll".to_string()
                    } else {
                        "native_urc".to_string()
                    }
                },
                message = resync_receiver.recv() => match message {
                    Some(message) => message.reason,
                    None => return Ok(()),
                },
            };
            for device in fleet.all() {
                if !device.spec.sms_reception_enabled {
                    continue;
                }
                if reason == "native_urc"
                    && !native_sms_events_pending(&device, &config_manager, &line_registry).await
                {
                    continue;
                }
                maybe_scan_sms_paths(
                    SmsScanContext {
                        conn: &conn,
                        db: &db,
                        notification_sender: &notification_sender,
                        config_manager: &config_manager,
                        line_registry: &line_registry,
                        mt_sms: &mt_sms,
                    },
                    &device.spec.selector(),
                    &reason,
                    true,
                )
                .await;
            }
        }
    }
    info!("Starting SMS listener (ModemManager mode)");
    loop {
        let modem_paths = loop {
            match list_modem_paths(&conn).await {
                Ok(paths) if !paths.is_empty() => break paths,
                Ok(_) => {
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
            config_manager: &config_manager,
            line_registry: line_registry.as_ref(),
            mt_sms: &mt_sms,
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

                                let Some(signal_line) =
                                    line_registry.for_modem_path(&signal_path).await
                                else {
                                    warn!(
                                        modem_path = %signal_path,
                                        "Deferring incoming SMS until the modem is bound to a line"
                                    );
                                    continue;
                                };
                                let signal_line_id = signal_line.binding().line_id;
                                let cs_fallback_receiver = config_manager
                                    .get_line_sms_path_policy(&signal_line_id)
                                    .cs_fallback_receiver;
                                if modem_sms_paused_for_ims(
                                    &config_manager,
                                    &line_registry,
                                    &signal_path,
                                ).await
                                    && !cs_fallback_receiver
                                {
                                    debug!("Ignoring ModemManager SMS event while IMS owns reception");
                                    continue;
                                }

                                let sms_path_str = sms_path.to_string();
                                info!(path = %sms_path_str, "New SMS received");
                                // Give ModemManager a short moment to assemble multipart SMS content.
                                tokio::time::sleep(Duration::from_millis(500)).await;
                                process_sms_path(
                                    SmsIngestContext {
                                        conn: &conn,
                                        db: &db,
                                        notification_sender: &notification_sender,
                                        modem_path: signal_path.as_str(),
                                        line_id: &signal_line_id,
                                        config_manager: &config_manager,
                                        mt_sms: &mt_sms,
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
    fn disabled_or_absent_lines_never_admit_modem_sms_even_with_cs_fallback() {
        let mut profile = LineProfileConfig::for_line("line-0123456789abcdef0123456789abcdef");
        profile.sms_path.cs_fallback_receiver = true;
        assert!(modem_sms_scan_allowed(&profile, true, false));
        assert!(modem_sms_scan_allowed(&profile, true, true));
        assert!(!modem_sms_scan_allowed(&profile, false, false));
        profile.enabled = false;
        assert!(!modem_sms_scan_allowed(&profile, true, false));
        assert!(!modem_sms_scan_allowed(&profile, true, true));
        profile.enabled = true;
        profile.sms_path.cs_fallback_receiver = false;
        assert!(modem_sms_scan_allowed(&profile, true, false));
        assert!(!modem_sms_scan_allowed(&profile, true, true));
    }

    #[test]
    fn only_ready_ims_runtime_on_the_same_line_pauses_cs_sms() {
        let mut line_a = LineProfileConfig::for_line("line-0123456789abcdef0123456789abcdef");
        line_a.vowifi.enabled = true;

        assert!(!ims_sms_owns_reception(&line_a, false, false));
        assert!(ims_sms_owns_reception(&line_a, true, false));

        let line_b = LineProfileConfig::for_line("line-fedcba9876543210fedcba9876543210");
        assert!(
            !ims_sms_owns_reception(&line_b, true, false),
            "line A VoWiFi readiness must not pause line B"
        );

        line_a.vowifi.enabled = false;
        line_a.cellular_ims_connection_enabled = true;
        assert!(!ims_sms_owns_reception(&line_a, false, false));
        assert!(ims_sms_owns_reception(&line_a, false, true));
        line_a.enabled = false;
        assert!(!ims_sms_owns_reception(&line_a, true, true));
    }

    #[test]
    fn normalizes_sms_timestamp_with_short_timezone() {
        assert_eq!(
            normalize_sms_timestamp_for_display("2026-05-19 20:17:25+08").as_deref(),
            Some("2026-05-19T12:17:25Z")
        );
    }

    #[test]
    fn rejects_sms_timestamp_without_an_explicit_timezone() {
        assert_eq!(
            normalize_sms_timestamp_for_display("2026-05-19 16:50:26"),
            None
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
