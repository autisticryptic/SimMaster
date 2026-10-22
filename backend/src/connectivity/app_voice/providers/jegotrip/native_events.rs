//! Adapter for local notifications confirmed in the reviewed native SDK.
//! Only a trusted backend's already-decoded notification may reach this module.
//! Completion names are emitted AFTER native RPC result validation; they do not
//! contain another boolean result field. The adapter must use the operation and
//! owner captured at submission (the native update-status cookie), not the active
//! owner at the time the notification is received.
use super::{controller::Callback, session::BackendFailure};
use std::fmt;

/// Local submission context, not an invented field in the SDK/network payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeOperation {
    SdkLogin,
    HostingRegister,
    HostingUnregister,
}

pub struct NativeNotification<'a> {
    pub name: &'a str,
    pub operation: Option<NativeOperation>,
    pub message: Option<&'a [u8]>,
    pub from: Option<&'a [u8]>,
    pub time: Option<&'a [u8]>,
}

impl fmt::Debug for NativeNotification<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NativeNotification(<redacted>)")
    }
}

pub struct IncomingMessage {
    pub body: Vec<u8>,
    pub from: Option<Vec<u8>>,
    pub time: Option<Vec<u8>>,
}

impl fmt::Debug for IncomingMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IncomingMessage").field("body_bytes", &self.body.len())
            .field("from_present", &self.from.is_some()).field("time_present", &self.time.is_some())
            .finish()
    }
}

#[derive(Debug)]
pub enum NativeEvent {
    Callback(Callback),
    IncomingMessage(IncomingMessage),
}

fn message_copy(value: &[u8]) -> Result<Vec<u8>, &'static str> {
    if value.len() > 64 * 1024 { return Err("jegotrip_native_message_too_large"); }
    Ok(value.to_vec())
}

pub fn decode(notification: NativeNotification<'_>) -> Result<Option<NativeEvent>, &'static str> {
    let callback = match notification.name {
        "MtcCliServerLoginOkNotification" | "MtcCliServerLoginDidFailNotification" => {
            if notification.operation != Some(NativeOperation::SdkLogin) { return Ok(None); }
            Some(Callback::SdkLogin { success: notification.name == "MtcCliServerLoginOkNotification" })
        }
        "MtcSmgwUpdateStatusOkNotification" | "MtcSmgwUpdateStatusFailNotification" => {
            // Register and unregister share these names. Only the captured
            // register operation can acknowledge the pending hosting attempt.
            if notification.operation != Some(NativeOperation::HostingRegister) { return Ok(None); }
            Some(Callback::HostingAck { success: notification.name == "MtcSmgwUpdateStatusOkNotification" })
        }
        "MtcCliReconnectingNotification" | "MtcCliServerDidLogoutNotification"
            | "MtcCliServerLogoutedNotification" => Some(Callback::Failure(BackendFailure::Disconnected)),
        // Reconnect-ok alone does not acknowledge SIM hosting or delivery readiness.
        "MtcCliReconnectOkNotification" => None,
        "MtcSmgwMessageRecvNotification" => {
            let body = notification.message.ok_or("jegotrip_native_message_missing")?;
            return Ok(Some(NativeEvent::IncomingMessage(IncomingMessage {
                body: message_copy(body)?,
                from: notification.from.map(message_copy).transpose()?,
                time: notification.time.map(message_copy).transpose()?,
            })));
        }
        // SendMessageOk, UpdateStatus2Ok and getInfo belong to different commands;
        // they cannot promote the controller's pending updateStatus operation.
        _ => None,
    };
    Ok(callback.map(NativeEvent::Callback))
}

#[cfg(test)]
#[path = "native_events_tests.rs"]
mod tests;
