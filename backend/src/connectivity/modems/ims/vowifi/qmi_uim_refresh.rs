//! QMI UIM refresh subscription (libqmi 1.36.0 `uim.json`):
//! Refresh Register 0x002A, Refresh Complete 0x002C, Refresh indication 0x0033.
//!
//! This is a *supplementary* detector: it never reads or writes card content
//! itself. On completion the IMS owner re-reads provisioning with the existing
//! bounded snapshot and decides. When the modem does not support the request we
//! return None and the boundary checks remain the only detector.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub(crate) const QMI_UIM_REFRESH_REGISTER: u16 = 0x002A;
pub(crate) const QMI_UIM_REFRESH_COMPLETE: u16 = 0x002C;
pub(crate) const QMI_UIM_REFRESH_INDICATION: u16 = 0x0033;
const SESSION_TLV: u8 = 0x01;
const INFO_TLV: u8 = 0x02;
const EVENT_TLV: u8 = 0x10;
/// PRIMARY_GW_PROVISIONING; card-slot sessions use 6..=10 for slots 1..=5.
const SESSION_PRIMARY_GW: u8 = 0;
const STAGE_WAIT_FOR_OK: u8 = 0;
const STAGE_END_WITH_SUCCESS: u8 = 2;
const READ_POLL: Duration = Duration::from_millis(250);
/// Bounded deny: never hold operator provisioning hostage for longer.
const MAX_DENY: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefreshEvent {
    /// Card content changed and the modem finished the refresh procedure.
    Completed,
    /// Refresh ended in failure; treat as "no evidence of change".
    Failed,
}

pub(crate) struct RefreshSubscription {
    busy: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    events: tokio::sync::mpsc::UnboundedReceiver<RefreshEvent>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RefreshSubscription {
    /// Set while a REGISTER/AKA exchange is in flight, so the vote can defer.
    pub(crate) fn set_busy(&self, busy: bool) {
        self.busy.store(busy, Ordering::SeqCst);
    }

    pub(crate) fn poll_event(&mut self) -> Option<RefreshEvent> {
        self.events.try_recv().ok()
    }

    /// Bounded stop: the listener polls with a short read timeout, so the join
    /// cannot hang on a silent modem. Client release happens inside the thread.
    pub(crate) fn stop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for RefreshSubscription {
    fn drop(&mut self) {
        self.stop();
    }
}

fn session_tlv(session_type: u8, aid: &[u8]) -> Vec<u8> {
    let mut value = vec![session_type, aid.len().min(u8::MAX as usize) as u8];
    value.extend_from_slice(&aid[..aid.len().min(u8::MAX as usize)]);
    value
}

fn register_info_tlv() -> Vec<u8> {
    // Register flag = 1, vote for init = 1, no explicit file list.
    vec![1, 1, 0, 0]
}

fn complete_info_tlv(success: bool) -> Vec<u8> {
    vec![u8::from(success)]
}

/// Parse the indication's Event TLV into (stage, session_type, aid).
fn parse_event(value: &[u8]) -> Option<(u8, u8, Vec<u8>)> {
    let stage = *value.first()?;
    let session_type = *value.get(2)?;
    let aid_len = usize::from(*value.get(3)?);
    let aid = value.get(4..4 + aid_len)?.to_vec();
    Some((stage, session_type, aid))
}

/// A refresh indication is only ours when the session (and AID, when reported)
/// matches the subscription. Unknown shapes are ignored, never guessed.
fn event_matches(session_type: u8, aid: &[u8], event: &(u8, u8, Vec<u8>)) -> bool {
    event.1 == session_type && (event.2.is_empty() || aid.is_empty() || event.2 == aid)
}

struct Voter {
    busy: Arc<AtomicBool>,
    denied_since: Option<std::time::Instant>,
}

impl Voter {
    fn new(busy: Arc<AtomicBool>) -> Self {
        Self { busy, denied_since: None }
    }

    /// Deny while an exchange is in flight, but never longer than MAX_DENY.
    #[cfg(test)]
    fn force_denied_since(&mut self, at: std::time::Instant) {
        self.denied_since = Some(at);
    }

    fn allow(&mut self) -> bool {
        if self.busy.load(Ordering::SeqCst) {
            let since = *self.denied_since.get_or_insert_with(std::time::Instant::now);
            if since.elapsed() < MAX_DENY {
                return false;
            }
        }
        self.denied_since = None;
        true
    }
}

fn handle_message(
    message: &QmiMessage,
    session_type: u8,
    aid: &[u8],
    client: u8,
    voter: &mut Voter,
    connection: &mut QmiProxyConnection,
    events: &tokio::sync::mpsc::UnboundedSender<RefreshEvent>,
) -> Result<(), QmiUimError> {
    if message.service != QMUX_UIM_SERVICE || message.message_id != QMI_UIM_REFRESH_INDICATION {
        return Ok(());
    }
    let Some(value) = find_tlv(message, EVENT_TLV) else { return Ok(()); };
    let Some(event) = parse_event(value) else { return Ok(()); };
    if !event_matches(session_type, aid, &event) {
        return Ok(());
    }
    match event.0 {
        STAGE_WAIT_FOR_OK => {
            let allow = voter.allow();
            let request = QmiMessage {
                service: QMUX_UIM_SERVICE,
                client_id: client,
                transaction_id: connection.take_service_transaction(),
                message_id: QMI_UIM_REFRESH_COMPLETE,
                tlvs: vec![
                    QmiTlv { tlv_type: SESSION_TLV, value: session_tlv(session_type, aid) },
                    QmiTlv { tlv_type: INFO_TLV, value: complete_info_tlv(allow) },
                ],
            };
            connection.send_message(&request)?;
            tracing::info!(allowed = allow, "UIM refresh vote sent");
        }
        STAGE_END_WITH_SUCCESS => {
            let _ = events.send(RefreshEvent::Completed);
        }
        3 => {
            let _ = events.send(RefreshEvent::Failed);
        }
        _ => {}
    }
    Ok(())
}

/// Start a best-effort subscription. `None` means the modem rejected or does not
/// implement the request; the caller keeps the existing boundary checks.
pub(crate) fn start(
    proxy_socket: &str,
    device_path: &str,
    slot: u8,
    aid: Vec<u8>,
) -> Option<RefreshSubscription> {
    if slot == 0 || device_path.is_empty() { return None; }
    let session_type = if slot == 1 { SESSION_PRIMARY_GW } else { 5 + slot };
    let busy = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let (sender, events) = tokio::sync::mpsc::unbounded_channel();
    let thread_busy = busy.clone();
    let thread_cancel = cancel.clone();
    let proxy = proxy_socket.to_owned();
    let device = device_path.to_owned();
    let thread = std::thread::Builder::new()
        .name(format!("uim-refresh-{slot}"))
        .spawn(move || {
            run(&proxy, &device, session_type, &aid, thread_busy, thread_cancel, sender);
        })
        .ok()?;
    Some(RefreshSubscription { busy, cancel, events, thread: Some(thread) })
}

fn run(
    proxy_socket: &str,
    device_path: &str,
    session_type: u8,
    aid: &[u8],
    busy: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    events: tokio::sync::mpsc::UnboundedSender<RefreshEvent>,
) {
    let Ok(mut connection) = QmiProxyConnection::connect(proxy_socket, Duration::from_secs(5)) else {
        #[cfg(test)] note_exit("connect");
        return;
    };
    if connection.proxy_open(device_path).is_err() {
        #[cfg(test)] note_exit("proxy_open");
        return;
    }
    let Ok(client) = connection.allocate_uim_cid() else {
        #[cfg(test)] note_exit("allocate");
        return;
    };
    let result = (|| -> Result<(), QmiUimError> {
        let request = QmiMessage {
            service: QMUX_UIM_SERVICE,
            client_id: client,
            transaction_id: connection.take_service_transaction(),
            message_id: QMI_UIM_REFRESH_REGISTER,
            tlvs: vec![
                QmiTlv { tlv_type: SESSION_TLV, value: session_tlv(session_type, aid) },
                QmiTlv { tlv_type: INFO_TLV, value: register_info_tlv() },
            ],
        };
        connection.send_message(&request)?;
        connection.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut registered = false;
        while !registered && !cancel.load(Ordering::SeqCst) {
            match connection.read_message() {
                Ok(message) if message.message_id == QMI_UIM_REFRESH_REGISTER => {
                    ensure_success(&message)?;
                    registered = true;
                }
                Ok(_) => continue,
                Err(QmiUimError::Io(error)) if is_timeout(&error) => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        if !registered {
            #[cfg(test)] note_exit("register");
            return Ok(());
        }
        connection.set_read_timeout(Some(READ_POLL))?;
        let mut voter = Voter::new(busy);
        while !cancel.load(Ordering::SeqCst) {
            match connection.read_message() {
                Ok(message) => handle_message(&message, session_type, aid, client, &mut voter, &mut connection, &events)?,
                Err(QmiUimError::Io(error)) if is_timeout(&error) => continue,
                Err(_) => break,
            }
        }
        Ok(())
    })();
    match &result {
        Ok(()) => { #[cfg(test)] note_exit("closed"); }
        Err(error) => {
            #[cfg(test)] note_exit("error");
            tracing::debug!(%error, "UIM refresh subscription ended");
        }
    }
    let _ = connection.release_uim_cid(client);
}

fn is_timeout(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

#[cfg(test)]
static TEST_EXIT: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(None);
#[cfg(test)]
fn note_exit(reason: &'static str) {
    *TEST_EXIT.lock().unwrap() = Some(reason);
}
#[cfg(test)]
pub(crate) fn last_exit() -> Option<&'static str> {
    *TEST_EXIT.lock().unwrap()
}

#[cfg(test)]
#[path = "qmi_uim_refresh_tests.rs"]
mod tests;
