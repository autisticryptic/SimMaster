//! RFC 5626 client flow maintenance, shared by the two IMS access legs.
//!
//! A REGISTER lease and a transport flow are different lifetimes. Only a
//! successful outbound negotiation AND a live flow permit another registration.
//! NATed/unknown paths require acknowledged keepalives. IMS UDP paths positively
//! observed without NAT may omit them unless Flow-Timer requests them (TS 24.229
//! K.2.1.5). The coordinator holds a Weak lease, never a stale
//! "last 200 OK" as authorization. UDP probes use the actual SIP client socket;
//! CRLF probes use the registered TCP stream. Neither is a SIP refresh or an
//! instruction to renegotiate IPsec.

use super::{
    ims_access::ImsAccess,
    ims_registration_coordinator::{self, ImsRegistrationCoordinator},
    register::RegisterTransactionKey,
    register_response::{name_addr_range, split_sip_list, split_sip_parameters, RegisterArtifacts},
    sip_frame, ImsError,
};
use ring::rand::{SecureRandom, SystemRandom};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const COOKIE: [u8; 4] = [0x21, 0x12, 0xa4, 0x42];
const FINGERPRINT_XOR: u32 = 0x5354_554e;
const PONG_TIMEOUT: Duration = Duration::from_secs(10);
// RFC 5626 4.4.2 bounds flow failure detection at 16 RTO. Probes are
// retransmitted seven times within that window, not for another 127 seconds.
const STUN_RTO: Duration = Duration::from_millis(500);
const STUN_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug)]
struct LeaseState {
    expires: Instant,
    negotiated: bool,
    healthy: bool,
    healthy_until: Instant,
    failed: bool,
}

/// Not serializable: instance identity and live flow handles are internal.
#[derive(Debug)]
pub struct FlowLease {
    pub instance: String,
    state: Mutex<LeaseState>,
}

impl FlowLease {
    pub fn proven(&self) -> bool {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.negotiated
            && s.healthy
            && !s.failed
            && Instant::now() < s.expires
            && Instant::now() < s.healthy_until
    }
    pub fn invalidate(&self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.failed = true;
        s.healthy = false;
    }
    pub fn live(&self) -> bool {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        !s.failed && Instant::now() < s.expires
    }
    fn acknowledge(&self, healthy_until: Instant) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.healthy = true;
        s.healthy_until = healthy_until;
    }
    fn negotiated(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .negotiated
    }
    fn failed(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).failed
    }
}

#[derive(Clone)]
struct Offer {
    coordinator: Arc<ImsRegistrationCoordinator>,
    access: ImsAccess,
    instance: String,
    enabled: bool,
}

struct PendingProbe {
    id: [u8; 12],
    packet: Vec<u8>,
    deadline: Instant,
    retransmits: u8,
    timeout_at: Instant,
}

#[derive(Default)]
pub struct OutboundFlow {
    offer: Option<Offer>,
    last_request: Option<Vec<u8>>,
    last_source: Option<Vec<u8>>,
    lease: Option<Arc<FlowLease>>,
    tcp: bool,
    timer: Option<u32>,
    next_probe: Option<Instant>,
    pending: Option<PendingProbe>,
    mapped: Option<SocketAddr>,
    failure_reported: bool,
    no_nat: bool,
}

impl OutboundFlow {
    pub fn configure(&mut self, line_id: &str, access: ImsAccess, instance: &str, enabled: bool) {
        self.offer = Some(Offer {
            coordinator: ims_registration_coordinator::for_line(line_id),
            access,
            instance: instance.to_string(),
            enabled,
        });
    }

    /// Apply only to REGISTER, including authenticated retries and unregister.
    /// The source request's transaction identity and digest URI are unchanged.
    /// Adding the second flow requires outbound, so a compliant legacy registrar
    /// must reject it rather than silently replace the first binding.
    pub fn prepare(&mut self, frame: &[u8]) -> Result<Vec<u8>, ImsError> {
        if !sip_frame::is_request(frame, "REGISTER") {
            return Ok(frame.to_vec());
        }
        let Some(offer) = &self.offer else {
            return Ok(frame.to_vec());
        };
        // Retransmit the exact bytes from this transaction, including the
        // original capability offer. A policy tick must not mutate Timer E.
        if self.last_source.as_deref() == Some(frame) {
            if let Some(prepared) = &self.last_request {
                return Ok(prepared.clone());
            }
        }
        let bound_outbound = self.lease.as_ref().is_some_and(|lease| lease.negotiated());
        // Once bound, refresh/remove this binding using its original outbound
        // identity even if a capability probe on the other access was refused.
        let required = bound_outbound
            || offer
                .coordinator
                .additional_flow_requires_outbound(offer.access);
        let enabled =
            bound_outbound || (offer.enabled && offer.coordinator.may_offer_outbound(offer.access));
        if required && (!enabled || !offer.coordinator.instance_matches(&offer.instance)) {
            offer.coordinator.reject_outbound(offer.access);
            return Err(ImsError::new("ims_outbound_additional_flow_not_supported"));
        }
        let source = frame.to_vec();
        let frame = if enabled {
            offer_register(frame, &offer.instance, offer.access.reg_id(), required)?
        } else {
            legacy_register(frame)?
        };
        // Log the FINAL frame, not the adapter's pre-rewrite template. In
        // particular the authenticated request can take a different channel.
        // No Contact/IMSI, Call-ID, digest or security material is logged.
        tracing::info!(
            access = offer.access.as_str(),
            supported_outbound = has_option(&frame, "Supported", "outbound"),
            supported_path = has_option(&frame, "Supported", "path"),
            require_outbound = has_option(&frame, "Require", "outbound"),
            cseq = ?sip_frame::header_value(&frame, "CSeq"),
            authorization_present = !sip_frame::header_values(&frame, "Authorization").is_empty(),
            security_verify_present = !sip_frame::header_values(&frame, "Security-Verify").is_empty(),
            reg_id = offer.access.reg_id(),
            "IMS REGISTER outbound offer prepared"
        );
        self.last_source = Some(source);
        self.last_request = Some(frame.clone());
        Ok(frame)
    }

    /// Inspect only a final response to OUR current REGISTER. A refusal of
    /// outbound must not enter the generic header/security candidate ladder.
    pub fn received_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        let (Some(offer), Some(request)) = (&self.offer, &self.last_request) else {
            return Ok(());
        };
        if !RegisterTransactionKey::from_register_request(request)
            .is_some_and(|key| key.matches_response(frame))
        {
            return Ok(());
        }
        let status = sip_frame::parse_status(frame).ok();
        // Determine NAT only on the initial, unprotected tuple. Protected
        // UDP advertises port_us but sends from port_uc: comparing those ports
        // would incorrectly classify every IMS AKA flow as NATed.
        if matches!(status, Some(200 | 401 | 407))
            && sip_frame::header_values(request, "Security-Verify").is_empty()
        {
            self.no_nat = initial_response_confirms_no_nat(request, frame);
        }
        if status == Some(439)
            || (status == Some(420) && has_option(frame, "Unsupported", "outbound"))
        {
            offer.coordinator.reject_outbound(offer.access);
            return Err(ImsError::new("ims_outbound_rejected"));
        }
        Ok(())
    }

    /// A security challenge can stage a new transport while retaining the old
    /// flow for rollback. Never send the old flow's STUN on the tentative tuple.
    pub fn replacement(&self) -> Self {
        Self {
            offer: self.offer.clone(),
            no_nat: self.no_nat,
            ..Self::default()
        }
    }

    /// Security negotiation replaces sockets, not the IP-CAN path. Preserve
    /// only the initial NAT observation, never the old lease or probe state.
    pub fn inherit_path_observation(&mut self, previous: &Self) {
        self.no_nat = previous.no_nat;
    }

    /// Called only after the REGISTER driver has accepted its matching 200.
    /// Absence of Path means the next hop is the registrar. With Path, RFC
    /// 5626 4.3/4.4.2 requires the LAST Path URI's ob parameter (the first hop).
    pub fn registered(
        &mut self,
        response: &[u8],
        tcp: bool,
        default_expires: u32,
    ) -> Result<RegisterArtifacts, ImsError> {
        let Some(offer) = &self.offer else {
            return Ok(RegisterArtifacts::parse(response));
        };
        let Some(request) = &self.last_request else {
            return Ok(RegisterArtifacts::parse(response));
        };
        if !RegisterTransactionKey::from_register_request(request)
            .is_some_and(|key| key.matches_response(response))
            || sip_frame::parse_status(response).ok() != Some(200)
        {
            return Err(ImsError::new("ims_outbound_register_response_mismatch"));
        }
        let offered = has_option(request, "Supported", "outbound");
        // A legacy registrar may ignore reg-id and omit it from the returned
        // Contact. Keep its real Contact expiry for a single registration;
        // binding-specific parsing is mandatory only when outbound was
        // accepted/required, never borrow the other access's lifetime.
        let outbound_required = has_option(request, "Require", "outbound")
            || has_option(response, "Require", "outbound");
        let mut artifacts = RegisterArtifacts::parse(response);
        if offered {
            let binding = RegisterArtifacts::parse_for_binding(
                response,
                &offer.instance,
                offer.access.reg_id(),
            );
            if outbound_required {
                artifacts = binding;
            } else {
                // Some peers echo instance/reg-id without negotiating outbound.
                // Report that distinction without borrowing another Contact's
                // expiry or treating an echo as permission for the second flow.
                artifacts.own_binding_found = binding.own_binding_found;
            }
        }
        offer.coordinator.observe_response(offer.access, &artifacts);
        let negotiated = offered
            && artifacts.outbound_required
            && artifacts.first_hop_outbound
            && artifacts.own_binding_found;
        if (has_option(request, "Require", "outbound") || artifacts.outbound_required)
            && !negotiated
        {
            // A broken registrar ignored Require and may have replaced the
            // primary Contact. Neither old flow may remain marked valid.
            offer.coordinator.reject_outbound(offer.access);
            offer.coordinator.invalidate_outbound_flows();
            return Err(ImsError::new("ims_outbound_negotiation_lost"));
        }
        let expires = artifacts.expires_seconds.unwrap_or(default_expires);
        if expires == 0 {
            self.disable();
            return Ok(artifacts);
        }
        let lease = self.lease.get_or_insert_with(|| {
            Arc::new(FlowLease {
                instance: offer.instance.clone(),
                state: Mutex::new(LeaseState {
                    expires: Instant::now(),
                    negotiated: false,
                    healthy: false,
                    healthy_until: Instant::now(),
                    failed: false,
                }),
            })
        });
        {
            let mut state = lease.state.lock().unwrap_or_else(|e| e.into_inner());
            // A refresh does not turn a failed flow back into a healthy flow.
            if state.failed {
                return Err(ImsError::new("ims_outbound_flow_failed"));
            }
            state.expires = Instant::now() + Duration::from_secs(u64::from(expires));
            state.negotiated = negotiated;
        }
        offer
            .coordinator
            .attach_flow(offer.access, Arc::downgrade(lease));
        self.tcp = tcp;
        self.timer = artifacts.flow_timer_seconds;
        if negotiated && !tcp && self.no_nat && self.timer.is_none() {
            // TS 24.229 K.2.1.5 allows omitting keepalive without NAT. Still
            // honor an explicit Flow-Timer. "ob" alone is explicitly not
            // evidence that an IMS P-CSCF implements STUN keepalive.
            self.next_probe = None;
            self.pending = None;
            lease.acknowledge(Instant::now() + Duration::from_secs(u64::from(expires)));
            offer.coordinator.flow_healthy(offer.access);
        } else if negotiated {
            // Validate the next hop before allowing a second access REGISTER.
            // Refresh preserves an outstanding probe/deadline, never postpones
            // an unacknowledged probe by repeatedly resetting its timer.
            if self.next_probe.is_none() && self.pending.is_none() {
                self.next_probe = Some(Instant::now());
            }
        } else {
            self.next_probe = None;
            self.pending = None;
        }
        tracing::info!(
            access = offer.access.as_str(),
            offered,
            require_outbound = artifacts.outbound_required,
            first_hop_outbound = artifacts.first_hop_outbound,
            own_binding_found = artifacts.own_binding_found,
            negotiated,
            no_nat = self.no_nat,
            keepalive_required = self.next_probe.is_some() || self.pending.is_some(),
            flow_timer_seconds = self.timer,
            reg_id = offer.access.reg_id(),
            expires_seconds = expires,
            "IMS outbound registration flow accepted"
        );
        Ok(artifacts)
    }

    pub fn active(&self) -> bool {
        self.next_probe.is_some() || self.pending.is_some()
    }
    pub fn deadline(&self) -> Option<Instant> {
        self.pending
            .as_ref()
            .map(|p| p.deadline)
            .or(self.next_probe)
    }
    pub fn disable(&mut self) {
        if let Some(lease) = self.lease.take() {
            lease.invalidate();
        }
        self.pending = None;
        self.next_probe = None;
        self.mapped = None;
    }
    pub fn fail(&mut self, code: &'static str) -> ImsError {
        if let Some(lease) = &self.lease {
            lease.invalidate();
        }
        if !self.failure_reported {
            if let Some(offer) = &self.offer {
                offer.coordinator.flow_failed(offer.access);
            }
            self.failure_reported = true;
        }
        ImsError::new(code)
    }

    /// No IO here: channels send this payload on their original SIP socket.
    pub fn poll(&mut self, now: Instant) -> Result<Option<Vec<u8>>, ImsError> {
        if self.lease.as_ref().is_some_and(|l| l.failed()) {
            return Err(ImsError::new("ims_outbound_flow_failed"));
        }
        if let Some(pending) = &mut self.pending {
            if now < pending.deadline {
                return Ok(None);
            }
            if self.tcp || now >= pending.timeout_at {
                return Err(self.fail("ims_outbound_keepalive_timeout"));
            }
            if pending.retransmits >= 7 {
                pending.deadline = pending.timeout_at;
                return Ok(None);
            }
            pending.retransmits += 1;
            // Frequent bounded retries (0.5s, then 1s) also keep NAT state
            // alive while detecting a failed flow within the RFC 5626 limit.
            pending.deadline = if pending.retransmits == 7 {
                pending.timeout_at
            } else {
                (now + STUN_RTO * 2).min(pending.timeout_at)
            };
            return Ok(Some(pending.packet.clone()));
        }
        if self.next_probe.is_none_or(|deadline| now < deadline) {
            return Ok(None);
        }
        let mut id = [0u8; 12];
        SystemRandom::new()
            .fill(&mut id)
            .map_err(|_| ImsError::new("ims_outbound_random_failed"))?;
        let packet = if self.tcp {
            b"\r\n\r\n".to_vec()
        } else {
            binding_request(id)
        };
        self.pending = Some(PendingProbe {
            id,
            packet: packet.clone(),
            retransmits: 0,
            deadline: now + if self.tcp { PONG_TIMEOUT } else { STUN_RTO },
            timeout_at: now + if self.tcp { PONG_TIMEOUT } else { STUN_TIMEOUT },
        });
        self.next_probe = None;
        Ok(Some(packet))
    }

    fn acknowledged(&mut self, now: Instant) {
        let interval = probe_interval(self.timer, self.tcp);
        // Flow-Timer is measured between sends, not pong reception. Do not
        // add the network RTT to every keepalive interval.
        let next = self.pending.as_ref().map_or(now + interval, |p| {
            let timeout = if self.tcp { PONG_TIMEOUT } else { STUN_TIMEOUT };
            (p.timeout_at - timeout + interval).max(now)
        });
        self.pending = None;
        if let Some(lease) = &self.lease {
            lease.acknowledge(next + if self.tcp { PONG_TIMEOUT } else { STUN_TIMEOUT });
        }
        self.next_probe = Some(next);
        if let Some(offer) = &self.offer {
            offer.coordinator.flow_healthy(offer.access);
        }
    }

    pub fn pong(&mut self) {
        if self.tcp && self.pending.is_some() {
            self.acknowledged(Instant::now());
        }
    }

    /// Return true for a STUN datagram even if malformed/unmatched: binary
    /// traffic must never enter a SIP stream/framing buffer. Connected sockets
    /// perform source-tuple validation; the unpredictable transaction ID binds
    /// the response to this flow's outstanding request. For IMS AKA over UDP,
    /// the logical flow includes both protected tuples (TS 24.229 5.1.1.2.2):
    /// a response received on the protected server socket is valid too.
    pub fn receive_stun(&mut self, packet: &[u8]) -> Result<bool, ImsError> {
        if packet.first().is_none_or(|byte| byte & 0xc0 != 0) {
            return Ok(false);
        }
        // CRLF/lone LF are SIP keepalives, not STUN. Let stream framing consume
        // them without ever interpreting a stray newline as a successful pong.
        if packet.iter().all(|byte| matches!(byte, b'\r' | b'\n')) {
            return Ok(false);
        }
        let Some(pending) = &self.pending else {
            return Ok(true);
        };
        if self.tcp {
            return Ok(true);
        }
        match binding_response(packet, pending.id) {
            StunResponse::Ignore => {}
            StunResponse::Error => return Err(self.fail("ims_outbound_stun_error")),
            StunResponse::Mapped(addr) => {
                if self.mapped.is_some_and(|old| old != addr) {
                    return Err(self.fail("ims_outbound_mapping_changed"));
                }
                self.mapped = Some(addr);
                self.acknowledged(Instant::now());
            }
        }
        Ok(true)
    }
}

fn initial_response_confirms_no_nat(request: &[u8], response: &[u8]) -> bool {
    let via = |frame| {
        sip_frame::header_value(frame, "Via").or_else(|| sip_frame::header_value(frame, "v"))
    };
    let (Some(sent), Some(received)) = (via(request), via(response)) else {
        return false;
    };
    let Some(local) = sent
        .split(';')
        .next()
        .and_then(|sent_by| sent_by.split_whitespace().nth(1))
        .and_then(|addr| addr.parse::<SocketAddr>().ok())
    else {
        return false;
    };
    let parameter = |name: &str| {
        let values = received
            .split(';')
            .skip(1)
            .filter_map(|part| part.trim().split_once('='))
            .filter(|(key, _)| key.trim().eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim())
            .collect::<Vec<_>>();
        match values.as_slice() {
            [value] => Some((*value).to_string()),
            _ => None,
        }
    };
    // Demand explicit received/rport evidence; a missing or malformed
    // observation must not waive RFC 5626 maintenance on an unknown path.
    parameter("received").and_then(|addr| addr.trim_matches(['[', ']']).parse::<IpAddr>().ok())
        == Some(local.ip())
        && parameter("rport").and_then(|port| port.parse::<u16>().ok()) == Some(local.port())
}

pub fn has_option(frame: &[u8], header: &str, token: &str) -> bool {
    let mut values = sip_frame::header_values(frame, header);
    if header.eq_ignore_ascii_case("Supported") {
        values.extend(sip_frame::header_values(frame, "k"));
    }
    values
        .iter()
        .flat_map(|v| v.split(','))
        .any(|v| v.trim().eq_ignore_ascii_case(token))
}

fn offer_register(
    frame: &[u8],
    instance: &str,
    reg_id: u32,
    required: bool,
) -> Result<Vec<u8>, ImsError> {
    if instance.is_empty()
        || instance
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '"' | '\\' | '<' | '>'))
        || reg_id == 0
    {
        return Err(ImsError::new("ims_outbound_contact_invalid"));
    }
    rewrite_register(frame, Some((instance, reg_id, required)))
}

fn legacy_register(frame: &[u8]) -> Result<Vec<u8>, ImsError> {
    rewrite_register(frame, None)
}

fn append_option(tokens: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() && !tokens.iter().any(|token| token.eq_ignore_ascii_case(value)) {
        tokens.push(value.to_string());
    }
}

/// Complete Contact header parameters and capability options, never URI
/// parameters, quoted feature values, digest fields, or transaction identifiers.
/// Folded/compact headers are accepted; multiple Contacts fail closed instead
/// of editing another flow.
/// Emit one canonical Supported/Require field (RFC 3261 7.3.1): separate list
/// fields are legal, but a peer reading only the first field would miss an
/// appended outbound offer, including on the authenticated REGISTER.
fn rewrite_register(frame: &[u8], offer: Option<(&str, u32, bool)>) -> Result<Vec<u8>, ImsError> {
    let text =
        std::str::from_utf8(frame).map_err(|_| ImsError::new("ims_outbound_request_invalid"))?;
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| ImsError::new("ims_outbound_request_invalid"))?;
    let mut unfolded: Vec<String> = Vec::new();
    for line in head.split("\r\n") {
        if line.starts_with([' ', '\t']) {
            let previous = unfolded
                .last_mut()
                .ok_or_else(|| ImsError::new("ims_outbound_request_invalid"))?;
            previous.push(' ');
            previous.push_str(line.trim());
        } else {
            unfolded.push(line.to_string());
        }
    }
    let mut lines = Vec::new();
    let mut contacts = 0;
    let mut supported = Vec::new();
    let mut require = Vec::new();
    for line in unfolded {
        let Some((name, value)) = line.split_once(':') else {
            lines.push(line);
            continue;
        };
        let name = name.trim();
        if name.eq_ignore_ascii_case("Contact") || name.eq_ignore_ascii_case("m") {
            contacts += 1;
            if value.trim() == "*" {
                return Err(ImsError::new("ims_outbound_wildcard_unregister_forbidden"));
            }
            if split_sip_list(value).len() != 1 {
                return Err(ImsError::new("ims_outbound_contact_invalid"));
            }
            let end = name_addr_range(value)
                .map(|(_, end)| end + 1)
                .or_else(|| {
                    offer
                        .is_none()
                        .then(|| value.find(';').unwrap_or(value.len()))
                })
                .ok_or_else(|| ImsError::new("ims_outbound_contact_invalid"))?;
            let mut contact = value[..end].to_string();
            for parameter in split_sip_parameters(&value[end..]).into_iter().skip(1) {
                let key = parameter.split('=').next().unwrap_or("").trim();
                if !key.eq_ignore_ascii_case("reg-id")
                    && !(offer.is_some() && key.eq_ignore_ascii_case("+sip.instance"))
                {
                    contact.push(';');
                    contact.push_str(parameter);
                }
            }
            if let Some((instance, reg_id, _)) = offer {
                contact.push_str(&format!(";+sip.instance=\"<{instance}>\";reg-id={reg_id}"));
            }
            lines.push(format!("{name}:{contact}"));
        } else if name.eq_ignore_ascii_case("Supported")
            || name.eq_ignore_ascii_case("k")
            || name.eq_ignore_ascii_case("Require")
        {
            let tokens = if name.eq_ignore_ascii_case("Require") {
                &mut require
            } else {
                &mut supported
            };
            for token in value.split(',').map(str::trim) {
                if offer.is_some() || !token.eq_ignore_ascii_case("outbound") {
                    append_option(tokens, token);
                }
            }
        } else {
            lines.push(line);
        }
    }
    if contacts != 1 {
        return Err(ImsError::new("ims_outbound_contact_invalid"));
    }
    if let Some((_, _, required)) = offer {
        // RFC 5626 4.2.1 requires Path support as well as outbound.
        append_option(&mut supported, "path");
        append_option(&mut supported, "outbound");
        if required {
            append_option(&mut require, "outbound");
        }
    }
    if !supported.is_empty() {
        lines.push(format!("Supported: {}", supported.join(", ")));
    }
    if !require.is_empty() {
        lines.push(format!("Require: {}", require.join(", ")));
    }
    Ok(format!("{}\r\n\r\n{body}", lines.join("\r\n")).into_bytes())
}

pub(super) fn random_fraction() -> f64 {
    let mut bytes = [0u8; 4];
    if SystemRandom::new().fill(&mut bytes).is_err() {
        return 0.5;
    }
    f64::from(u32::from_be_bytes(bytes)) / f64::from(u32::MAX)
}
fn probe_interval(timer: Option<u32>, tcp: bool) -> Duration {
    let value = match timer {
        Some(seconds) => f64::from(seconds) * (0.8 + random_fraction() * 0.2),
        None if tcp => 95.0 + random_fraction() * 25.0,
        None => 24.0 + random_fraction() * 5.0,
    };
    Duration::from_secs_f64(value.max(0.001))
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

pub(crate) fn binding_request(id: [u8; 12]) -> Vec<u8> {
    let mut packet = vec![0, 1, 0, 8]; // Binding request, one FINGERPRINT
    packet.extend_from_slice(&COOKIE);
    packet.extend_from_slice(&id);
    let fingerprint = crc32(&packet) ^ FINGERPRINT_XOR;
    packet.extend_from_slice(&[0x80, 0x28, 0, 4]);
    packet.extend_from_slice(&fingerprint.to_be_bytes());
    packet
}

#[derive(Debug, PartialEq, Eq)]
enum StunResponse {
    Ignore,
    Error,
    Mapped(SocketAddr),
}

fn binding_response(packet: &[u8], id: [u8; 12]) -> StunResponse {
    if packet.len() < 20 || packet[4..8] != COOKIE || packet[8..20] != id {
        return StunResponse::Ignore;
    }
    let size = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    if size % 4 != 0 || size + 20 != packet.len() {
        return StunResponse::Ignore;
    }
    let kind = u16::from_be_bytes([packet[0], packet[1]]);
    if kind != 0x0101 && kind != 0x0111 {
        return StunResponse::Ignore;
    }
    let mut mapped = None;
    let mut error_code = None;
    let mut offset = 20;
    while offset < packet.len() {
        if offset + 4 > packet.len() {
            return StunResponse::Ignore;
        }
        let attr = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
        let len = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]) as usize;
        let end = offset + 4 + len;
        let padded_end = offset + 4 + ((len + 3) & !3);
        if padded_end > packet.len() {
            return StunResponse::Ignore;
        }
        let data = &packet[offset + 4..end];
        match attr {
            0x0020 => {
                if mapped.is_some() || data.len() < 4 {
                    return StunResponse::Ignore;
                }
                let port = u16::from_be_bytes([data[2], data[3]]) ^ 0x2112;
                let mut mask = [0u8; 16];
                mask[..4].copy_from_slice(&COOKIE);
                mask[4..].copy_from_slice(&id);
                let ip = match (data[1], data.len()) {
                    (1, 8) => IpAddr::V4(Ipv4Addr::new(
                        data[4] ^ mask[0],
                        data[5] ^ mask[1],
                        data[6] ^ mask[2],
                        data[7] ^ mask[3],
                    )),
                    (2, 20) => {
                        let mut addr = [0u8; 16];
                        for i in 0..16 {
                            addr[i] = data[4 + i] ^ mask[i];
                        }
                        IpAddr::V6(Ipv6Addr::from(addr))
                    }
                    _ => return StunResponse::Ignore,
                };
                mapped = Some(SocketAddr::new(ip, port));
            }
            0x8028 => {
                if len != 4
                    || padded_end != packet.len()
                    || data != (crc32(&packet[..offset]) ^ FINGERPRINT_XOR).to_be_bytes()
                {
                    return StunResponse::Ignore;
                }
            }
            0x0009 => {
                if len < 4
                    || error_code.is_some()
                    || data[2] & 0xf8 != 0
                    || !(3..=6).contains(&data[2])
                    || data[3] > 99
                {
                    return StunResponse::Ignore;
                }
                error_code = Some(u16::from(data[2]) * 100 + u16::from(data[3]));
            }
            // Known optional/core attributes that do not authenticate this
            // unauthenticated STUN usage. Unknown mandatory attributes reject.
            0x0001 | 0x000a => {}
            _ if attr >= 0x8000 => {}
            _ => return StunResponse::Ignore,
        }
        offset = padded_end;
    }
    // RFC 5626 section 8 requires only XOR-MAPPED-ADDRESS in a Binding
    // success response, not FINGERPRINT. SIP/STUN are distinguishable by
    // their first byte. Validate a fingerprint if supplied, but do not turn
    // a valid minimal response into a false keepalive timeout.
    if kind == 0x0111 {
        if error_code.is_some() {
            StunResponse::Error
        } else {
            StunResponse::Ignore
        }
    } else {
        mapped.map_or(StunResponse::Ignore, StunResponse::Mapped)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::connectivity::core::ims_access::{
        decide, ConcurrentRegistrationSupport, ImsAccessInputs, ImsAccessPreference,
    };

    pub(crate) const INSTANCE: &str = "urn:uuid:4dbe7146-bdf7-3eea-90a4-31e79e6d22f0";

    pub(crate) fn register_request(call_id: &str, cseq: u32) -> Vec<u8> {
        format!("REGISTER sip:ims.test SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1:5060;branch=z9hG4bK-{call_id}-{cseq}\r\nCall-ID: {call_id}\r\nCSeq: {cseq} REGISTER\r\nContact: <sip:ue@127.0.0.1:5060>\r\nExpires: 3600\r\nContent-Length: 0\r\n\r\n").into_bytes()
    }

    pub(crate) fn register_success(request: &[u8], expires: u32) -> Vec<u8> {
        format!("SIP/2.0 200 OK\r\nVia: {}\r\nCall-ID: {}\r\nCSeq: {}\r\nContact: {};expires={expires}\r\nRequire: outbound\r\nPath: <sip:edge.ims.test;lr;ob>\r\nFlow-Timer: 25\r\nContent-Length: 0\r\n\r\n",
            sip_frame::header_value(request, "Via").unwrap(),
            sip_frame::header_value(request, "Call-ID").unwrap(),
            sip_frame::header_value(request, "CSeq").unwrap(),
            sip_frame::header_value(request, "Contact").unwrap(),
        ).into_bytes()
    }

    fn register_flow(flow: &mut OutboundFlow, call_id: &str, cseq: u32, expires: u32) -> Vec<u8> {
        let request = flow.prepare(&register_request(call_id, cseq)).unwrap();
        let response = register_success(&request, expires);
        let artifacts = flow.registered(&response, false, 3600).unwrap();
        assert!(artifacts.own_binding_found);
        assert_eq!(artifacts.expires_seconds, Some(expires));
        request
    }

    fn acknowledge_probe(flow: &mut OutboundFlow, address: &str) {
        let packet = flow.poll(flow.deadline().unwrap()).unwrap().unwrap();
        let id = packet[8..20].try_into().unwrap();
        assert!(flow
            .receive_stun(&success(id, address.parse().unwrap()))
            .unwrap());
    }

    #[tokio::test]
    async fn two_accesses_keep_distinct_bindings_and_refresh_then_remove_only_one() {
        let coordinator = ims_registration_coordinator::for_line("outbound-two-bindings");
        let inputs = ImsAccessInputs {
            cellular_enabled: true,
            wlan_enabled: true,
            cellular_available: true,
            wlan_available: true,
            preference: ImsAccessPreference::Concurrent,
            ..Default::default()
        };
        coordinator.publish(decide(inputs)).await;
        assert!(coordinator.admit(ImsAccess::Cellular).await.is_err());
        let mut wlan = OutboundFlow::default();
        wlan.configure("outbound-two-bindings", ImsAccess::Wlan, INSTANCE, true);
        let initial_wlan = register_flow(&mut wlan, "wlan-binding", 1, 1800);
        assert!(!has_option(&initial_wlan, "Require", "outbound"));
        assert_eq!(
            coordinator.concurrent_support(),
            ConcurrentRegistrationSupport::NotNegotiated
        );
        acknowledge_probe(&mut wlan, "192.0.2.2:5060");

        let decision = decide(ImsAccessInputs {
            wlan_registered: true,
            concurrent_support: coordinator.concurrent_support(),
            ..inputs
        });
        assert_eq!(decision.effective_mode(), "concurrent");
        coordinator.publish(decision).await;
        let _permit = coordinator.admit(ImsAccess::Cellular).await.unwrap();
        assert_eq!(
            coordinator.registration_instance("urn:imei:other-profile"),
            INSTANCE
        );
        let mut cellular = OutboundFlow::default();
        cellular.configure("outbound-two-bindings", ImsAccess::Cellular, INSTANCE, true);
        let initial_cellular = register_flow(&mut cellular, "cellular-binding", 1, 2400);
        assert!(has_option(&initial_cellular, "Require", "outbound"));
        assert!(has_option(&initial_cellular, "Supported", "path"));
        acknowledge_probe(&mut cellular, "192.0.2.1:5060");

        let wlan_lease = wlan.lease.as_ref().unwrap().clone();
        let cell_lease = cellular.lease.as_ref().unwrap().clone();
        let wlan_expiry = wlan_lease.state.lock().unwrap().expires;
        let cellular_probe = cellular.deadline();
        let refresh = register_flow(&mut cellular, "cellular-binding", 2, 3200);
        assert!(Arc::ptr_eq(&cell_lease, cellular.lease.as_ref().unwrap()));
        assert_eq!(
            cellular.deadline(),
            cellular_probe,
            "refresh cannot postpone keepalive"
        );
        assert_eq!(wlan_lease.state.lock().unwrap().expires, wlan_expiry);
        assert!(wlan_lease.proven() && cell_lease.proven());
        assert_eq!(
            contact_parameter_for_test(&initial_cellular, "reg-id"),
            Some("1".into())
        );
        assert_eq!(
            contact_parameter_for_test(&initial_wlan, "reg-id"),
            Some("2".into())
        );
        assert_eq!(
            contact_parameter_for_test(&refresh, "+sip.instance"),
            Some(INSTANCE.into())
        );

        let remove = register_request("cellular-binding", 3);
        let remove = String::from_utf8(remove)
            .unwrap()
            .replace("Expires: 3600", "Expires: 0");
        let remove = cellular.prepare(remove.as_bytes()).unwrap();
        assert!(!sip_frame::header_value(&remove, "Contact")
            .unwrap()
            .contains('*'));
        assert_eq!(
            contact_parameter_for_test(&remove, "reg-id"),
            Some("1".into())
        );
        cellular
            .registered(&register_success(&remove, 0), false, 3600)
            .unwrap();
        assert!(!cell_lease.live());
        assert!(
            wlan_lease.proven(),
            "removing cellular must not remove WLAN"
        );
        assert_eq!(
            coordinator.concurrent_support(),
            ConcurrentRegistrationSupport::Negotiated
        );
    }

    fn contact_parameter_for_test(request: &[u8], name: &str) -> Option<String> {
        super::super::register_response::contact_parameter(
            &sip_frame::header_value(request, "Contact").unwrap(),
            name,
        )
    }

    #[test]
    fn secondary_refusal_preserves_primary_and_only_cools_down_secondary_creation() {
        let coordinator = ims_registration_coordinator::for_line("outbound-secondary-refused");
        let mut wlan = OutboundFlow::default();
        wlan.configure(
            "outbound-secondary-refused",
            ImsAccess::Wlan,
            INSTANCE,
            true,
        );
        register_flow(&mut wlan, "primary", 1, 1800);
        acknowledge_probe(&mut wlan, "192.0.2.2:5060");
        let mut cellular = OutboundFlow::default();
        cellular.configure(
            "outbound-secondary-refused",
            ImsAccess::Cellular,
            INSTANCE,
            true,
        );
        cellular.prepare(&register_request("secondary", 1)).unwrap();
        cellular.received_sip(b"SIP/2.0 439 First Hop Lacks Outbound Support\r\nCall-ID: secondary\r\nCSeq: 1 REGISTER\r\n\r\n").unwrap_err();
        assert!(!coordinator.flow_creation_ready(ImsAccess::Cellular));
        assert!(coordinator.flow_creation_ready(ImsAccess::Wlan));
        assert_eq!(
            coordinator.concurrent_support(),
            ConcurrentRegistrationSupport::Negotiated
        );
        let refresh = register_flow(&mut wlan, "primary", 2, 2000);
        assert!(has_option(&refresh, "Require", "outbound"));
        assert!(wlan.lease.as_ref().unwrap().proven());
    }

    #[test]
    fn unowned_or_failed_flow_is_never_proof_of_dual_registration() {
        let coordinator = ims_registration_coordinator::for_line("outbound-owner-lifetime");
        let mut flow = OutboundFlow::default();
        flow.configure("outbound-owner-lifetime", ImsAccess::Wlan, INSTANCE, true);
        register_flow(&mut flow, "owned", 1, 1800);
        acknowledge_probe(&mut flow, "192.0.2.2:5060");
        assert_eq!(
            coordinator.concurrent_support(),
            ConcurrentRegistrationSupport::Negotiated
        );
        flow.fail("ims_outbound_keepalive_timeout");
        assert_eq!(
            coordinator.concurrent_support(),
            ConcurrentRegistrationSupport::NotNegotiated
        );
        assert!(!coordinator.recovery_ready(ImsAccess::Wlan));
        drop(flow);
        assert!(coordinator.binding_instance(ImsAccess::Wlan).is_none());
    }

    #[test]
    fn retransmission_reuses_exact_outbound_offer_even_after_policy_changes() {
        let coordinator = ims_registration_coordinator::for_line("outbound-retransmit");
        let mut flow = OutboundFlow::default();
        flow.configure("outbound-retransmit", ImsAccess::Wlan, INSTANCE, true);
        let request = register_request("same-transaction", 1);
        let first = flow.prepare(&request).unwrap();
        coordinator.reject_outbound(ImsAccess::Wlan);
        assert_eq!(flow.prepare(&request).unwrap(), first);
        let next = flow
            .prepare(&register_request("same-transaction", 2))
            .unwrap();
        assert!(!has_option(&next, "Supported", "outbound"));
        assert_eq!(contact_parameter_for_test(&next, "reg-id"), None);
    }

    #[test]
    fn legacy_single_registration_retains_real_contact_expiry_without_reg_id_echo() {
        let mut flow = OutboundFlow::default();
        flow.configure("outbound-legacy-expiry", ImsAccess::Wlan, INSTANCE, true);
        flow.prepare(&register_request("legacy", 1)).unwrap();
        let response = b"SIP/2.0 200 OK\r\nCall-ID: legacy\r\nCSeq: 1 REGISTER\r\nContact: <sip:ue@127.0.0.1:5060>;expires=900\r\nContent-Length: 0\r\n\r\n";
        let artifacts = flow.registered(response, false, 3600).unwrap();
        assert_eq!(artifacts.expires_seconds, Some(900));
        assert!(!flow.active());
        assert_eq!(
            ims_registration_coordinator::for_line("outbound-legacy-expiry").concurrent_support(),
            ConcurrentRegistrationSupport::NotNegotiated
        );
    }

    #[test]
    fn changing_instance_or_ignoring_required_outbound_cannot_fake_dual_success() {
        let coordinator = ims_registration_coordinator::for_line("outbound-invalid-secondary");
        let mut primary = OutboundFlow::default();
        primary.configure(
            "outbound-invalid-secondary",
            ImsAccess::Wlan,
            INSTANCE,
            true,
        );
        register_flow(&mut primary, "primary", 1, 1800);
        acknowledge_probe(&mut primary, "192.0.2.2:5060");
        let mut secondary = OutboundFlow::default();
        secondary.configure(
            "outbound-invalid-secondary",
            ImsAccess::Cellular,
            "urn:uuid:wrong",
            true,
        );
        assert_eq!(
            secondary
                .prepare(&register_request("secondary", 1))
                .unwrap_err()
                .code(),
            "ims_outbound_additional_flow_not_supported"
        );
        assert!(primary.lease.as_ref().unwrap().proven());
        // A malformed 200 after Require: outbound must never count as success.
        let required =
            offer_register(&register_request("secondary", 2), INSTANCE, 1, true).unwrap();
        secondary.configure(
            "outbound-invalid-secondary",
            ImsAccess::Cellular,
            INSTANCE,
            true,
        );
        secondary.last_request = Some(required);
        let response = b"SIP/2.0 200 OK\r\nCall-ID: secondary\r\nCSeq: 2 REGISTER\r\nContact: <sip:other@127.0.0.1>;expires=1800\r\n\r\n";
        assert_eq!(
            secondary
                .registered(response, false, 3600)
                .unwrap_err()
                .code(),
            "ims_outbound_negotiation_lost"
        );
        assert_eq!(
            coordinator.concurrent_support(),
            ConcurrentRegistrationSupport::NotNegotiated
        );
    }

    pub(crate) fn success(id: [u8; 12], addr: SocketAddr) -> Vec<u8> {
        let mut data = vec![0, if addr.is_ipv4() { 1 } else { 2 }];
        data.extend_from_slice(&(addr.port() ^ 0x2112).to_be_bytes());
        let mut mask = COOKIE.to_vec();
        mask.extend_from_slice(&id);
        let ip = match addr.ip() {
            IpAddr::V4(v) => v.octets().to_vec(),
            IpAddr::V6(v) => v.octets().to_vec(),
        };
        data.extend(ip.iter().zip(&mask).map(|(v, m)| v ^ m));
        let mut packet = vec![1, 1];
        packet.extend_from_slice(&((data.len() + 12) as u16).to_be_bytes());
        packet.extend_from_slice(&COOKIE);
        packet.extend_from_slice(&id);
        packet.extend_from_slice(&[0, 0x20]);
        packet.extend_from_slice(&(data.len() as u16).to_be_bytes());
        packet.extend_from_slice(&data);
        let fingerprint = crc32(&packet) ^ FINGERPRINT_XOR;
        packet.extend_from_slice(&[0x80, 0x28, 0, 4]);
        packet.extend_from_slice(&fingerprint.to_be_bytes());
        packet
    }
    #[test]
    fn stun_ipv4_ipv6_fingerprint_and_transaction_validation() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        let id = [17; 12];
        assert_eq!(binding_request(id).len(), 28);
        for address in ["192.0.2.4:12345", "[2001:db8::1]:5060"] {
            let address = address.parse().unwrap();
            let packet = success(id, address);
            assert_eq!(binding_response(&packet, id), StunResponse::Mapped(address));
            assert_eq!(binding_response(&packet, [18; 12]), StunResponse::Ignore);
            assert_eq!(
                binding_response(&packet[..packet.len() - 1], id),
                StunResponse::Ignore
            );
            let mut bad_crc = packet.clone();
            *bad_crc.last_mut().unwrap() ^= 1;
            assert_eq!(binding_response(&bad_crc, id), StunResponse::Ignore);
            let mut no_fingerprint = packet[..packet.len() - 8].to_vec();
            let size = (no_fingerprint.len() - 20) as u16;
            no_fingerprint[2..4].copy_from_slice(&size.to_be_bytes());
            assert_eq!(
                binding_response(&no_fingerprint, id),
                StunResponse::Mapped(address)
            );
        }
    }
    #[test]
    fn minimal_stun_response_proves_flow_without_fingerprint() {
        let coordinator = ims_registration_coordinator::for_line("outbound-minimal-stun");
        let mut flow = OutboundFlow::default();
        flow.configure("outbound-minimal-stun", ImsAccess::Wlan, INSTANCE, true);
        register_flow(&mut flow, "minimal-stun", 1, 1800);
        let request = flow.poll(flow.deadline().unwrap()).unwrap().unwrap();
        let id = request[8..20].try_into().unwrap();
        let mut response = success(id, "192.0.2.2:5060".parse().unwrap());
        response.truncate(response.len() - 8);
        let size = (response.len() - 20) as u16;
        response[2..4].copy_from_slice(&size.to_be_bytes());
        assert!(flow.receive_stun(&response).unwrap());
        assert!(flow.pending.is_none());
        assert!(flow.lease.as_ref().unwrap().proven());
        assert_eq!(
            coordinator.concurrent_support(),
            ConcurrentRegistrationSupport::Negotiated
        );
    }
    #[test]
    fn timer_ranges_never_exceed_network_deadline() {
        for _ in 0..100 {
            let seconds = probe_interval(Some(25), false).as_secs_f64();
            assert!((20.0..=25.0).contains(&seconds));
            let seconds = probe_interval(None, false).as_secs_f64();
            assert!((24.0..=29.0).contains(&seconds));
        }
    }
    #[test]
    fn register_completion_retains_transaction_and_rejects_wildcard() {
        let request = b"REGISTER sip:ims.test SIP/2.0\r\nCall-ID: a\r\nCSeq: 4 REGISTER\r\nContact: <sip:a@192.0.2.1>;expires=3600;reg-id=9\r\nSupported: path\r\nContent-Length: 0\r\n\r\n";
        let result = offer_register(request, "urn:uuid:stable", 2, true).unwrap();
        assert!(has_option(&result, "Supported", "outbound"));
        assert!(has_option(&result, "Require", "outbound"));
        assert_eq!(
            RegisterTransactionKey::from_register_request(request),
            RegisterTransactionKey::from_register_request(&result)
        );
        let contact = sip_frame::header_value(&result, "Contact").unwrap();
        assert!(contact.contains("expires=3600"));
        assert!(contact.contains("reg-id=2"));
        assert!(!contact.contains("reg-id=9"));
        assert!(offer_register(
            b"REGISTER sip:a SIP/2.0\r\nContact: *\r\n\r\n",
            "i",
            1,
            false
        )
        .is_err());
    }

    #[test]
    fn outbound_capabilities_are_in_one_field_on_initial_auth_refresh_and_remove() {
        let mut flow = OutboundFlow::default();
        flow.configure("outbound-canonical-register", ImsAccess::Wlan, INSTANCE, true);
        for (cseq, expires) in [(1, 3600), (2, 3600), (3, 3600), (4, 0)] {
            let source = String::from_utf8(register_request("same-binding", cseq))
                .unwrap()
                .replace("Expires: 3600", &format!("Expires: {expires}"))
                .replace(
                    "Content-Length: 0",
                    "Supported: path,sec-agree,gruu\r\nRequire: sec-agree\r\nAuthorization: Digest username=\"test\", response=\"unchanged\"\r\nContent-Length: 0",
                );
            let prepared = flow.prepare(source.as_bytes()).unwrap();
            assert_eq!(
                sip_frame::header_values(&prepared, "Supported"),
                vec!["path, sec-agree, gruu, outbound"]
            );
            assert!(sip_frame::header_values(&prepared, "k").is_empty());
            assert_eq!(
                sip_frame::header_value(&prepared, "Authorization"),
                sip_frame::header_value(source.as_bytes(), "Authorization")
            );
            assert_eq!(
                RegisterTransactionKey::from_register_request(&prepared),
                RegisterTransactionKey::from_register_request(source.as_bytes())
            );
            // Retransmissions do not rebuild or modify the capability offer.
            assert_eq!(flow.prepare(source.as_bytes()).unwrap(), prepared);
            assert_eq!(
                sip_frame::header_value(&prepared, "Expires"),
                Some(expires.to_string())
            );
        }
    }

    #[test]
    fn folded_compact_and_repeated_options_coalesce_without_losing_security() {
        let source = b"REGISTER sip:ims.test SIP/2.0\r\nCall-ID: options\r\nCSeq: 2 REGISTER\r\nContact: <sip:ue@127.0.0.1>;expires=3600\r\nSupported: path,\r\n sec-agree\r\nk: GRUU, PATH\r\nSupported: outbound, x-outbound\r\nRequire: sec-agree\r\nRequire: outbound\r\nSecurity-Verify: ipsec-3gpp;spi-c=100;spi-s=200\r\nContent-Length: 0\r\n\r\n";
        let prepared = offer_register(source, INSTANCE, 2, true).unwrap();
        assert_eq!(
            sip_frame::header_values(&prepared, "Supported"),
            vec!["path, sec-agree, GRUU, outbound, x-outbound"]
        );
        assert_eq!(
            sip_frame::header_values(&prepared, "Require"),
            vec!["sec-agree, outbound"]
        );
        assert!(sip_frame::header_values(&prepared, "k").is_empty());
        assert_eq!(
            sip_frame::header_value(&prepared, "Security-Verify"),
            sip_frame::header_value(source, "Security-Verify")
        );
        // Formatting and Contact completion are idempotent.
        assert_eq!(offer_register(&prepared, INSTANCE, 2, true).unwrap(), prepared);
    }

    #[test]
    fn legacy_offer_removes_only_exact_outbound_and_preserves_instance_and_body() {
        let source = b"REGISTER sip:ims.test SIP/2.0\r\nCall-ID: legacy-options\r\nCSeq: 1 REGISTER\r\nContact: <sip:ue@127.0.0.1>;+sip.instance=\"<urn:uuid:stable>\";reg-id=2\r\nSupported: path, OutBound\r\nk: x-outbound, sec-agree\r\nRequire: OUTBOUND, sec-agree\r\nContent-Length: 4\r\n\r\ntest";
        let prepared = legacy_register(source).unwrap();
        assert_eq!(
            sip_frame::header_values(&prepared, "Supported"),
            vec!["path, x-outbound, sec-agree"]
        );
        assert_eq!(
            sip_frame::header_values(&prepared, "Require"),
            vec!["sec-agree"]
        );
        let contact = sip_frame::header_value(&prepared, "Contact").unwrap();
        assert!(contact.contains("+sip.instance=\"<urn:uuid:stable>\""));
        assert!(!contact.contains("reg-id="));
        assert!(prepared.ends_with(b"\r\n\r\ntest"));
        assert_eq!(legacy_register(&prepared).unwrap(), prepared);
    }

    #[test]
    fn echoed_binding_without_require_is_evidence_not_dual_flow_permission() {
        let coordinator = ims_registration_coordinator::for_line("outbound-echo-only");
        let mut flow = OutboundFlow::default();
        flow.configure("outbound-echo-only", ImsAccess::Wlan, INSTANCE, true);
        let prepared = flow.prepare(&register_request("echo", 1)).unwrap();
        let response = String::from_utf8(register_success(&prepared, 3195))
            .unwrap()
            .replace("Require: outbound\r\n", "")
            .replace(";lr;ob>", ";lr>")
            .replace("Flow-Timer: 25\r\n", "");
        let artifacts = flow.registered(response.as_bytes(), false, 3600).unwrap();
        assert!(artifacts.own_binding_found);
        assert!(!artifacts.outbound_required);
        assert!(!artifacts.first_hop_outbound);
        assert_eq!(artifacts.expires_seconds, Some(3195));
        assert_eq!(
            coordinator.concurrent_support(),
            ConcurrentRegistrationSupport::NotNegotiated
        );
        assert!(flow.poll(Instant::now()).unwrap().is_none());
    }

    #[test]
    fn no_negotiation_means_no_blind_stun() {
        let mut flow = OutboundFlow::default();
        assert!(flow.poll(Instant::now()).unwrap().is_none());
    }
    #[test]
    fn ims_no_nat_path_does_not_require_unsolicited_stun() {
        let coordinator = ims_registration_coordinator::for_line("outbound-ims-no-nat");
        let mut initial = OutboundFlow::default();
        initial.configure("outbound-ims-no-nat", ImsAccess::Wlan, INSTANCE, true);
        let request = initial.prepare(&register_request("ims-no-nat", 1)).unwrap();
        let challenge = format!(
            "SIP/2.0 401 Unauthorized\r\nVia: {};received=127.0.0.1;rport=5060\r\nCall-ID: ims-no-nat\r\nCSeq: 1 REGISTER\r\n\r\n",
            sip_frame::header_value(&request, "Via").unwrap()
        );
        initial.received_sip(challenge.as_bytes()).unwrap();
        assert!(initial.no_nat);
        let mut protected = initial.replacement();
        let mut wlan_protected = OutboundFlow::default();
        wlan_protected.inherit_path_observation(&initial);
        assert!(wlan_protected.no_nat);
        let request = protected
            .prepare(&register_request("ims-no-nat", 2))
            .unwrap();
        let response = String::from_utf8(register_success(&request, 1800))
            .unwrap()
            .replace("Flow-Timer: 25\r\n", "");
        protected
            .registered(response.as_bytes(), false, 3600)
            .unwrap();
        assert!(!protected.active());
        assert!(protected.poll(Instant::now()).unwrap().is_none());
        assert_eq!(
            coordinator.concurrent_support(),
            ConcurrentRegistrationSupport::Negotiated
        );

        // A later explicit Flow-Timer overrides the no-NAT exemption.
        register_flow(&mut protected, "ims-no-nat", 3, 1800);
        assert!(protected.active());
        assert!(protected.poll(Instant::now()).unwrap().is_some());
    }
    #[test]
    fn nat_exemption_requires_explicit_matching_initial_tuple() {
        let request = register_request("nat-observation", 1);
        let via = sip_frame::header_value(&request, "Via").unwrap();
        for suffix in [
            "",
            ";received=127.0.0.1",
            ";received=192.0.2.2;rport=5060",
            ";received=127.0.0.1;rport=62000",
            ";received=127.0.0.1;rport=5060;rport=5060",
        ] {
            let response = format!("SIP/2.0 401 Unauthorized\r\nVia: {via}{suffix}\r\n\r\n");
            assert!(
                !initial_response_confirms_no_nat(&request, response.as_bytes()),
                "{suffix}"
            );
        }
        let request = String::from_utf8(request)
            .unwrap()
            .replace("127.0.0.1", "[2001:db8::1]");
        let via = sip_frame::header_value(request.as_bytes(), "Via").unwrap();
        let response = format!(
            "SIP/2.0 401 Unauthorized\r\nVia: {via};received=2001:db8::1;rport=5060\r\n\r\n"
        );
        assert!(initial_response_confirms_no_nat(
            request.as_bytes(),
            response.as_bytes()
        ));
    }
    #[test]
    fn udp_probe_mapping_change_and_timeout_are_flow_failures() {
        let mut flow = OutboundFlow::default();
        flow.next_probe = Some(Instant::now());
        flow.poll(Instant::now()).unwrap().unwrap();
        let id = flow.pending.as_ref().unwrap().id;
        assert!(flow
            .receive_stun(&success(id, "192.0.2.1:1234".parse().unwrap()))
            .unwrap());
        let due = flow.next_probe.unwrap();
        flow.poll(due).unwrap().unwrap();
        let id = flow.pending.as_ref().unwrap().id;
        assert_eq!(
            flow.receive_stun(&success(id, "192.0.2.1:1235".parse().unwrap()))
                .unwrap_err()
                .code(),
            "ims_outbound_mapping_changed"
        );
        let mut flow = OutboundFlow::default();
        flow.next_probe = Some(Instant::now());
        flow.poll(Instant::now()).unwrap();
        for _ in 0..7 {
            let due = flow.deadline().unwrap();
            assert!(flow.poll(due).unwrap().is_some());
        }
        let due = flow.deadline().unwrap();
        assert_eq!(
            flow.poll(due).unwrap_err().code(),
            "ims_outbound_keepalive_timeout"
        );
    }
}
