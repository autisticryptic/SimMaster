//! Supported Smgw text subset recovered from the original native/DEX path.
//!
//! 510E3C registers EXACT Sip.Request -> 524104/524110 -> 523EC0. The latter
//! copies body and Sip.Message.From/Time into type5 JSON strings (585608,
//! 250BF4, 61DFF4/24F738); JCNotify/JCMessageChannel then insert plain text into
//! the app's SMS history. No PDU/base64/JSON payload decoding or random remote id.
//! Other kinds, missing sender and unsupported encoding/time stay unclassified.
use super::native_events::IncomingMessage;
use chrono::{DateTime, SecondsFormat, Utc};
use std::fmt;

pub(super) const SMGW_RECEIVER: &[u8] = b"Sip.Request";
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_SENDER_BYTES: usize = 1024;
// End of UTC year9999: history timestamps must remain ordinary RFC3339, not
// an extended-year value the existing history/API readers cannot round-trip.
const MAX_SENT_MILLIS: i64 = 253_402_300_799_999;

pub(super) struct SmgwText<'a> {
    text: &'a str,
    sender: &'a str,
    sent_at_millis: Option<i64>,
    raw_time: Option<&'a [u8]>,
}
impl fmt::Debug for SmgwText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("SmgwText(<redacted>)") }
}
impl SmgwText<'_> {
    pub(super) fn text(&self) -> &str { self.text }
    pub(super) fn sender(&self) -> &str { self.sender }
    pub(super) fn sent_at_millis(&self) -> Option<i64> { self.sent_at_millis }
    pub(super) fn raw_time(&self) -> Option<&[u8]> { self.raw_time }
    pub(super) fn timestamp(&self, received_at: &str) -> Option<String> {
        let time = match self.sent_at_millis {
            Some(millis) => DateTime::<Utc>::from_timestamp_millis(millis)?,
            None => DateTime::parse_from_rfc3339(received_at).ok()?.with_timezone(&Utc),
        };
        Some(time.to_rfc3339_opts(SecondsFormat::Millis, true))
    }
    pub(super) fn timestamp_source(&self) -> &'static str {
        if self.sent_at_millis.is_some() { "sender" } else { "received" }
    }
}

pub(super) fn classify<'a>(receiver: &[u8], message: &'a IncomingMessage) -> Option<SmgwText<'a>> {
    if receiver != SMGW_RECEIVER || message.body.len() > MAX_TEXT_BYTES { return None; }
    let text = std::str::from_utf8(&message.body).ok()?;
    // Deliberately narrower than the native raw JSON emitter: unsupported
    // C0/C1/NUL text is kept privately, never lossily changed or silently cut.
    if text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) { return None; }
    let sender = std::str::from_utf8(message.from.as_deref()?).ok()?;
    if sender.is_empty() || sender.len() > MAX_SENDER_BYTES || sender.trim().is_empty()
        || sender.chars().any(char::is_control) { return None; }
    let sent_at_millis = match message.time.as_deref() {
        None => None,
        Some(value) => {
            if value.is_empty() || value.len() > 19 || !value.iter().all(u8::is_ascii_digit) { return None; }
            let value = std::str::from_utf8(value).ok()?.parse::<i64>().ok()?;
            if value > MAX_SENT_MILLIS { return None; }
            DateTime::<Utc>::from_timestamp_millis(value)?;
            Some(value)
        }
    };
    Some(SmgwText { text, sender, sent_at_millis, raw_time: message.time.as_deref() })
}

#[cfg(test)]
#[path = "smgw_text_tests.rs"]
mod tests;
