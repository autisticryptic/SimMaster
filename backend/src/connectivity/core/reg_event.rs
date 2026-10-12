//! RFC 3680 registration-event observation on an existing, access-owned SIP flow.
//!
//! This state machine never registers, resolves addresses, changes security, or
//! extends a REGISTER lease. A binding is captured from the final wire REGISTER;
//! extraction failure means this optional feature is unavailable, not REGISTER
//! failure. All diagnostics deliberately omit SIP identities and credentials.

use std::{collections::HashSet, fmt, time::{Duration, Instant}};

use super::{
    context::{ImsRoute, SipTransport},
    digest_aka::{select_digest_challenge, DigestChallenge},
    registration::RegisteredImsContext,
    sip_message::{build_request, SipHeader, SipRequest},
    ImsError,
};

#[path = "reg_event_xml.rs"]
mod xml;
#[cfg(test)]
#[path = "reg_event_tests.rs"]
mod tests;

const MAX_BODY: usize = 65_536;
const MAX_HEADERS: usize = 16_384;
const REQUESTED_LEASE: u32 = 3_600;
const TRANSACTION_TIMEOUT: Duration = Duration::from_secs(32);
const T1: Duration = Duration::from_millis(500);
const T2: Duration = Duration::from_secs(4);

fn invalid() -> ImsError { ImsError::new("ims_reg_event_invalid") }
fn random_id() -> Result<String, ImsError> {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut bytes = [0u8; 16];
    SystemRandom::new().fill(&mut bytes).map_err(|_| ImsError::new("ims_reg_event_entropy_unavailable"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Only public binding facts, never REGISTER headers, private identity or AKA.
#[derive(Clone, PartialEq, Eq)]
pub struct RegEventBinding {
    aor: String,
    contact_uri: String,
    instance: Option<String>,
    reg_id: Option<u32>,
}

impl fmt::Debug for RegEventBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RegEventBinding([redacted])")
    }
}

impl RegEventBinding {
    pub fn from_register_request(frame: &[u8]) -> Result<Self, ImsError> {
        let frame = Frame::parse(frame).map_err(|_| invalid())?;
        if frame.method() != Some("REGISTER") || !frame.body.is_empty() {
            return Err(invalid());
        }
        let to = address(frame.one("to")?.ok_or_else(invalid)?)?;
        let from = address(frame.one("from")?.ok_or_else(invalid)?)?;
        if to.uri != from.uri { return Err(invalid()); }
        let contact = address(frame.one("contact")?.ok_or_else(invalid)?)?;
        let instance = contact.param("+sip.instance").map(instance_value).transpose()?;
        let reg_id = contact.param("reg-id").map(positive).transpose()?;
        if reg_id.is_some() && instance.is_none() { return Err(invalid()); }
        Ok(Self { aor: to.uri, contact_uri: contact.uri, instance, reg_id })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegEventTerminationReason { Expired, Deactivated, Probation, Rejected, Unregistered }

impl RegEventTerminationReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "reg_event_expired",
            Self::Deactivated => "reg_event_deactivated",
            Self::Probation => "reg_event_probation",
            Self::Rejected => "reg_event_rejected",
            Self::Unregistered => "reg_event_unregistered",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegEventEffect {
    None,
    BindingTerminated { reason: RegEventTerminationReason, retry_after: Option<u32> },
    NeedFull,
}

pub enum RegEventAction { Begin, Retransmit(Vec<u8>), Idle }
impl fmt::Debug for RegEventAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self { Self::Begin => "Begin", Self::Retransmit(_) => "Retransmit([redacted])", Self::Idle => "Idle" })
    }
}

pub enum RegEventFrame {
    Other,
    Notify { response_status: u16, effect: RegEventEffect },
    AuthChallenge { challenge: DigestChallenge },
    Response { status: u16 },
}
impl fmt::Debug for RegEventFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Other => f.write_str("Other"),
            Self::Notify { response_status, effect } => f.debug_struct("Notify").field("response_status", response_status).field("effect", effect).finish(),
            Self::AuthChallenge { .. } => f.write_str("AuthChallenge([redacted])"),
            Self::Response { status } => f.debug_struct("Response").field("status", status).finish(),
        }
    }
}

struct Transaction {
    wire: Vec<u8>,
    branch: String,
    deadline: Instant,
    resend_at: Option<Instant>,
    interval: Duration,
}

/// A bounded observer for exactly one accepted REGISTER binding. Keep it on the
/// channel/card/owner that produced the binding; replace it when that binding
/// changes. Permanent disablement lasts until the adapter replaces the binding.
pub struct RegEventSubscription {
    binding: RegEventBinding,
    call_id: String,
    local_tag: String,
    remote_tag: Option<String>,
    target: String,
    dialog_routes: Option<Vec<String>>,
    cseq: u32,
    transaction: Option<Transaction>,
    auth_pending: Option<bool>,
    auth_rounds: u8,
    auth_deadline: Option<Instant>,
    due: Instant,
    lease_deadline: Option<Instant>,
    requested_lease: u32,
    disabled: bool,
    last_notify: Option<(u32, Vec<u8>)>,
    snapshot: xml::Snapshot,
    full_requests: u8,
    need_full: bool,
    auth_scope: Option<(bool, String, String)>,
}
impl fmt::Debug for RegEventSubscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegEventSubscription").field("disabled", &self.disabled).field("binding", &self.binding).finish_non_exhaustive()
    }
}

impl RegEventSubscription {
    pub fn new(binding: RegEventBinding, now: Instant) -> Result<Self, ImsError> {
        // Existing ring CSPRNG: fail closed on entropy failure; IDs contain no
        // IMSI, public identity, instance, contact address or credentials.
        Ok(Self {
            target: binding.aor.clone(), binding,
            call_id: format!("{}@reg-event.invalid", random_id()?),
            local_tag: random_id()?,
            remote_tag: None, dialog_routes: None, cseq: 0, transaction: None,
            auth_pending: None, auth_rounds: 0, auth_deadline: None, due: now,
            lease_deadline: None, requested_lease: REQUESTED_LEASE,
            disabled: false, last_notify: None, snapshot: xml::Snapshot::default(),
            full_requests: 0, need_full: false, auth_scope: None,
        })
    }

    pub fn request_uri(&self) -> &str { &self.target }
    pub fn is_disabled(&self) -> bool { self.disabled }
    pub fn disable(&mut self) {
        self.disabled = true;
        self.transaction = None;
        self.auth_pending = None;
        self.auth_deadline = None;
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        if self.disabled { return None; }
        let next = if let Some(transaction) = &self.transaction {
            transaction.resend_at.unwrap_or(transaction.deadline).min(transaction.deadline)
        } else if let Some(deadline) = self.auth_deadline {
            deadline
        } else { self.due };
        Some(self.lease_deadline.map_or(next, |expiry| expiry.min(next)))
    }

    pub fn poll(&mut self, now: Instant) -> RegEventAction {
        if self.disabled { return RegEventAction::Idle; }
        if self.lease_deadline.is_some_and(|deadline| now >= deadline)
            || self.auth_deadline.is_some_and(|deadline| now >= deadline)
            || self.transaction.as_ref().is_some_and(|tx| now >= tx.deadline)
        {
            self.disable();
            return RegEventAction::Idle;
        }
        if let Some(tx) = &mut self.transaction {
            if tx.resend_at.is_some_and(|deadline| now >= deadline) {
                tx.interval = (tx.interval * 2).min(T2);
                tx.resend_at = Some(now + tx.interval);
                return RegEventAction::Retransmit(tx.wire.clone());
            }
            return RegEventAction::Idle;
        }
        if self.auth_pending.is_none() && now >= self.due { RegEventAction::Begin } else { RegEventAction::Idle }
    }

    /// Build AND remember the bytes actually sent. Only retransmissions may
    /// reuse authorization. Each challenge continuation must receive a freshly
    /// generated full header from the existing bound UICC helper.
    pub fn build_request(
        &mut self,
        route: &ImsRoute,
        registration: &RegisteredImsContext,
        user_agent: &str,
        access_headers: &[SipHeader],
        authorization: Option<&str>,
        now: Instant,
    ) -> Result<Vec<u8>, ImsError> {
        if self.disabled || self.transaction.is_some()
            || self.lease_deadline.is_some_and(|deadline| now >= deadline)
            || self.auth_deadline.is_some_and(|deadline| now >= deadline)
        { return Err(invalid()); }
        if self.auth_pending.is_none() && now < self.due { return Err(invalid()); }
        let auth = match (self.auth_pending, authorization) {
            (None, None) => None,
            (Some(proxy), Some(value)) => {
                let value = value.trim_end_matches("\r\n");
                let (name, value) = value.split_once(':').ok_or_else(invalid)?;
                if !name.eq_ignore_ascii_case(if proxy { "Proxy-Authorization" } else { "Authorization" })
                    || !safe_header_value(value) || !value.trim_start().starts_with("Digest ")
                { return Err(invalid()); }
                Some(SipHeader::new(name, value.trim()))
            }
            _ => return Err(invalid()),
        };
        if !safe_header_value(user_agent) || access_headers.len() > 32 { return Err(invalid()); }
        let mut headers = Vec::new();
        if let Some(routes) = &self.dialog_routes {
            for route in routes { headers.push(SipHeader::new("Route", route)); }
        } else if let Some(routes) = &registration.service_route {
            for route in route_list(routes)? { headers.push(SipHeader::new("Route", route)); }
        } else {
            headers.push(SipHeader::new("Route", format!("<sip:{}:{};lr>", super::sip_frame::sip_host(route.pcscf_addr.ip()), route.pcscf_addr.port())));
        }
        self.requested_lease = REQUESTED_LEASE.min(registration.lease.expires_seconds);
        if self.requested_lease == 0 { return Err(invalid()); }
        headers.extend([
            SipHeader::new("Contact", format!("<{}>", self.binding.contact_uri)),
            SipHeader::new("Event", "reg"),
            SipHeader::new("Accept", "application/reginfo+xml"),
            SipHeader::new("Expires", self.requested_lease.to_string()),
            SipHeader::new("User-Agent", user_agent),
        ]);
        let mut names = HashSet::new();
        for header in access_headers {
            let name = header.name.to_ascii_lowercase();
            if !token(&name) || !safe_header_value(&header.value)
                || !names.insert(name.clone())
                || matches!(name.as_str(), "via" | "v" | "from" | "f" | "to" | "t" | "call-id" | "i" | "cseq" | "contact" | "m" | "event" | "o" | "accept" | "expires" | "content-length" | "l" | "content-type" | "c" | "route" | "record-route" | "authorization" | "proxy-authorization" | "user-agent" | "max-forwards")
            { return Err(invalid()); }
            headers.push(header.clone());
        }
        if let Some(auth) = auth { headers.push(auth); }
        let cseq = self.cseq.checked_add(1).filter(|n| *n < 0x8000_0000).ok_or_else(invalid)?;
        let branch = format!("z9hG4bK{}", random_id()?);
        let mut to = format!("<{}>", self.binding.aor);
        if let Some(tag) = &self.remote_tag { to.push_str(&format!(";tag={tag}")); }
        let wire = build_request(&SipRequest {
            method: "SUBSCRIBE", request_uri: &self.target, route: *route,
            branch: &branch, from_uri: &self.binding.aor, from_tag: &self.local_tag,
            to_value: &to, call_id: &self.call_id, cseq, headers: &headers, body: &[],
        });
        if wire.len() > MAX_HEADERS { return Err(invalid()); }
        // Budget is reset only for a new unauthenticated transaction, not for a
        // challenge continuation or byte-identical resend.
        if self.auth_pending.is_none() {
            self.auth_rounds = 0;
            self.auth_scope = None;
            self.need_full = false;
        }
        self.auth_pending = None;
        self.auth_deadline = None;
        self.cseq = cseq;
        self.transaction = Some(Transaction {
            wire: wire.clone(), branch, deadline: now + TRANSACTION_TIMEOUT,
            resend_at: (route.transport == SipTransport::Udp).then_some(now + T1), interval: T1,
        });
        Ok(wire)
    }

    /// Read-only dispatch before adapter ownership I/O. Unrelated SIP traffic
    /// must not pay for card checks or be delayed by this optional observer.
    pub fn wants_frame(&self, wire: &[u8]) -> bool {
        is_reg_notify(wire) || Frame::parse(wire).is_ok_and(|frame|
            frame.one("call-id").ok().flatten() == Some(self.call_id.as_str()))
    }

    pub fn on_frame(&mut self, wire: &[u8], now: Instant) -> RegEventFrame {
        // Lightweight bounded classification ensures malformed reg NOTIFY is
        // rejected here, while unrelated MWI/REFER remains with its own parser.
        let notify = wire.starts_with(b"NOTIFY ");
        let reg = is_reg_notify(wire);
        let parsed = match Frame::parse(wire) {
            Ok(frame) => frame,
            Err(_) if notify && reg => return notify_result(400, RegEventEffect::None),
            Err(_) => return RegEventFrame::Other,
        };
        if notify {
            let our_id = parsed.one("call-id").ok().flatten() == Some(self.call_id.as_str());
            if !reg && !(our_id && parsed.one("event").ok().flatten().is_none()) { return RegEventFrame::Other; }
            return match self.notify(&parsed, wire, now) {
                Ok(effect) => notify_result(200, effect),
                Err(status) => notify_result(status, RegEventEffect::None),
            };
        }
        if parsed.status().is_some() { self.response(&parsed, now) } else { RegEventFrame::Other }
    }

    fn response(&mut self, frame: &Frame<'_>, now: Instant) -> RegEventFrame {
        let Some(tx) = self.transaction.as_ref() else { return RegEventFrame::Other; };
        let matches = (|| -> Result<bool, ImsError> {
            let from = address(frame.one("from")?.ok_or_else(invalid)?)?;
            let to = address(frame.one("to")?.ok_or_else(invalid)?)?;
            let via = frame.one("via")?.ok_or_else(invalid)?;
            let (_, params) = parameters(via)?;
            let branch = params.iter().find(|(n, _)| n == "branch").and_then(|(_, v)| v.as_deref());
            Ok(frame.one("call-id")? == Some(self.call_id.as_str())
                && frame.cseq()? == (self.cseq, "SUBSCRIBE")
                && from.uri == self.binding.aor && from.param("tag") == Some(self.local_tag.as_str())
                && to.uri == self.binding.aor && branch == Some(tx.branch.as_str()))
        })().unwrap_or(false);
        if !matches { return RegEventFrame::Other; }
        let status = frame.status().unwrap_or(0);
        if now >= tx.deadline || self.disabled {
            self.disable();
            return RegEventFrame::Response { status };
        }
        if (100..200).contains(&status) {
            // Non-INVITE timer E continues at T2 in proceeding state.
            if let Some(tx) = self.transaction.as_mut() {
                tx.interval = T2;
                if tx.resend_at.is_some() { tx.resend_at = Some(now + T2); }
            }
            return RegEventFrame::Response { status };
        }
        if status == 401 || status == 407 {
            let challenge = (|| -> Result<DigestChallenge, ImsError> {
                if self.auth_rounds >= 2 || frame.one("security-server")?.is_some()
                    || frame.one("security-verify")?.is_some()
                { return Err(invalid()); }
                let wanted = if status == 401 { "www-authenticate" } else { "proxy-authenticate" };
                let other = if status == 401 { "proxy-authenticate" } else { "www-authenticate" };
                if frame.one(other)?.is_some() { return Err(invalid()); }
                let value = frame.one(wanted)?.ok_or_else(invalid)?.to_string();
                if super::digest_aka::split_digest_challenge_values(&value).len() != 1 { return Err(invalid()); }
                // One scope only: no implicit origin+proxy credential replay.
                if self.auth_pending.is_some() { return Err(invalid()); }
                let values = [value];
                let challenge = if status == 401 { select_digest_challenge(&values, &[], false)? }
                    else { select_digest_challenge(&[], &values, false)? };
                if self.auth_scope.as_ref().is_some_and(|(proxy, realm, algorithm)| {
                    *proxy != challenge.proxy || realm != &challenge.realm || !algorithm.eq_ignore_ascii_case(&challenge.algorithm)
                }) { return Err(invalid()); }
                Ok(challenge)
            })();
            if let Ok(challenge) = challenge {
                self.transaction = None;
                self.auth_rounds += 1;
                self.auth_scope = Some((challenge.proxy, challenge.realm.clone(), challenge.algorithm.clone()));
                self.auth_pending = Some(challenge.proxy);
                self.auth_deadline = Some(now + TRANSACTION_TIMEOUT);
                return RegEventFrame::AuthChallenge { challenge };
            }
            self.disable();
            return RegEventFrame::Response { status };
        }
        if status == 200 || status == 202 {
            let accepted = (|| -> Result<(String, String, Vec<String>, u32), ImsError> {
                let to = address(frame.one("to")?.ok_or_else(invalid)?)?;
                let tag = valid_tag(to.param("tag").ok_or_else(invalid)?)?.to_string();
                if self.remote_tag.as_ref().is_some_and(|old| old != &tag) { return Err(invalid()); }
                let target = address(frame.one("contact")?.ok_or_else(invalid)?)?.uri;
                let routes = frame.routes(true)?;
                if self.dialog_routes.as_ref().is_some_and(|old| !routes.is_empty() && old != &routes) { return Err(invalid()); }
                let expires = positive(frame.one("expires")?.ok_or_else(invalid)?)?.min(self.requested_lease);
                Ok((tag, target, routes, expires))
            })();
            match accepted {
                Ok((tag, target, routes, expires)) => {
                    self.remote_tag = Some(tag);
                    self.target = target;
                    if self.dialog_routes.is_none() { self.dialog_routes = Some(routes); }
                    self.transaction = None;
                    self.lease_deadline = Some(now + Duration::from_secs(u64::from(expires)));
                    self.due = if self.need_full { now } else { now + Duration::from_secs(u64::from((expires / 2).max(1))) };
                }
                Err(_) => self.disable(),
            }
        } else { self.disable(); }
        RegEventFrame::Response { status }
    }

    fn notify(&mut self, frame: &Frame<'_>, wire: &[u8], now: Instant) -> Result<RegEventEffect, u16> {
        let map = |_: ImsError| 400u16;
        let event = frame.one("event").map_err(map)?.ok_or(489u16)?;
        let (event, params) = parameters(event).map_err(map)?;
        // We never send an Event id; an id denotes a different subscription.
        if !event.eq_ignore_ascii_case("reg") || !params.is_empty() { return Err(489); }
        if frame.one("call-id").map_err(map)? != Some(self.call_id.as_str()) { return Err(481); }
        let from = address(frame.one("from").map_err(map)?.ok_or(400u16)?).map_err(map)?;
        let to = address(frame.one("to").map_err(map)?.ok_or(400u16)?).map_err(map)?;
        let remote = valid_tag(from.param("tag").ok_or(481u16)?).map_err(map)?;
        if to.uri != self.binding.aor || from.uri != self.binding.aor
            || to.param("tag") != Some(self.local_tag.as_str())
            || self.remote_tag.as_deref().is_some_and(|tag| tag != remote)
            || frame.request_uri().and_then(|uri| canonical_uri(uri).ok()).as_deref() != Some(self.binding.contact_uri.as_str())
        { return Err(481); }
        let (cseq, method) = frame.cseq().map_err(map)?;
        if method != "NOTIFY" { return Err(400); }
        if let Some((last, bytes)) = &self.last_notify {
            if cseq == *last { return if wire == bytes { Ok(RegEventEffect::None) } else { Err(400) }; }
            if cseq < *last { return Err(500); }
        }
        if self.disabled || self.lease_deadline.is_some_and(|deadline| now >= deadline)
            || (self.remote_tag.is_none() && (self.transaction.is_none()
                || self.transaction.as_ref().is_some_and(|tx| now >= tx.deadline)))
        { return Err(481); }
        let subscription = subscription_state(frame.one("subscription-state").map_err(map)?.ok_or(400u16)?).map_err(map)?;
        let content_type = frame.one("content-type").map_err(map)?;
        if frame.one("require").map_err(map)?.is_some() { return Err(420); }
        let via = frame.headers.iter().find(|(name, _)| name == "via").map(|(_, v)| v.as_str()).ok_or(400u16)?;
        let (sent_by, via_params) = parameters(via.split(',').next().ok_or(400u16)?).map_err(map)?;
        if !(sent_by.starts_with("SIP/2.0/UDP ") || sent_by.starts_with("SIP/2.0/TCP "))
            || !via_params.iter().any(|(name, value)| name == "branch" && value.as_deref().is_some_and(token))
        { return Err(400); }
        let document = if frame.body.is_empty() && subscription.terminated {
            None
        } else {
            let (media, params) = parameters(content_type.ok_or(415u16)?).map_err(|_| 415u16)?;
            if !media.eq_ignore_ascii_case("application/reginfo+xml") || params.iter().any(|(name, value)| {
                name != "charset" || !value.as_deref().is_some_and(|value| value.trim_matches('"').eq_ignore_ascii_case("utf-8"))
            }) { return Err(415); }
            Some(xml::parse(frame.body).map_err(map)?)
        };
        let contact = address(frame.one("contact").map_err(map)?.ok_or(400u16)?).map_err(map)?;
        let routes = frame.routes(false).map_err(map)?;
        if self.dialog_routes.as_ref().is_some_and(|old| !routes.is_empty() && old != &routes) { return Err(481); }
        // Work on a copy. No tag/target/CSeq/version may be pinned by malformed
        // or semantically ambiguous input (including an early NOTIFY race).
        let mut snapshot = self.snapshot.clone();
        let effect = match document {
            Some(document) => snapshot.apply(&document, &self.binding).map_err(map)?,
            None => RegEventEffect::None,
        };
        let effect = match effect {
            RegEventEffect::BindingTerminated { reason, retry_after } => RegEventEffect::BindingTerminated {
                reason, retry_after: retry_after.into_iter().chain(subscription.retry_after).max(),
            },
            other => other,
        };
        self.snapshot = snapshot;
        self.remote_tag = Some(remote.to_string());
        self.target = contact.uri;
        if self.dialog_routes.is_none() { self.dialog_routes = Some(routes); }
        self.last_notify = Some((cseq, wire.to_vec()));
        if let Some(expires) = subscription.expires {
            let deadline = now + Duration::from_secs(u64::from(expires.min(self.requested_lease)));
            // NOTIFY can shorten a subscription but never renew it on its own.
            self.lease_deadline = Some(self.lease_deadline.map_or(deadline, |old| old.min(deadline)));
            let renewal = now + Duration::from_secs(u64::from((expires.min(self.requested_lease) / 2).max(1)));
            if self.transaction.is_none() { self.due = self.due.min(renewal); }
        }
        if matches!(effect, RegEventEffect::NeedFull) {
            self.need_full = true;
            self.full_requests += 1;
            if self.full_requests > 1 { self.disable(); }
            else { self.due = now; }
        }
        if subscription.terminated || matches!(effect, RegEventEffect::BindingTerminated { .. }) { self.disable(); }
        Ok(effect)
    }
}

/// Classify reg NOTIFY when an adapter has no proven binding/subscription.
/// `None` leaves MWI/REFER untouched; a status is sent using the original request.
/// Never construct a guessed binding merely to reject an unsolicited NOTIFY.
pub fn classify_unsubscribed_notify(wire: &[u8]) -> Option<u16> {
    if !is_reg_notify(wire) { return None; }
    let valid = (|| -> Result<(), ImsError> {
        let frame = Frame::parse(wire)?;
        let (event, params) = parameters(frame.one("event")?.ok_or_else(invalid)?)?;
        if !event.eq_ignore_ascii_case("reg") || !params.is_empty() { return Err(invalid()); }
        Ok(())
    })();
    Some(if valid.is_ok() { 481 } else { 400 })
}

fn is_reg_notify(wire: &[u8]) -> bool {
    if let Ok(frame) = Frame::parse(wire) {
        return frame.method() == Some("NOTIFY") && frame.headers.iter().any(|(name, value)|
            name == "event" && value.split([';', ',']).next().is_some_and(|event| event.trim().eq_ignore_ascii_case("reg")));
    }
    let head = &wire[..wire.len().min(MAX_HEADERS)];
    let end = head.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(head.len());
    let text = std::str::from_utf8(&head[..end]).unwrap_or("");
    text.starts_with("NOTIFY ") && text.split("\r\n").any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            (name.trim_end().eq_ignore_ascii_case("event") || name.trim_end().eq_ignore_ascii_case("o"))
                && value.trim().split([';', ',']).next().is_some_and(|v| v.trim().eq_ignore_ascii_case("reg"))
        })
    })
}

fn notify_result(response_status: u16, effect: RegEventEffect) -> RegEventFrame {
    RegEventFrame::Notify { response_status, effect }
}

struct SubscriptionState { terminated: bool, expires: Option<u32>, retry_after: Option<u32> }
fn subscription_state(value: &str) -> Result<SubscriptionState, ImsError> {
    let (state, params) = parameters(value)?;
    let terminated = match state.to_ascii_lowercase().as_str() {
        "active" | "pending" => false, "terminated" => true, _ => return Err(invalid()),
    };
    let mut expires = None;
    let mut retry_after = None;
    for (name, value) in params {
        let value = value.ok_or_else(invalid)?;
        match name.as_str() {
            "expires" if !terminated => expires = Some(positive(&value)?),
            "retry-after" if terminated => retry_after = Some(unsigned(&value)?),
            "reason" if terminated && matches!(value.to_ascii_lowercase().as_str(), "deactivated" | "probation" | "rejected" | "timeout" | "giveup" | "noresource" | "invariant") => {}
            _ => return Err(invalid()),
        }
    }
    if !terminated && expires.is_none() { return Err(invalid()); }
    Ok(SubscriptionState { terminated, expires, retry_after })
}

fn safe_header_value(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && !value.bytes().any(|b| b < 32 || b == 127)
}
fn token(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.!%*_+`'~".contains(&b))
}
fn valid_tag(value: &str) -> Result<&str, ImsError> {
    if value.len() > 256 || !token(value) { return Err(invalid()); }
    Ok(value)
}
fn unsigned(value: &str) -> Result<u32, ImsError> {
    if value.is_empty() || value.len() > 10 || !value.bytes().all(|b| b.is_ascii_digit()) { return Err(invalid()); }
    value.parse().map_err(|_| invalid())
}
fn positive(value: &str) -> Result<u32, ImsError> {
    let value = unsigned(value)?;
    if value == 0 { return Err(invalid()); }
    Ok(value)
}

/// Intentionally conservative URI matching: fold scheme/host only, retaining
/// case-sensitive user, explicit port, every URI parameter and parameter order.
/// Unsupported/custom syntax is unavailable rather than a guessed identity.
fn canonical_uri(value: &str) -> Result<String, ImsError> {
    if value.is_empty() || value.len() > 2048 || !value.is_ascii()
        || value.bytes().any(|b| b <= 32 || b == 127 || b"<>\"\\,?".contains(&b))
    { return Err(invalid()); }
    let (scheme, rest) = value.split_once(':').ok_or_else(invalid)?;
    if !scheme.eq_ignore_ascii_case("sip") { return Err(invalid()); }
    let (authority, suffix) = rest.split_once(';').map_or((rest, ""), |(a, b)| (a, b));
    let (user, host_port) = authority.rsplit_once('@').map_or((None, authority), |(u, h)| (Some(u), h));
    if let Some(user) = user {
        if user.is_empty() || user.contains([':', '@']) { return Err(invalid()); }
    }
    let (host, port) = if host_port.starts_with('[') {
        let end = host_port.find(']').ok_or_else(invalid)?;
        host_port[1..end].parse::<std::net::Ipv6Addr>().map_err(|_| invalid())?;
        let tail = &host_port[end + 1..];
        (&host_port[..=end], if tail.is_empty() { None } else { Some(tail.strip_prefix(':').ok_or_else(invalid)?) })
    } else {
        let (host, port) = host_port.split_once(':').map_or((host_port, None), |(h, p)| (h, Some(p)));
        if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b)) { return Err(invalid()); }
        (host, port)
    };
    if let Some(port) = port {
        if positive(port)? > u16::MAX as u32 { return Err(invalid()); }
    }
    let bytes = value.as_bytes();
    for (i, byte) in bytes.iter().enumerate() {
        if *byte == b'%' && (i + 2 >= bytes.len() || !bytes[i + 1].is_ascii_hexdigit() || !bytes[i + 2].is_ascii_hexdigit()) { return Err(invalid()); }
    }
    if !suffix.is_empty() {
        let mut seen = HashSet::new();
        for param in suffix.split(';') {
            let (name, value) = param.split_once('=').map_or((param, None), |(n, v)| (n, Some(v)));
            if !token(name) || !seen.insert(name.to_ascii_lowercase()) || value.is_some_and(str::is_empty) { return Err(invalid()); }
        }
    } else if rest.ends_with(';') { return Err(invalid()); }
    Ok(format!("sip:{}{}{}{}", user.map_or(String::new(), |u| format!("{u}@")), host.to_ascii_lowercase(), port.map_or(String::new(), |p| format!(":{p}")), if suffix.is_empty() { String::new() } else { format!(";{suffix}") }))
}

struct Address { uri: String, params: Vec<(String, Option<String>)> }
impl Address {
    fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(n, _)| n == name).and_then(|(_, v)| v.as_deref())
    }
}
fn address(value: &str) -> Result<Address, ImsError> {
    if !safe_header_value(value) { return Err(invalid()); }
    let value = value.trim();
    let (uri, tail) = if let Some(open) = value.find('<') {
        let display = value[..open].trim();
        if display.contains([',', '<', '>']) { return Err(invalid()); }
        let close = value[open + 1..].find('>').map(|n| n + open + 1).ok_or_else(invalid)?;
        (&value[open + 1..close], &value[close + 1..])
    } else {
        // A bare Contact/AoR with parameters is ambiguous (URI vs header
        // parameters). Do not guess how a custom REGISTER builder intended it.
        if value.contains(';') { return Err(invalid()); }
        (value, "")
    };
    if tail.contains(',') || (!tail.trim().is_empty() && !tail.trim_start().starts_with(';')) { return Err(invalid()); }
    let (_, params) = parameters(&format!("address{}", tail.trim()))?;
    Ok(Address { uri: canonical_uri(uri)?, params })
}

fn instance_value(value: &str) -> Result<String, ImsError> {
    let value = value.trim();
    let value = if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 { &value[1..value.len() - 1] } else { value };
    if value.len() > 256 || !value.starts_with('<') || !value.ends_with('>')
        || !value[1..value.len() - 1].starts_with("urn:")
        || value[1..value.len() - 1].bytes().any(|b| b <= 32 || b >= 127 || b"<>\"\\".contains(&b))
    { return Err(invalid()); }
    Ok(value.to_string())
}

/// Strict quoted parameter splitting with uniqueness, no escaped delimiters.
fn parameters(value: &str) -> Result<(String, Vec<(String, Option<String>)>), ImsError> {
    if !safe_header_value(value) { return Err(invalid()); }
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote = false;
    for (i, b) in value.bytes().enumerate() {
        if b == b'\\' { return Err(invalid()); }
        if b == b'"' { quote = !quote; }
        if b == b';' && !quote { parts.push(&value[start..i]); start = i + 1; }
    }
    if quote { return Err(invalid()); }
    parts.push(&value[start..]);
    let first = parts.remove(0).trim().to_string();
    let mut params = Vec::new();
    let mut names = HashSet::new();
    if parts.len() > 32 { return Err(invalid()); }
    for part in parts {
        let (name, value) = part.trim().split_once('=').map_or((part.trim(), None), |(n, v)| (n.trim(), Some(v.trim().to_string())));
        if !token(name) || !names.insert(name.to_ascii_lowercase()) || value.as_ref().is_some_and(|v| v.is_empty()) { return Err(invalid()); }
        params.push((name.to_ascii_lowercase(), value));
    }
    Ok((first, params))
}

fn route_list(value: &str) -> Result<Vec<String>, ImsError> {
    if !safe_header_value(value) { return Err(invalid()); }
    let mut routes = Vec::new();
    for entry in value.split(',') {
        let parsed = address(entry.trim())?;
        if !parsed.params.is_empty() || !parsed.uri.split(';').skip(1).any(|p| p.eq_ignore_ascii_case("lr")) { return Err(invalid()); }
        routes.push(format!("<{}>", parsed.uri));
        if routes.len() > 16 { return Err(invalid()); }
    }
    Ok(routes)
}

struct Frame<'a> { start: &'a str, headers: Vec<(String, String)>, body: &'a [u8] }
impl<'a> Frame<'a> {
    fn parse(wire: &'a [u8]) -> Result<Self, ImsError> {
        if wire.len() > MAX_HEADERS + MAX_BODY { return Err(invalid()); }
        let end = wire.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(invalid)?;
        if end > MAX_HEADERS { return Err(invalid()); }
        let text = std::str::from_utf8(&wire[..end]).map_err(|_| invalid())?;
        let mut lines = text.split("\r\n");
        let start = lines.next().ok_or_else(invalid)?;
        if !safe_header_value(start) { return Err(invalid()); }
        let mut headers: Vec<(String, String)> = Vec::new();
        for (physical_line, line) in lines.enumerate() {
            if physical_line >= 128 || line.bytes().any(|b| (b < 32 && b != b'\t') || b == 127) { return Err(invalid()); }
            if line.starts_with([' ', '\t']) {
                let (_, value) = headers.last_mut().ok_or_else(invalid)?;
                value.push(' ');
                value.push_str(line.trim());
                if value.len() > 4096 { return Err(invalid()); }
                continue;
            }
            let (name, value) = line.split_once(':').ok_or_else(invalid)?;
            let name = name.trim_end_matches([' ', '\t']);
            if !token(name) || value.len() > 4096 { return Err(invalid()); }
            let name = match name.to_ascii_lowercase().as_str() {
                "f" => "from".into(), "t" => "to".into(), "i" => "call-id".into(),
                "v" => "via".into(), "o" => "event".into(), "l" => "content-length".into(),
                "c" => "content-type".into(), "m" => "contact".into(), other => other.to_string(),
            };
            headers.push((name, value.trim().to_owned()));
            if headers.len() > 128 { return Err(invalid()); }
        }
        let frame = Self { start, headers, body: &wire[end + 4..] };
        let length = unsigned(frame.one("content-length")?.ok_or_else(invalid)?)? as usize;
        if length != frame.body.len() || length > MAX_BODY { return Err(invalid()); }
        Ok(frame)
    }
    fn one(&self, name: &str) -> Result<Option<&str>, ImsError> {
        let mut values = self.headers.iter().filter(|(n, _)| n == name).map(|(_, v)| v.as_str());
        let value = values.next();
        if values.next().is_some() { return Err(invalid()); }
        Ok(value)
    }
    fn method(&self) -> Option<&str> {
        let mut words = self.start.split(' ');
        let method = words.next()?;
        words.next()?;
        (words.next()? == "SIP/2.0" && words.next().is_none()).then_some(method)
    }
    fn request_uri(&self) -> Option<&str> { self.method()?; self.start.split(' ').nth(1) }
    fn status(&self) -> Option<u16> {
        let rest = self.start.strip_prefix("SIP/2.0 ")?;
        let (code, reason) = rest.split_once(' ')?;
        if code.len() != 3 || reason.is_empty() { return None; }
        let code = unsigned(code).ok()?;
        (100..=699).contains(&code).then_some(code as u16)
    }
    fn cseq(&self) -> Result<(u32, &str), ImsError> {
        let value = self.one("cseq")?.ok_or_else(invalid)?;
        let mut parts = value.split_ascii_whitespace();
        let number = unsigned(parts.next().ok_or_else(invalid)?)?;
        let method = parts.next().ok_or_else(invalid)?;
        if parts.next().is_some() || number >= 0x8000_0000 || !token(method) { return Err(invalid()); }
        Ok((number, method))
    }
    fn routes(&self, reverse: bool) -> Result<Vec<String>, ImsError> {
        let mut routes = Vec::new();
        for (_, value) in self.headers.iter().filter(|(n, _)| n == "record-route") {
            routes.extend(route_list(value)?);
            if routes.len() > 16 { return Err(invalid()); }
        }
        if reverse { routes.reverse(); }
        Ok(routes)
    }
}
