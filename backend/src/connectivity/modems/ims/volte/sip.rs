//! VoLTE SIP message construction + response parsing.
//!
//! Clean-room implementation from RFC 3261 (SIP) and 3GPP TS 24.229/24.341.
//! The header set and 3GPP feature tags are reproduced to interoperate with a
//! real operator P-CSCF; they are public-spec constants, not copied source.
//!
//! Transport note: VoLTE carries SIP over the IMS APN bearer. When IPsec is
//! established the signaling rides the kernel xfrm SAs; on fallback it is plain
//! UDP. Either way the wire format built here is identical — only the socket
//! underneath differs. This mirrors the reference behavior where the same
//! `MESSAGE`/`REGISTER` builders are used for both `register_ipsec` and
//! `register_udp` modes.

use std::net::IpAddr;

use super::errors::VolteError;
use crate::connectivity::core::{
    register_message::{RegisterHeaderFields, RegisterRequest},
    sip_message::{SipHeader, SipRequest},
};
use crate::connectivity::modems::ims::vowifi::profiles::CarrierProfile;

pub use crate::connectivity::core::context::{
    ImsIdentity, ImsRegisterParams, ImsRoute as SipRoute, SipTransport,
};

/// 3GPP SMS-over-IP ICSI service identifier (TS 24.341).
pub const SMS_ICSI: &str = "urn:urn-7:3gpp-service.ims.icsi.sms";
/// 3GPP MMTel (voice) ICSI service identifier (TS 24.173). URL-encoded form is
/// used inside `+g.3gpp.icsi-ref` feature tags.
pub const MMTEL_ICSI: &str = "urn:urn-7:3gpp-service.ims.icsi.mmtel";
pub const MMTEL_ICSI_REF: &str = "urn%3Aurn-7%3A3gpp-service.ims.icsi.mmtel";
/// Access network info value observed for the LTE (E-UTRAN) path.
pub const PANI_EUTRAN: &str = "3GPP-E-UTRAN-FDD";
pub const USER_AGENT: &str = "SimAdmin VoLTE";
pub const SMS_CONTENT_TYPE: &str = "application/vnd.3gpp.sms";
pub const DTMF_RELAY_CONTENT_TYPE: &str = "application/dtmf-relay";
pub const MMTEL_ALLOW_METHODS: &str =
    "INVITE,ACK,CANCEL,BYE,UPDATE,PRACK,MESSAGE,REFER,NOTIFY,INFO,OPTIONS";

/// RFC 5626 `reg-id` for this (cellular) access leg.
///
/// Sourced from the access policy rather than written literally, because the
/// two legs now share one `+sip.instance` and a binding is keyed on
/// (AOR, instance-id, reg-id): if both legs emitted the same reg-id, the
/// second registration would silently replace the first.
const CELLULAR_REG_ID: u32 = crate::connectivity::core::ims_access::ImsAccess::Cellular.reg_id();

/// Format a host for a SIP URI: bare IPv4, bracketed IPv6 (RFC 3261 §19.1.2).
/// Delegates to the shared IMS core.
pub fn sip_host(ip: IpAddr) -> String {
    crate::connectivity::core::sip_frame::sip_host(ip)
}

/// Route set learned during REGISTER, falling back to the directly discovered
/// P-CSCF only when the network did not return `Service-Route`.
fn route_header_value(route: &SipRoute, service_route: Option<&str>) -> String {
    service_route
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            format!(
                "<sip:{}:{};lr>",
                sip_host(route.pcscf_addr.ip()),
                route.pcscf_addr.port()
            )
        })
}

/// Escape a quoted SIP header parameter value. Delegates to the shared IMS core.
pub fn quote_sip_param(value: &str) -> String {
    crate::connectivity::core::sip_frame::quote_param(value)
}

/// Random lowercase-hex token of `bytes` bytes (result string len = 2*bytes).
/// Falls back to a fixed token if the RNG fails, matching the reference's
/// defensive posture (a failed RNG must not panic the signaling path).
pub fn hex_token(bytes: usize) -> String {
    match random_bytes(bytes) {
        Ok(b) => b.iter().map(|byte| format!("{byte:02x}")).collect(),
        Err(_) => "simadmin".to_string(),
    }
}

fn random_bytes(len: usize) -> Result<Vec<u8>, VolteError> {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut buf = vec![0u8; len];
    SystemRandom::new()
        .fill(&mut buf)
        .map_err(|_| VolteError::new("volte_random_failed"))?;
    Ok(buf)
}

/// A branch parameter with the RFC 3261 magic cookie.
pub fn new_branch() -> String {
    format!("z9hG4bK{}", hex_token(12))
}

/// Builder inputs shared across request types within one dialog/transaction.
#[derive(Debug, Clone)]
pub struct RequestIds {
    pub call_id: String,
    pub from_tag: String,
    pub cseq: u32,
}

/// Carrier-facing REGISTER header policy. The transaction/authentication
/// sequence stays shared, while a few P-CSCFs differ on whether the first
/// unauthenticated REGISTER may advertise sec-agree and MMTel features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisterRequestPolicy {
    pub advertise_sec_agree: bool,
    pub require_sec_agree: bool,
    pub proxy_require_sec_agree: bool,
    pub include_mmtel_features: bool,
    /// Local media capability gate. Carrier permission still comes from the
    /// catalog's explicit Contact parameter list.
    pub include_video_feature: bool,
    pub include_route_header: bool,
    pub include_visited_network: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterPhase {
    Initial,
    Authenticated,
    Refresh,
}

/// Access-specific REGISTER addressing fixed for one IMS session. Carrier
/// header/security policy remains in `CarrierProfile`, while these values may
/// come from a SIM-bound user override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisterTarget<'a> {
    pub domain: &'a str,
    pub realm: &'a str,
    pub registrar: Option<&'a str>,
}

impl<'a> RegisterTarget<'a> {
    pub fn from_profile(profile: &'a CarrierProfile) -> Self {
        Self {
            domain: profile.ims.domain,
            realm: profile.ims.realm,
            registrar: profile.ims.registrar,
        }
    }
}

impl RegisterRequestPolicy {
    pub const LEGACY: Self = Self {
        // The proven Maxis request requires sec-agree but does not advertise it
        // in Supported on the initial transaction.
        advertise_sec_agree: false,
        require_sec_agree: true,
        proxy_require_sec_agree: true,
        include_mmtel_features: false,
        include_video_feature: false,
        include_route_header: false,
        include_visited_network: false,
    };
}

impl RequestIds {
    pub fn fresh(cseq: u32) -> Self {
        Self {
            call_id: format!("{}@simadmin", hex_token(16)),
            from_tag: hex_token(8),
            cseq,
        }
    }
}

/// Build a REGISTER request (initial or authenticated).
///
/// `authorization` is the full `Authorization: Digest ...` header line without
/// trailing CRLF, or `None` for the initial empty-AKA register (which the
/// caller can supply via `initial_authorization` instead).
#[allow(clippy::too_many_arguments)]
pub fn build_register(
    identity: &ImsIdentity,
    route: &SipRoute,
    ids: &RequestIds,
    expires: u32,
    authorization: Option<&str>,
    security_client: Option<&str>,
    security_verify: Option<&str>,
    sip_instance: &str,
) -> Vec<u8> {
    build_register_with_security_policy(
        identity,
        route,
        ids,
        expires,
        authorization,
        security_client,
        security_verify,
        sip_instance,
        true,
    )
}

/// REGISTER builder with an explicit sec-agree policy. The normal VoLTE path
/// requires IPsec; the `false` form is reserved for the documented UDP
/// degradation path when the network omits Security-Server entirely.
#[allow(clippy::too_many_arguments)]
pub fn build_register_with_security_policy(
    identity: &ImsIdentity,
    route: &SipRoute,
    ids: &RequestIds,
    expires: u32,
    authorization: Option<&str>,
    security_client: Option<&str>,
    security_verify: Option<&str>,
    sip_instance: &str,
    require_sec_agree: bool,
) -> Vec<u8> {
    build_register_with_policy(
        identity,
        route,
        ids,
        expires,
        authorization,
        security_client,
        security_verify,
        sip_instance,
        RegisterRequestPolicy {
            advertise_sec_agree: require_sec_agree,
            require_sec_agree,
            proxy_require_sec_agree: require_sec_agree,
            ..RegisterRequestPolicy::LEGACY
        },
    )
}

/// Build a REGISTER with explicit carrier header policy.
#[allow(clippy::too_many_arguments)]
pub fn build_register_with_policy(
    identity: &ImsIdentity,
    route: &SipRoute,
    ids: &RequestIds,
    expires: u32,
    authorization: Option<&str>,
    security_client: Option<&str>,
    security_verify: Option<&str>,
    sip_instance: &str,
    policy: RegisterRequestPolicy,
) -> Vec<u8> {
    build_register_internal(
        None,
        None,
        RegisterPhase::Authenticated,
        identity,
        route,
        ids,
        expires,
        authorization,
        security_client,
        security_verify,
        sip_instance,
        policy,
        None,
    )
}

/// Build REGISTER using the carrier catalog's IMS and SIP policy.
#[allow(clippy::too_many_arguments)]
pub fn build_register_from_profile(
    profile: &CarrierProfile,
    phase: RegisterPhase,
    identity: &ImsIdentity,
    route: &SipRoute,
    ids: &RequestIds,
    expires: u32,
    authorization: Option<&str>,
    security_client: Option<&str>,
    security_verify: Option<&str>,
    sip_instance: &str,
    policy: RegisterRequestPolicy,
) -> Vec<u8> {
    build_register_from_profile_with_target(
        profile,
        RegisterTarget::from_profile(profile),
        phase,
        identity,
        route,
        ids,
        expires,
        authorization,
        security_client,
        security_verify,
        sip_instance,
        policy,
    )
}

/// Build REGISTER with catalog policy and an access-specific addressing
/// snapshot. This is the live adapter entry point for per-SIM overrides.
#[allow(clippy::too_many_arguments)]
pub fn build_register_from_profile_with_target(
    profile: &CarrierProfile,
    target: RegisterTarget<'_>,
    phase: RegisterPhase,
    identity: &ImsIdentity,
    route: &SipRoute,
    ids: &RequestIds,
    expires: u32,
    authorization: Option<&str>,
    security_client: Option<&str>,
    security_verify: Option<&str>,
    sip_instance: &str,
    policy: RegisterRequestPolicy,
) -> Vec<u8> {
    build_register_from_profile_with_target_and_visited(
        profile,
        target,
        phase,
        identity,
        route,
        ids,
        expires,
        authorization,
        security_client,
        security_verify,
        sip_instance,
        policy,
        None,
    )
}

/// Build REGISTER with a runtime visited-network override.
///
/// Carrier profiles normally provide a static `P-Visited-Network-ID`. During
/// roaming the visited PLMN is learned from ModemManager, so the live path can
/// replace that static value without changing the home profile used for IMS
/// identities, APN, authentication, or registrar selection.
#[allow(clippy::too_many_arguments)]
pub fn build_register_from_profile_with_target_and_visited(
    profile: &CarrierProfile,
    target: RegisterTarget<'_>,
    phase: RegisterPhase,
    identity: &ImsIdentity,
    route: &SipRoute,
    ids: &RequestIds,
    expires: u32,
    authorization: Option<&str>,
    security_client: Option<&str>,
    security_verify: Option<&str>,
    sip_instance: &str,
    policy: RegisterRequestPolicy,
    visited_network_override: Option<&str>,
) -> Vec<u8> {
    build_register_internal(
        Some(profile),
        Some(target),
        phase,
        identity,
        route,
        ids,
        expires,
        authorization,
        security_client,
        security_verify,
        sip_instance,
        policy,
        visited_network_override,
    )
}

/// Resolve the REGISTER Request-URI from the same carrier policy and route the
/// request builder uses. Digest AKA must sign this exact value.
pub fn register_request_uri(profile: &CarrierProfile, route: &SipRoute) -> String {
    register_request_uri_with_target(profile, RegisterTarget::from_profile(profile), route)
}

pub fn register_request_uri_with_target(
    profile: &CarrierProfile,
    target: RegisterTarget<'_>,
    route: &SipRoute,
) -> String {
    match profile.ims.register.request_uri_policy {
        "home_domain" => format!("sip:{}", target.domain),
        "pcscf" => format!(
            "sip:{}:{}",
            sip_host(route.pcscf_addr.ip()),
            route.pcscf_addr.port()
        ),
        "registrar" | "configured" => {
            let registrar = target.registrar.unwrap_or(target.domain);
            if registrar.starts_with("sip:") || registrar.starts_with("sips:") {
                registrar.to_string()
            } else {
                format!("sip:{registrar}")
            }
        }
        _ => format!("sip:{}", target.domain),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_register_internal(
    profile: Option<&CarrierProfile>,
    target: Option<RegisterTarget<'_>>,
    phase: RegisterPhase,
    identity: &ImsIdentity,
    route: &SipRoute,
    ids: &RequestIds,
    expires: u32,
    authorization: Option<&str>,
    security_client: Option<&str>,
    security_verify: Option<&str>,
    sip_instance: &str,
    policy: RegisterRequestPolicy,
    visited_network_override: Option<&str>,
) -> Vec<u8> {
    let branch = new_branch();
    let local_host = sip_host(route.local_addr.ip());
    let local_port = route.local_addr.port();
    // An MMTEL-capable cellular binding must identify its radio access. Some
    // imported carrier bundles omit the PANI booleans even though they enable
    // MMTEL, which can leave a 200-OK registration ineligible for MT routing.
    let include_pani = policy.include_mmtel_features
        || profile.is_none_or(|profile| match phase {
            RegisterPhase::Initial => profile.ims.register.include_pani_initial,
            RegisterPhase::Authenticated | RegisterPhase::Refresh => {
                profile.ims.register.include_pani_authenticated
            }
        });
    let access_network_info = profile
        .map(|profile| profile.ims.register.access_network_info)
        .unwrap_or(PANI_EUTRAN);
    let params = ImsRegisterParams {
        realm: target
            .map(|target| target.realm)
            .or_else(|| profile.map(|profile| profile.ims.realm))
            .unwrap_or(identity.home_domain.as_str())
            .to_string(),
        domain: target
            .map(|target| target.domain)
            .or_else(|| profile.map(|profile| profile.ims.domain))
            .unwrap_or(identity.home_domain.as_str())
            .to_string(),
        registrar: target
            .and_then(|target| target.registrar.map(str::to_string))
            .or_else(|| profile.and_then(|profile| profile.ims.registrar.map(str::to_string))),
        supported_header: profile.map_or_else(
            || {
                if policy.advertise_sec_agree {
                    "path, gruu, sec-agree"
                } else {
                    "path, gruu"
                }
                .to_string()
            },
            |profile| profile.ims.register.supported_header.to_string(),
        ),
        require_sec_agree: policy.require_sec_agree,
        user_agent: profile
            .map(|profile| profile.ims.user_agent)
            .unwrap_or(USER_AGENT)
            .to_string(),
        pani: include_pani.then(|| access_network_info.to_string()),
        visited_network: None,
        allow_header: profile
            .and_then(|profile| profile.ims.register.allow_methods)
            .unwrap_or_else(|| {
                if policy.include_mmtel_features {
                    MMTEL_ALLOW_METHODS
                } else if profile.is_some() {
                    ""
                } else {
                    MMTEL_ALLOW_METHODS
                }
            })
            .to_string(),
        expires,
    };
    let request_uri = profile.map_or_else(
        || params.request_uri(),
        |profile| {
            register_request_uri_with_target(
                profile,
                target.unwrap_or_else(|| RegisterTarget::from_profile(profile)),
                route,
            )
        },
    );
    let to_value = format!("<{}>", identity.public_uri);
    let mut contact = format!(
        "<sip:{}@{}:{};transport={}>",
        identity.contact_user,
        local_host,
        local_port,
        route.transport.as_param(),
    );
    let mut advertises_sms_over_ip = false;
    let mut declared_sip_instance = false;
    let always_add_sip_instance =
        profile.is_some_and(|profile| profile.ims.register.always_add_sip_instance);
    // A profile dictates the Contact parameter list only when it actually
    // carries one. Catalog bundles that omit `sip.common.contact_parameters`
    // -- and every hardcoded profile, which sets `contact_param_order: &[]`
    // -- are expressing no opinion, not "advertise nothing". Treating the
    // empty list as a third, silent case skipped both arms below and emitted
    // a bare `<sip:user@host:port;transport=udp>;+g.3gpp.smsip`: without
    // `+g.3gpp.icsi-ref` the S-CSCF has no reason to consider the
    // registration MMTEL voice capable, so terminating calls are never
    // delivered even though REGISTER answers 200 OK.
    let explicit_parameters = profile
        .map(|profile| profile.ims.register.contact_param_order)
        .unwrap_or(&[]);
    if !explicit_parameters.is_empty() {
        for parameter in explicit_parameters {
            let name = parameter
                .split_once('=')
                .map_or(*parameter, |(name, _)| name)
                .trim();
            if name.eq_ignore_ascii_case("video") && !policy.include_video_feature {
                continue;
            }
            if name.eq_ignore_ascii_case("+g.3gpp.smsip") {
                advertises_sms_over_ip = true;
            }
            if name.eq_ignore_ascii_case("+sip.instance") {
                declared_sip_instance = true;
            }
            contact.push(';');
            contact.push_str(parameter);
        }
    } else {
        contact.push_str(&format!(";+g.3gpp.accesstype=\"{access_network_info}\""));
        if policy.include_mmtel_features {
            contact.push_str(";audio");
        }
        contact.push_str(";+g.3gpp.smsip");
        advertises_sms_over_ip = true;
        if policy.include_mmtel_features {
            contact.push_str(&format!(";+g.3gpp.icsi-ref=\"{}\"", MMTEL_ICSI_REF));
        }
        contact.push_str(&format!(";+sip.instance=\"<{}>\"", sip_instance));
        // RFC 5626: reg-id only has meaning next to the instance it pairs
        // with, so emit it here rather than letting the tail append a second
        // +sip.instance further down the parameter list.
        if always_add_sip_instance {
            contact.push_str(&format!(";reg-id={CELLULAR_REG_ID}"));
        }
        declared_sip_instance = true;
        contact.push_str(&format!(";expires={expires}"));
    }
    if !advertises_sms_over_ip {
        contact.push_str(";+g.3gpp.smsip");
    }
    if always_add_sip_instance && !declared_sip_instance {
        contact.push_str(&format!(
            ";+sip.instance=\"<{}>\";reg-id={CELLULAR_REG_ID}",
            sip_instance
        ));
    }
    let visited_network = visited_network_override
        .map(str::to_string)
        .or_else(|| {
            profile
                .and_then(|profile| profile.ims.register.visited_network_header)
                .map(str::to_string)
        })
        .or_else(|| (profile.is_none() && policy.include_visited_network).then(String::new))
        .map(|value| {
            if value.is_empty() {
                format!("\"{}\"", identity.home_domain)
            } else {
                value
            }
        });
    crate::connectivity::core::register_message::build_register(&RegisterRequest {
        request_uri,
        advertised_route: *route,
        branch,
        from_uri: identity.public_uri.clone(),
        from_tag: ids.from_tag.clone(),
        to_value,
        call_id: ids.call_id.clone(),
        cseq: ids.cseq,
        headers: RegisterHeaderFields {
            authorization: authorization.map(str::to_string),
            contact,
            // REGISTER publishes the UE's MMTEL capability through the
            // Contact feature tags above. Accept-Contact is caller preference
            // for a target request and P-Preferred-Service selects the service
            // of that request; neither belongs on this registration binding.
            accept_contacts: Vec::new(),
            route: policy.include_route_header.then(|| {
                format!(
                    "<sip:{}:{};lr>",
                    sip_host(route.pcscf_addr.ip()),
                    route.pcscf_addr.port()
                )
            }),
            expires,
            supported: Some(params.supported_header),
            require_sec_agree: policy.require_sec_agree,
            proxy_require_sec_agree: policy.proxy_require_sec_agree,
            allow: Some(params.allow_header),
            preferred_service: None,
            preferred_identity: profile
                .is_none_or(|profile| profile.ims.register.include_p_preferred_identity)
                .then(|| format!("<{}>", identity.public_uri)),
            visited_network,
            access_network_info: params.pani,
            cellular_network_info: None,
            security_client: security_client.map(str::to_string),
            security_verify: security_verify.map(str::to_string),
            user_agent: params.user_agent,
        },
    })
}

/// Build a SIP MESSAGE carrying a 3GPP SMS RPDU body (MO submit).
#[allow(clippy::too_many_arguments)]
pub fn build_sms_message(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    request_uri: &str,
    to_uri: &str,
    body: &[u8],
    security_verify: Option<&str>,
) -> Vec<u8> {
    let branch = new_branch();
    let route_host = sip_host(route.pcscf_addr.ip());
    let call_id = format!("{}@simadmin", hex_token(16));
    let from_tag = hex_token(8);
    let to_value = format!("<{to_uri}>");
    let route_value = service_route
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("<sip:{route_host}:{};lr>", route.pcscf_addr.port()));
    let mut headers = vec![
        SipHeader::new("Route", route_value),
        SipHeader::new("P-Preferred-Identity", format!("<{}>", identity.public_uri)),
        SipHeader::new("P-Access-Network-Info", PANI_EUTRAN),
        SipHeader::new("P-Preferred-Service", SMS_ICSI),
    ];
    if let Some(sv) = security_verify {
        headers.push(SipHeader::new("Security-Verify", sv));
    }
    headers.push(SipHeader::new("Accept-Contact", "*;+g.3gpp.smsip"));
    headers.push(SipHeader::new("Accept", SMS_CONTENT_TYPE));
    headers.push(SipHeader::new("User-Agent", USER_AGENT));
    headers.push(SipHeader::new("Content-Type", SMS_CONTENT_TYPE));
    crate::connectivity::core::sip_message::build_message(&SipRequest {
        method: "MESSAGE",
        request_uri,
        route: *route,
        branch: &branch,
        from_uri: &identity.public_uri,
        from_tag: &from_tag,
        to_value: &to_value,
        call_id: &call_id,
        cseq: 1,
        headers: &headers,
        body,
    })
}

/// Build the RP-ACK MESSAGE sent back to the network for a received MT SMS.
/// The Request-URI/To are taken from the inbound MESSAGE `From` header.
#[allow(clippy::too_many_arguments)]
pub fn build_rp_ack(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    inbound_frame: &[u8],
    body: &[u8],
    fallback_uri: &str,
    security_verify: Option<&str>,
) -> Vec<u8> {
    let request_uri =
        sip_header_uri(inbound_frame, "From").unwrap_or_else(|| fallback_uri.to_string());
    build_sms_message(
        identity,
        route,
        service_route,
        &request_uri,
        &request_uri,
        body,
        security_verify,
    )
}

// ============================ Voice (INVITE dialog) ============================

/// Dialog identifiers for a voice call. Unlike the fire-and-forget MESSAGE
/// transaction, an INVITE dialog must keep a stable Call-ID + local tag across
/// INVITE/ACK/BYE, and learn the remote tag from the answer's To header.
#[derive(Debug, Clone)]
pub struct DialogIds {
    pub call_id: String,
    pub local_tag: String,
    pub remote_tag: Option<String>,
    pub cseq: u32,
}

impl DialogIds {
    /// Fresh dialog for a mobile-originated call.
    pub fn fresh() -> Self {
        Self {
            call_id: format!("{}@simadmin", hex_token(16)),
            local_tag: hex_token(8),
            remote_tag: None,
            cseq: 1,
        }
    }

    /// Record the remote (To) tag learned from a 2xx/early dialog response.
    pub fn set_remote_tag(&mut self, tag: impl Into<String>) {
        self.remote_tag = Some(tag.into());
    }
}

/// Build a SIP INVITE carrying an SDP audio offer (MO voice call).
///
/// `contact_host`/`contact_port` are where the peer should route in-dialog
/// requests (the device's signaling address). The SDP body is produced by the
/// shared voice layer (`build_mo_audio_offer_with_params(...).to_sdp()`).
#[allow(clippy::too_many_arguments)]
pub fn build_invite(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    dialog: &DialogIds,
    callee_uri: &str,
    sdp_offer: &[u8],
    security_verify: Option<&str>,
) -> Vec<u8> {
    build_invite_for_access_with_supported(
        identity,
        route,
        service_route,
        dialog,
        callee_uri,
        sdp_offer,
        security_verify,
        PANI_EUTRAN,
        USER_AGENT,
        "100rel, precondition",
    )
}

/// Build an initial INVITE with access-specific PANI and User-Agent values.
/// VoLTE uses [`build_invite`]; VoWiFi supplies its carrier profile's IEEE
/// 802.11 PANI while retaining the same stable dialog identifiers.
#[allow(clippy::too_many_arguments)]
pub fn build_invite_for_access(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    dialog: &DialogIds,
    callee_uri: &str,
    sdp_offer: &[u8],
    security_verify: Option<&str>,
    pani: &str,
    user_agent: &str,
) -> Vec<u8> {
    build_invite_for_access_with_supported(
        identity,
        route,
        service_route,
        dialog,
        callee_uri,
        sdp_offer,
        security_verify,
        pani,
        user_agent,
        "100rel, timer",
    )
}

#[allow(clippy::too_many_arguments)]
fn build_invite_for_access_with_supported(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    dialog: &DialogIds,
    callee_uri: &str,
    sdp_offer: &[u8],
    security_verify: Option<&str>,
    pani: &str,
    user_agent: &str,
    supported: &str,
) -> Vec<u8> {
    let branch = new_branch();
    let local_host = sip_host(route.local_addr.ip());
    let local_port = route.local_addr.port();
    let to_value = format!("<{callee_uri}>");
    let mut headers = vec![
        SipHeader::new("Route", route_header_value(route, service_route)),
        SipHeader::new(
            "Contact",
            format!(
                "<sip:{}@{}:{};transport={}>;+g.3gpp.icsi-ref=\"{}\"",
                identity.contact_user,
                local_host,
                local_port,
                route.transport.as_param(),
                MMTEL_ICSI_REF,
            ),
        ),
        SipHeader::new("P-Preferred-Identity", format!("<{}>", identity.public_uri)),
        SipHeader::new("P-Access-Network-Info", pani),
        SipHeader::new("P-Preferred-Service", MMTEL_ICSI),
        SipHeader::new(
            "Accept-Contact",
            format!("*;+g.3gpp.icsi-ref=\"{MMTEL_ICSI_REF}\""),
        ),
        SipHeader::new(
            "Allow",
            "INVITE,ACK,CANCEL,BYE,UPDATE,PRACK,MESSAGE,REFER,NOTIFY,INFO,OPTIONS",
        ),
        SipHeader::new("Supported", supported),
    ];
    if let Some(sv) = security_verify {
        headers.push(SipHeader::new("Security-Verify", sv));
    }
    headers.push(SipHeader::new("User-Agent", user_agent));
    headers.push(SipHeader::new("Content-Type", "application/sdp"));
    crate::connectivity::core::sip_message::build_invite(&SipRequest {
        method: "INVITE",
        request_uri: callee_uri,
        route: *route,
        branch: &branch,
        from_uri: &identity.public_uri,
        from_tag: &dialog.local_tag,
        to_value: &to_value,
        call_id: &dialog.call_id,
        cseq: dialog.cseq,
        headers: &headers,
        body: sdp_offer,
    })
}

/// Build an in-dialog **re-INVITE** to renegotiate media on an *already
/// established* call — this is how VoLTE⇄ViLTE switching works (add or drop the
/// `m=video` section mid-call), exactly like "turn video on/off during a call"
/// on a smartphone.
///
/// Differences from the initial [`build_invite`] (RFC 3261 §14):
///   - It is sent **within the confirmed dialog**, so `To` carries the learned
///     `remote_tag` (a re-INVITE is not a dialog-creating request).
///   - `CSeq` is the next value in the dialog (the caller bumps `dialog.cseq`
///     before calling; we assert the remote tag is present).
///   - The SDP offer is the *new* media description (audio-only to downgrade
///     ViLTE→VoLTE, or audio+video to upgrade VoLTE→ViLTE).
///
/// The far end answers with a 200 OK (new SDP answer), which the caller ACKs via
/// [`build_ack`]. If the peer rejects the change it returns e.g. 488 Not
/// Acceptable Here and the *existing* media continues unchanged.
#[allow(clippy::too_many_arguments)]
pub fn build_reinvite(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    dialog: &DialogIds,
    callee_uri: &str,
    sdp_offer: &[u8],
    security_verify: Option<&str>,
) -> Vec<u8> {
    build_reinvite_for_access(
        identity,
        route,
        service_route,
        dialog,
        callee_uri,
        sdp_offer,
        security_verify,
        PANI_EUTRAN,
        USER_AGENT,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_reinvite_for_access(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    dialog: &DialogIds,
    callee_uri: &str,
    sdp_offer: &[u8],
    security_verify: Option<&str>,
    pani: &str,
    user_agent: &str,
) -> Vec<u8> {
    let branch = new_branch();
    let local_host = sip_host(route.local_addr.ip());
    let local_port = route.local_addr.port();
    // In-dialog: To MUST carry the remote tag. If it is somehow absent we still
    // emit a tagless To (degrades to initial-INVITE semantics) rather than panic.
    let to = match &dialog.remote_tag {
        Some(tag) => format!("<{callee_uri}>;tag={tag}"),
        None => format!("<{callee_uri}>"),
    };
    let mut headers = vec![
        SipHeader::new("Route", route_header_value(route, service_route)),
        SipHeader::new(
            "Contact",
            format!(
                "<sip:{}@{}:{};transport={}>;+g.3gpp.icsi-ref=\"{}\"",
                identity.contact_user,
                local_host,
                local_port,
                route.transport.as_param(),
                MMTEL_ICSI_REF,
            ),
        ),
        SipHeader::new("P-Preferred-Identity", format!("<{}>", identity.public_uri)),
        SipHeader::new("P-Access-Network-Info", pani),
        SipHeader::new("P-Preferred-Service", MMTEL_ICSI),
        SipHeader::new(
            "Accept-Contact",
            format!("*;+g.3gpp.icsi-ref=\"{MMTEL_ICSI_REF}\""),
        ),
        SipHeader::new(
            "Allow",
            "INVITE,ACK,CANCEL,BYE,UPDATE,PRACK,MESSAGE,REFER,NOTIFY,INFO,OPTIONS",
        ),
        SipHeader::new("Supported", "100rel, precondition"),
    ];
    if let Some(sv) = security_verify {
        headers.push(SipHeader::new("Security-Verify", sv));
    }
    headers.push(SipHeader::new("User-Agent", user_agent));
    headers.push(SipHeader::new("Content-Type", "application/sdp"));
    crate::connectivity::core::sip_message::build_invite(&SipRequest {
        method: "INVITE",
        request_uri: callee_uri,
        route: *route,
        branch: &branch,
        from_uri: &identity.public_uri,
        from_tag: &dialog.local_tag,
        to_value: &to,
        call_id: &dialog.call_id,
        cseq: dialog.cseq,
        headers: &headers,
        body: sdp_offer,
    })
}

/// Build the ACK for a 2xx INVITE response (confirms the dialog). Uses the
/// remote tag learned from the 200 OK To header. Per RFC 3261 the ACK for a 2xx
/// is a separate transaction and carries the same CSeq number as the INVITE.
pub fn build_ack(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    dialog: &DialogIds,
    callee_uri: &str,
) -> Vec<u8> {
    build_ack_for_access(
        identity,
        route,
        service_route,
        dialog,
        callee_uri,
        USER_AGENT,
    )
}

pub fn build_ack_for_access(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    dialog: &DialogIds,
    callee_uri: &str,
    user_agent: &str,
) -> Vec<u8> {
    let branch = new_branch();
    let to = match &dialog.remote_tag {
        Some(tag) => format!("<{callee_uri}>;tag={tag}"),
        None => format!("<{callee_uri}>"),
    };
    let headers = [
        SipHeader::new("Route", route_header_value(route, service_route)),
        SipHeader::new("User-Agent", user_agent),
    ];
    crate::connectivity::core::sip_message::build_ack(&SipRequest {
        method: "ACK",
        request_uri: callee_uri,
        route: *route,
        branch: &branch,
        from_uri: &identity.public_uri,
        from_tag: &dialog.local_tag,
        to_value: &to,
        call_id: &dialog.call_id,
        cseq: dialog.cseq,
        headers: &headers,
        body: &[],
    })
}

/// Build PRACK for a reliable provisional response (RFC 3262). The dialog must
/// already contain the To-tag learned from the 18x response. `cseq` is the next
/// local in-dialog sequence while `invite_cseq` remains the INVITE sequence
/// referenced by RAck.
#[allow(clippy::too_many_arguments)]
pub fn build_prack(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    dialog: &DialogIds,
    callee_uri: &str,
    cseq: u32,
    rseq: u32,
    invite_cseq: u32,
) -> Vec<u8> {
    build_prack_for_access(
        identity,
        route,
        service_route,
        dialog,
        callee_uri,
        cseq,
        rseq,
        invite_cseq,
        USER_AGENT,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_prack_for_access(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    dialog: &DialogIds,
    callee_uri: &str,
    cseq: u32,
    rseq: u32,
    invite_cseq: u32,
    user_agent: &str,
) -> Vec<u8> {
    let branch = new_branch();
    let to = match &dialog.remote_tag {
        Some(tag) => format!("<{callee_uri}>;tag={tag}"),
        None => format!("<{callee_uri}>"),
    };
    let headers = [
        SipHeader::new("Route", route_header_value(route, service_route)),
        SipHeader::new("RAck", format!("{rseq} {invite_cseq} INVITE")),
        SipHeader::new("User-Agent", user_agent),
    ];
    crate::connectivity::core::sip_message::build_request(&SipRequest {
        method: "PRACK",
        request_uri: callee_uri,
        route: *route,
        branch: &branch,
        from_uri: &identity.public_uri,
        from_tag: &dialog.local_tag,
        to_value: &to,
        call_id: &dialog.call_id,
        cseq,
        headers: &headers,
        body: &[],
    })
}

/// Build a BYE to tear down a confirmed dialog. CSeq must be incremented past
/// the INVITE (the caller passes the next value).
pub fn build_bye(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    dialog: &DialogIds,
    callee_uri: &str,
    cseq: u32,
) -> Vec<u8> {
    build_bye_for_access(
        identity,
        route,
        service_route,
        dialog,
        callee_uri,
        cseq,
        USER_AGENT,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_bye_for_access(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    dialog: &DialogIds,
    callee_uri: &str,
    cseq: u32,
    user_agent: &str,
) -> Vec<u8> {
    let branch = new_branch();
    let local_host = sip_host(route.local_addr.ip());
    let local_port = route.local_addr.port();
    let to = match &dialog.remote_tag {
        Some(tag) => format!("<{callee_uri}>;tag={tag}"),
        None => format!("<{callee_uri}>"),
    };

    let mut h = String::new();
    h.push_str(&format!("BYE {callee_uri} SIP/2.0\r\n"));
    h.push_str(&format!(
        "Via: {} {local_host}:{local_port};branch={branch};rport\r\n",
        route.transport.as_via()
    ));
    h.push_str("Max-Forwards: 70\r\n");
    h.push_str(&format!(
        "Route: {}\r\n",
        route_header_value(route, service_route)
    ));
    h.push_str(&format!(
        "From: <{}>;tag={}\r\n",
        identity.public_uri, dialog.local_tag
    ));
    h.push_str(&format!("To: {to}\r\n"));
    h.push_str(&format!("Call-ID: {}\r\n", dialog.call_id));
    h.push_str(&format!("CSeq: {cseq} BYE\r\n"));
    h.push_str(&format!("User-Agent: {user_agent}\r\n"));
    h.push_str("Content-Length: 0\r\n\r\n");
    h.into_bytes()
}

/// Build an out-of-dialog OPTIONS request used to verify that the registered
/// cellular binding is still reachable.  The request deliberately carries the
/// current Service-Route/security binding, but no digest credentials: a
/// registrar may answer 401/403 and that is still useful evidence that the
/// SIP path is alive; only transport timeout is treated as a dead leg.
pub fn build_options(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    cseq: u32,
    security_verify: Option<&str>,
) -> Vec<u8> {
    use crate::connectivity::core::sip_message::build_request;

    let ids = RequestIds::fresh(cseq);
    let route_header = route_header_value(route, service_route);
    let to = format!("<{}>", identity.public_uri);
    let mut headers = vec![
        SipHeader::new("Route", route_header),
        SipHeader::new("P-Preferred-Identity", format!("<{}>", identity.public_uri)),
        SipHeader::new("P-Access-Network-Info", PANI_EUTRAN),
        SipHeader::new("Accept", "application/sdp"),
    ];
    if let Some(value) = security_verify {
        headers.push(SipHeader::new("Security-Verify", value));
    }
    headers.push(SipHeader::new("User-Agent", USER_AGENT));
    build_request(&SipRequest {
        method: "OPTIONS",
        request_uri: &identity.public_uri,
        route: *route,
        branch: &new_branch(),
        from_uri: &identity.public_uri,
        from_tag: &ids.from_tag,
        to_value: &to,
        call_id: &ids.call_id,
        cseq: ids.cseq,
        headers: &headers,
        body: &[],
    })
}

/// Build an in-dialog SIP INFO carrying one DTMF digit. This is the signaling
/// fallback when the operator dialog did not negotiate RFC 4733
/// `telephone-event`, or when Asterisk explicitly delivered DTMF via INFO.
/// RTP telephone-event remains preferred because it stays on the media path.
pub fn build_dtmf_info(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    dialog: &DialogIds,
    callee_uri: &str,
    cseq: u32,
    digit: char,
    duration_ms: u16,
) -> Result<Vec<u8>, VolteError> {
    build_dtmf_info_for_access(
        identity,
        route,
        service_route,
        dialog,
        callee_uri,
        cseq,
        digit,
        duration_ms,
        PANI_EUTRAN,
        USER_AGENT,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_dtmf_info_for_access(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    dialog: &DialogIds,
    callee_uri: &str,
    cseq: u32,
    digit: char,
    duration_ms: u16,
    pani: &str,
    user_agent: &str,
) -> Result<Vec<u8>, VolteError> {
    let digit = digit.to_ascii_uppercase();
    if !matches!(digit, '0'..='9' | '*' | '#' | 'A'..='D') {
        return Err(VolteError::new("volte_dtmf_digit_invalid"));
    }
    if !(40..=5000).contains(&duration_ms) {
        return Err(VolteError::new("volte_dtmf_duration_invalid"));
    }
    let branch = new_branch();
    let local_host = sip_host(route.local_addr.ip());
    let local_port = route.local_addr.port();
    let to = match &dialog.remote_tag {
        Some(tag) => format!("<{callee_uri}>;tag={tag}"),
        None => format!("<{callee_uri}>"),
    };
    let body = format!("Signal={digit}\r\nDuration={duration_ms}\r\n");
    let mut h = String::new();
    h.push_str(&format!("INFO {callee_uri} SIP/2.0\r\n"));
    h.push_str(&format!(
        "Via: {} {local_host}:{local_port};branch={branch};rport\r\n",
        route.transport.as_via()
    ));
    h.push_str("Max-Forwards: 70\r\n");
    h.push_str(&format!(
        "Route: {}\r\n",
        route_header_value(route, service_route)
    ));
    h.push_str(&format!(
        "From: <{}>;tag={}\r\n",
        identity.public_uri, dialog.local_tag
    ));
    h.push_str(&format!("To: {to}\r\n"));
    h.push_str(&format!("Call-ID: {}\r\n", dialog.call_id));
    h.push_str(&format!("CSeq: {cseq} INFO\r\n"));
    h.push_str(&format!("P-Access-Network-Info: {pani}\r\n"));
    h.push_str(&format!("User-Agent: {user_agent}\r\n"));
    h.push_str(&format!("Content-Type: {DTMF_RELAY_CONTENT_TYPE}\r\n"));
    h.push_str("Content-Disposition: signal;handling=optional\r\n");
    h.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    h.push_str(&body);
    Ok(h.into_bytes())
}

/// Build a CANCEL for a not-yet-answered INVITE. Per RFC 3261 the CANCEL copies
/// the INVITE's Call-ID/From/To/CSeq-number (method CANCEL) and top Via branch.
pub fn build_cancel(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    dialog: &DialogIds,
    callee_uri: &str,
    invite_branch: &str,
) -> Vec<u8> {
    build_cancel_for_access(
        identity,
        route,
        service_route,
        dialog,
        callee_uri,
        invite_branch,
        USER_AGENT,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_cancel_for_access(
    identity: &ImsIdentity,
    route: &SipRoute,
    service_route: Option<&str>,
    dialog: &DialogIds,
    callee_uri: &str,
    invite_branch: &str,
    user_agent: &str,
) -> Vec<u8> {
    let local_host = sip_host(route.local_addr.ip());
    let local_port = route.local_addr.port();

    let mut h = String::new();
    h.push_str(&format!("CANCEL {callee_uri} SIP/2.0\r\n"));
    // CANCEL MUST carry the same top Via branch as the INVITE it cancels.
    h.push_str(&format!(
        "Via: {} {local_host}:{local_port};branch={invite_branch};rport\r\n",
        route.transport.as_via()
    ));
    h.push_str("Max-Forwards: 70\r\n");
    h.push_str(&format!(
        "Route: {}\r\n",
        route_header_value(route, service_route)
    ));
    h.push_str(&format!(
        "From: <{}>;tag={}\r\n",
        identity.public_uri, dialog.local_tag
    ));
    h.push_str(&format!("To: <{callee_uri}>\r\n"));
    h.push_str(&format!("Call-ID: {}\r\n", dialog.call_id));
    h.push_str(&format!("CSeq: {} CANCEL\r\n", dialog.cseq));
    h.push_str(&format!("User-Agent: {user_agent}\r\n"));
    h.push_str("Content-Length: 0\r\n\r\n");
    h.into_bytes()
}

/// Build a response to an inbound request (e.g. 200 OK to an MT INVITE, or 486
/// Busy Here). Echoes Via/From/To/Call-ID/CSeq from the request per RFC 3261,
/// adds our tag to To, and optionally attaches an SDP answer body.
#[allow(clippy::too_many_arguments)]
pub fn build_response(
    request: &[u8],
    status: u16,
    reason: &str,
    local_tag: Option<&str>,
    contact: Option<&str>,
    sdp_answer: Option<&[u8]>,
) -> Vec<u8> {
    build_response_for_access(
        request, status, reason, local_tag, contact, sdp_answer, USER_AGENT,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_response_for_access(
    request: &[u8],
    status: u16,
    reason: &str,
    local_tag: Option<&str>,
    contact: Option<&str>,
    sdp_answer: Option<&[u8]>,
    user_agent: &str,
) -> Vec<u8> {
    let mut h = String::new();
    h.push_str(&format!("SIP/2.0 {status} {reason}\r\n"));
    // Echo all Via headers in order (RFC 3261 §8.2.6.2).
    for via in header_values(request, "Via") {
        h.push_str(&format!("Via: {via}\r\n"));
    }
    for record_route in header_values(request, "Record-Route") {
        h.push_str(&format!("Record-Route: {record_route}\r\n"));
    }
    if let Some(from) = header_value(request, "From") {
        h.push_str(&format!("From: {from}\r\n"));
    }
    // Add our tag to To if the request's To has none and we supply one.
    if let Some(to) = header_value(request, "To") {
        if let Some(tag) = local_tag {
            if !to.contains(";tag=") {
                h.push_str(&format!("To: {to};tag={tag}\r\n"));
            } else {
                h.push_str(&format!("To: {to}\r\n"));
            }
        } else {
            h.push_str(&format!("To: {to}\r\n"));
        }
    }
    if let Some(call_id) = header_value(request, "Call-ID") {
        h.push_str(&format!("Call-ID: {call_id}\r\n"));
    }
    if let Some(cseq) = header_value(request, "CSeq") {
        h.push_str(&format!("CSeq: {cseq}\r\n"));
    }
    if let Some(contact) = contact {
        h.push_str(&format!("Contact: <{contact}>\r\n"));
    }
    h.push_str(&format!("User-Agent: {user_agent}\r\n"));
    match sdp_answer {
        Some(body) => {
            h.push_str("Content-Type: application/sdp\r\n");
            h.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
            let mut frame = h.into_bytes();
            frame.extend_from_slice(body);
            frame
        }
        None => {
            h.push_str("Content-Length: 0\r\n\r\n");
            h.into_bytes()
        }
    }
}

// SIP framing/parsing primitives are shared with every IMS leg — they live in
// `crate::connectivity::core::sip_frame` (single implementation). The wrappers below keep the
// volte-facing names/signatures (e.g. `parse_status` returning `VolteError`) so
// existing volte call sites are unchanged.

/// Parse the SIP status code (delegates to shared framing; remaps the error).
pub fn parse_status(frame: &[u8]) -> Result<u16, VolteError> {
    crate::connectivity::core::sip_frame::parse_status(frame)
        .map_err(|_| VolteError::new("volte_sip_status_invalid"))
}

/// Everything after the header terminator (may be empty).
pub fn sip_body(frame: &[u8]) -> &[u8] {
    crate::connectivity::core::sip_frame::body(frame)
}

/// TCP de-coalescing: exact byte length of one complete SIP message, or None.
pub fn complete_frame_len(buf: &[u8]) -> Option<usize> {
    crate::connectivity::core::sip_frame::complete_frame_len(buf)
}

pub fn is_complete(buf: &[u8]) -> bool {
    crate::connectivity::core::sip_frame::is_complete(buf)
}

/// Whether a frame is a SIP request for the given method (start line check).
pub fn is_request(frame: &[u8], method: &str) -> bool {
    crate::connectivity::core::sip_frame::is_request(frame, method)
}

/// Collect all values of a header (case-insensitive name, first-colon split).
pub fn header_values(frame: &[u8], header_name: &str) -> Vec<String> {
    crate::connectivity::core::sip_frame::header_values(frame, header_name)
}

/// First value of a header, if present.
pub fn header_value(frame: &[u8], header_name: &str) -> Option<String> {
    crate::connectivity::core::sip_frame::header_value(frame, header_name)
}

/// Extract the bracketed `<sip:...>` URI from a named header value.
pub fn sip_header_uri(frame: &[u8], header_name: &str) -> Option<String> {
    crate::connectivity::core::sip_frame::header_uri(frame, header_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    fn ident() -> ImsIdentity {
        ImsIdentity {
            private_user: "460001234567890@ims.mnc000.mcc460.3gppnetwork.org".to_string(),
            public_uri: "sip:460001234567890@ims.mnc000.mcc460.3gppnetwork.org".to_string(),
            contact_user: "460001234567890".to_string(),
            home_domain: "ims.mnc000.mcc460.3gppnetwork.org".to_string(),
            contact_user_phone: false,
        }
    }

    fn route_udp() -> SipRoute {
        SipRoute {
            local_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), 5060),
            pcscf_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 5060),
            transport: SipTransport::Udp,
        }
    }

    #[test]
    fn sip_host_brackets_ipv6_only() {
        assert_eq!(sip_host(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))), "1.2.3.4");
        assert_eq!(sip_host(IpAddr::V6(Ipv6Addr::LOCALHOST)), "[::1]");
    }

    #[test]
    fn catalog_register_request_uri_policy_uses_the_actual_route() {
        let route = route_udp();
        let mut profile = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;

        profile.ims.register.request_uri_policy = "home_domain";
        assert_eq!(
            register_request_uri(&profile, &route),
            format!("sip:{}", profile.ims.domain)
        );

        profile.ims.register.request_uri_policy = "registrar";
        profile.ims.registrar = Some("sip:registrar.example:5070");
        assert_eq!(
            register_request_uri(&profile, &route),
            "sip:registrar.example:5070"
        );

        profile.ims.register.request_uri_policy = "pcscf";
        assert_eq!(register_request_uri(&profile, &route), "sip:10.0.0.1:5060");
    }

    #[test]
    fn effective_register_target_overrides_addressing_but_keeps_catalog_policy() {
        let route = route_udp();
        let mut profile = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
        profile.ims.register.request_uri_policy = "registrar";
        let target = RegisterTarget {
            domain: "ims.override.example",
            realm: "realm.override.example",
            registrar: Some("sip:registrar.override.example:5070"),
        };
        assert_eq!(
            register_request_uri_with_target(&profile, target, &route),
            "sip:registrar.override.example:5070"
        );

        let identity = ImsIdentity {
            private_user: "460001234567890@realm.override.example".to_string(),
            public_uri: "sip:460001234567890@ims.override.example".to_string(),
            contact_user: "460001234567890".to_string(),
            home_domain: "ims.override.example".to_string(),
            contact_user_phone: false,
        };
        let frame = build_register_from_profile_with_target(
            &profile,
            target,
            RegisterPhase::Initial,
            &identity,
            &route,
            &RequestIds::fresh(1),
            profile.ims.register.expires_seconds,
            None,
            None,
            None,
            "urn:uuid:test",
            RegisterRequestPolicy::LEGACY,
        );
        let frame = String::from_utf8(frame).expect("REGISTER is UTF-8");
        assert!(frame.starts_with("REGISTER sip:registrar.override.example:5070 SIP/2.0\r\n"));
        assert!(frame.contains("From: <sip:460001234567890@ims.override.example>"));
        assert_eq!(
            profile.ims.user_agent,
            header_value(frame.as_bytes(), "User-Agent").unwrap()
        );
    }

    #[test]
    fn roaming_register_overrides_static_visited_network_only_for_this_request() {
        let profile = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
        let frame = build_register_from_profile_with_target_and_visited(
            &profile,
            RegisterTarget::from_profile(&profile),
            RegisterPhase::Initial,
            &ident(),
            &route_udp(),
            &RequestIds::fresh(1),
            profile.ims.register.expires_seconds,
            None,
            None,
            None,
            "urn:uuid:test",
            RegisterRequestPolicy {
                include_visited_network: true,
                ..RegisterRequestPolicy::LEGACY
            },
            Some("\"ims.mnc000.mcc460.3gppnetwork.org\""),
        );
        assert_eq!(
            header_value(&frame, "P-Visited-Network-ID").as_deref(),
            Some("\"ims.mnc000.mcc460.3gppnetwork.org\"")
        );
        assert_eq!(
            profile.ims.register.visited_network_header,
            Some("\"legacy-test-profile\"")
        );
    }

    #[test]
    fn carrier_policy_adds_imei_sip_instance_and_reg_id_to_contact() {
        let mut profile = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
        profile.ims.register.always_add_sip_instance = true;
        let frame = build_register_from_profile(
            &profile,
            RegisterPhase::Initial,
            &ident(),
            &route_udp(),
            &RequestIds::fresh(1),
            profile.ims.register.expires_seconds,
            None,
            None,
            None,
            "urn:imei:490154203237518",
            RegisterRequestPolicy::LEGACY,
        );
        let text = String::from_utf8(frame).unwrap();
        assert!(text.contains(&format!(
            ";+sip.instance=\"<urn:imei:490154203237518>\";reg-id={CELLULAR_REG_ID}"
        )));
    }

    #[test]
    fn cellular_reg_id_never_equals_the_wlan_leg() {
        // The two legs share one +sip.instance now, so RFC 5626 §6 keys their
        // bindings on (AOR, instance-id, reg-id) alone. If this ever collides,
        // whichever leg registers second replaces the other's binding while our
        // runtime still reports both as registered.
        use crate::connectivity::core::ims_access::ImsAccess;
        assert_eq!(CELLULAR_REG_ID, ImsAccess::Cellular.reg_id());
        assert_ne!(CELLULAR_REG_ID, ImsAccess::Wlan.reg_id());
    }

    #[test]
    fn catalog_video_contact_feature_requires_local_capability() {
        let mut profile = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
        profile.ims.register.contact_param_order = &["audio", "video", "+g.3gpp.smsip"];
        let build = |include_video_feature| {
            build_register_from_profile(
                &profile,
                RegisterPhase::Initial,
                &ident(),
                &route_udp(),
                &RequestIds::fresh(1),
                profile.ims.register.expires_seconds,
                None,
                None,
                None,
                "urn:uuid:test",
                RegisterRequestPolicy {
                    include_video_feature,
                    ..RegisterRequestPolicy::LEGACY
                },
            )
        };
        let disabled = String::from_utf8(build(false)).unwrap();
        assert!(disabled.contains(";audio"));
        assert!(!disabled.contains(";video"));
        let enabled = String::from_utf8(build(true)).unwrap();
        assert!(enabled.contains(";audio;video;+g.3gpp.smsip"));
        assert_eq!(
            enabled
                .to_ascii_lowercase()
                .matches("+g.3gpp.smsip")
                .count(),
            1
        );
    }

    #[test]
    fn catalog_profile_without_contact_parameters_still_advertises_mmtel() {
        // Regression: a profile with an empty contact_param_order used to skip
        // both Contact arms, emitting a bare
        // `<sip:...;transport=udp>;+g.3gpp.smsip`. Without +g.3gpp.icsi-ref the
        // S-CSCF does not treat the registration as MMTEL voice capable and MT
        // calls are never delivered, even though REGISTER answers 200 OK.
        let mut profile = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
        profile.ims.register.contact_param_order = &[];
        let frame = build_register_from_profile(
            &profile,
            RegisterPhase::Initial,
            &ident(),
            &route_udp(),
            &RequestIds::fresh(1),
            profile.ims.register.expires_seconds,
            None,
            None,
            None,
            "urn:uuid:test",
            RegisterRequestPolicy {
                include_mmtel_features: true,
                ..RegisterRequestPolicy::LEGACY
            },
        );
        let text = String::from_utf8(frame).unwrap();
        let contact = header_value(text.as_bytes(), "Contact").unwrap();
        assert!(contact.contains(";audio"));
        assert!(contact.contains(&format!(";+g.3gpp.icsi-ref=\"{MMTEL_ICSI_REF}\"")));
        assert!(contact.contains(";+sip.instance=\"<urn:uuid:test>\""));
        assert!(contact.contains(";expires="));
        assert_eq!(
            contact
                .to_ascii_lowercase()
                .matches("+g.3gpp.smsip")
                .count(),
            1
        );
    }

    #[test]
    fn mmtel_register_keeps_lte_pani_across_registration_phases() {
        let mut profile = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
        profile.ims.register.include_pani_initial = false;
        profile.ims.register.include_pani_authenticated = false;
        profile.ims.register.access_network_info = "3GPP-E-UTRAN-FDD";

        for phase in [
            RegisterPhase::Initial,
            RegisterPhase::Authenticated,
            RegisterPhase::Refresh,
        ] {
            let frame = build_register_from_profile(
                &profile,
                phase,
                &ident(),
                &route_udp(),
                &RequestIds::fresh(1),
                profile.ims.register.expires_seconds,
                None,
                None,
                None,
                "urn:uuid:test",
                RegisterRequestPolicy {
                    include_mmtel_features: true,
                    ..RegisterRequestPolicy::LEGACY
                },
            );
            assert_eq!(
                header_value(&frame, "P-Access-Network-Info").as_deref(),
                Some("3GPP-E-UTRAN-FDD"),
                "missing PANI during {phase:?} REGISTER"
            );
        }
    }

    #[test]
    fn protected_reregistration_repeats_security_client_and_verify() {
        let profile = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
        let frame = build_register_from_profile(
            &profile,
            RegisterPhase::Refresh,
            &ident(),
            &route_udp(),
            &RequestIds::fresh(1),
            profile.ims.register.expires_seconds,
            Some("Digest username=\"impi\", realm=\"ims.example\", uri=\"sip:ims.example\""),
            Some("ipsec-3gpp;alg=hmac-md5-96;ealg=null;spi-c=1;spi-s=2;port-c=6000;port-s=6001"),
            Some("ipsec-3gpp;alg=hmac-md5-96;ealg=null;spi-c=1;spi-s=2;port-c=6000;port-s=6001"),
            "urn:uuid:test",
            RegisterRequestPolicy {
                advertise_sec_agree: true,
                require_sec_agree: true,
                proxy_require_sec_agree: true,
                include_mmtel_features: true,
                ..RegisterRequestPolicy::LEGACY
            },
        );
        let text = String::from_utf8(frame).unwrap();
        assert!(text.contains("Security-Client: ipsec-3gpp;"));
        assert!(text.contains("Security-Verify: ipsec-3gpp;"));
        assert!(text.contains("Require: sec-agree\r\n"));
        assert!(text.contains("Proxy-Require: sec-agree\r\n"));
    }

    #[test]
    fn non_mmtel_register_still_obeys_carrier_pani_policy() {
        let mut profile = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
        profile.ims.register.include_pani_initial = false;
        profile.ims.register.include_pani_authenticated = false;

        let frame = build_register_from_profile(
            &profile,
            RegisterPhase::Initial,
            &ident(),
            &route_udp(),
            &RequestIds::fresh(1),
            profile.ims.register.expires_seconds,
            None,
            None,
            None,
            "urn:uuid:test",
            RegisterRequestPolicy::LEGACY,
        );
        assert!(header_value(&frame, "P-Access-Network-Info").is_none());
        assert!(header_value(&frame, "Allow").is_none());
    }

    #[test]
    fn empty_contact_parameters_keep_sip_instance_single_with_reg_id() {
        // always_add_sip_instance must not append a second +sip.instance when
        // the fallback arm already emitted one.
        let mut profile = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
        profile.ims.register.contact_param_order = &[];
        profile.ims.register.always_add_sip_instance = true;
        let frame = build_register_from_profile(
            &profile,
            RegisterPhase::Initial,
            &ident(),
            &route_udp(),
            &RequestIds::fresh(1),
            profile.ims.register.expires_seconds,
            None,
            None,
            None,
            "urn:imei:490154203237518",
            RegisterRequestPolicy {
                include_mmtel_features: true,
                ..RegisterRequestPolicy::LEGACY
            },
        );
        let text = String::from_utf8(frame).unwrap();
        let contact = header_value(text.as_bytes(), "Contact").unwrap();
        assert_eq!(
            contact
                .to_ascii_lowercase()
                .matches("+sip.instance")
                .count(),
            1
        );
        assert_eq!(
            contact
                .matches(&format!(";reg-id={CELLULAR_REG_ID}"))
                .count(),
            1
        );
    }

    #[test]
    fn explicit_contact_parameters_are_not_widened_by_the_fallback() {
        // A bundle that spells out its Contact list keeps expressing the whole
        // opinion: no accesstype, no icsi-ref, no expires get bolted on.
        let mut profile = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
        profile.ims.register.contact_param_order = &["+g.3gpp.mid-call"];
        let frame = build_register_from_profile(
            &profile,
            RegisterPhase::Initial,
            &ident(),
            &route_udp(),
            &RequestIds::fresh(1),
            profile.ims.register.expires_seconds,
            None,
            None,
            None,
            "urn:uuid:test",
            RegisterRequestPolicy {
                include_mmtel_features: true,
                ..RegisterRequestPolicy::LEGACY
            },
        );
        let text = String::from_utf8(frame).unwrap();
        let contact = header_value(text.as_bytes(), "Contact").unwrap();
        assert!(contact.contains(";+g.3gpp.mid-call;+g.3gpp.smsip"));
        assert!(!contact.contains("+g.3gpp.icsi-ref"));
        assert!(!contact.contains("+g.3gpp.accesstype"));
        assert!(!contact.contains(";expires="));
    }

    #[test]
    fn carrier_register_adds_missing_sms_over_ip_feature_tag_once() {
        let mut profile = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
        profile.ims.register.contact_param_order = &["+g.3gpp.mid-call"];
        let frame = build_register_from_profile(
            &profile,
            RegisterPhase::Initial,
            &ident(),
            &route_udp(),
            &RequestIds::fresh(1),
            profile.ims.register.expires_seconds,
            None,
            None,
            None,
            "urn:uuid:test",
            RegisterRequestPolicy::LEGACY,
        );
        let text = String::from_utf8(frame).unwrap();
        let contact = header_value(text.as_bytes(), "Contact").unwrap();
        assert!(contact.contains(";+g.3gpp.mid-call;+g.3gpp.smsip"));
        assert_eq!(
            contact
                .to_ascii_lowercase()
                .matches("+g.3gpp.smsip")
                .count(),
            1
        );
    }

    #[test]
    fn carrier_register_recognizes_existing_sms_feature_tag_case_insensitively() {
        let mut profile = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
        profile.ims.register.contact_param_order = &["audio", "+G.3GPP.SMSIP"];
        let frame = build_register_from_profile(
            &profile,
            RegisterPhase::Initial,
            &ident(),
            &route_udp(),
            &RequestIds::fresh(1),
            profile.ims.register.expires_seconds,
            None,
            None,
            None,
            "urn:uuid:test",
            RegisterRequestPolicy::LEGACY,
        );
        let text = String::from_utf8(frame).unwrap();
        let contact = header_value(text.as_bytes(), "Contact").unwrap();
        assert_eq!(
            contact
                .to_ascii_lowercase()
                .matches("+g.3gpp.smsip")
                .count(),
            1
        );
    }

    #[test]
    fn register_contains_mandatory_headers_and_sms_feature_tag() {
        let ids = RequestIds::fresh(1);
        let frame = build_register(
            &ident(),
            &route_udp(),
            &ids,
            3600,
            None,
            Some("ipsec-3gpp; alg=hmac-md5-96; spi-c=1; spi-s=2; port-c=6000; port-s=6001"),
            None,
            "urn:uuid:00000000-0000-4000-8000-000000000000",
        );
        let text = String::from_utf8(frame).unwrap();
        assert!(text.starts_with("REGISTER sip:ims.mnc000.mcc460.3gppnetwork.org SIP/2.0\r\n"));
        assert!(text.contains("Via: SIP/2.0/UDP 10.0.0.2:5060;branch=z9hG4bK"));
        assert!(text.contains("CSeq: 1 REGISTER\r\n"));
        assert!(text.contains("+g.3gpp.smsip"));
        assert!(text.contains("Require: sec-agree\r\n"));
        assert!(text.contains("Security-Client: ipsec-3gpp"));
        assert!(text.contains("P-Access-Network-Info: 3GPP-E-UTRAN-FDD\r\n"));
        assert!(!text.contains("P-Preferred-Service:"));
        assert!(!text.contains("\r\nAccept: application/vnd.3gpp.sms\r\n"));
        assert!(text.ends_with("Content-Length: 0\r\n\r\n"));
    }

    #[test]
    fn legacy_register_matches_the_reference_sms_sec_agree_headers() {
        let frame = build_register_with_policy(
            &ident(),
            &route_udp(),
            &RequestIds::fresh(1),
            3600,
            None,
            Some("ipsec-3gpp"),
            None,
            "urn:uuid:00000000-0000-4000-8000-000000000000",
            RegisterRequestPolicy::LEGACY,
        );
        let text = String::from_utf8(frame).unwrap();

        assert!(text.contains("Supported: path, gruu\r\n"));
        assert!(!text.contains("Supported: path, gruu, sec-agree\r\n"));
        assert!(text.contains("Require: sec-agree\r\n"));
        assert!(text.contains("Proxy-Require: sec-agree\r\n"));
        assert!(!text.contains("Accept-Contact:"));
        assert!(!text.contains("P-Preferred-Service:"));
        assert!(!text.contains("\r\nAccept: application/vnd.3gpp.sms\r\n"));
        assert!(text.contains("Security-Client: ipsec-3gpp\r\n"));
    }

    #[test]
    fn ims_feature_policy_adds_mmtel_routing_without_forcing_sec_agree() {
        let ids = RequestIds::fresh(1);
        let frame = build_register_with_policy(
            &ident(),
            &route_udp(),
            &ids,
            3600,
            None,
            Some("ipsec-3gpp;alg=hmac-md5-96;ealg=null;spi-c=1;spi-s=2;port-c=6000;port-s=6001"),
            None,
            "urn:uuid:00000000-0000-4000-8000-000000000000",
            RegisterRequestPolicy {
                advertise_sec_agree: true,
                require_sec_agree: false,
                proxy_require_sec_agree: false,
                include_mmtel_features: true,
                include_video_feature: false,
                include_route_header: true,
                include_visited_network: true,
            },
        );
        let text = String::from_utf8(frame).unwrap();

        assert!(text.contains(";audio;+g.3gpp.smsip;+g.3gpp.icsi-ref=\""));
        assert!(text.contains(";+sip.instance=\"<urn:uuid:"));
        assert!(!text.contains("Accept-Contact:"));
        assert!(!text.contains("P-Preferred-Service:"));
        assert!(text.contains("Route: <sip:10.0.0.1:5060;lr>\r\n"));
        assert!(text.contains("P-Visited-Network-ID: \"ims.mnc000.mcc460.3gppnetwork.org\"\r\n"));
        assert!(text.contains("Supported: path, gruu, sec-agree\r\n"));
        assert!(text.contains(&format!("Allow: {MMTEL_ALLOW_METHODS}\r\n")));
        assert!(!text.contains("Require: sec-agree\r\n"));
        assert!(!text.contains("Proxy-Require: sec-agree\r\n"));
        assert!(!text.contains("Authorization:"));
    }

    #[test]
    fn sms_message_appends_binary_body_after_headers() {
        let body = vec![0x01, 0x02, 0x03, 0xff];
        let frame = build_sms_message(
            &ident(),
            &route_udp(),
            Some("<sip:service-route.example:9900;lr>"),
            "sip:+8613800100500@ims.mnc000.mcc460.3gppnetwork.org",
            "sip:+8619399144749@ims.mnc000.mcc460.3gppnetwork.org",
            &body,
            None,
        );
        // Body must be preserved verbatim after CRLFCRLF.
        assert_eq!(sip_body(&frame), &body[..]);
        let text = String::from_utf8_lossy(&frame);
        assert!(text.contains("Content-Type: application/vnd.3gpp.sms\r\n"));
        assert!(text.contains("Accept: application/vnd.3gpp.sms\r\n"));
        assert!(text.contains("Route: <sip:service-route.example:9900;lr>\r\n"));
        assert!(text.starts_with(
            "MESSAGE sip:+8613800100500@ims.mnc000.mcc460.3gppnetwork.org SIP/2.0\r\n"
        ));
        assert!(text.contains("To: <sip:+8619399144749@ims.mnc000.mcc460.3gppnetwork.org>\r\n"));
        assert!(text.contains("Content-Length: 4\r\n"));
        assert!(text.contains("Accept-Contact: *;+g.3gpp.smsip\r\n"));
    }

    #[test]
    fn parse_status_reads_code() {
        assert_eq!(parse_status(b"SIP/2.0 200 OK\r\n\r\n").unwrap(), 200);
        assert_eq!(
            parse_status(b"SIP/2.0 401 Unauthorized\r\nWWW-Authenticate: Digest\r\n\r\n").unwrap(),
            401
        );
        assert!(parse_status(b"garbage").is_err());
    }

    #[test]
    fn complete_frame_len_honors_content_length_and_coalescing() {
        // Two messages coalesced in one TCP read.
        let msg1 = b"SIP/2.0 200 OK\r\nContent-Length: 3\r\n\r\nabc";
        let msg2 = b"MESSAGE sip:x SIP/2.0\r\nContent-Length: 0\r\n\r\n";
        let mut buf = Vec::new();
        buf.extend_from_slice(msg1);
        buf.extend_from_slice(msg2);
        let len1 = complete_frame_len(&buf).unwrap();
        assert_eq!(len1, msg1.len());
        // Remaining bytes form the second complete frame.
        let rest = &buf[len1..];
        assert_eq!(complete_frame_len(rest).unwrap(), msg2.len());
    }

    #[test]
    fn complete_frame_len_needs_more_when_body_truncated() {
        let partial = b"SIP/2.0 200 OK\r\nContent-Length: 10\r\n\r\nabc";
        assert!(complete_frame_len(partial).is_none());
        let no_terminator = b"SIP/2.0 200 OK\r\nContent-Length: 10\r\n";
        assert!(complete_frame_len(no_terminator).is_none());
    }

    #[test]
    fn is_request_matches_method() {
        assert!(is_request(b"MESSAGE sip:x SIP/2.0\r\n\r\n", "MESSAGE"));
        assert!(!is_request(b"SIP/2.0 200 OK\r\n\r\n", "MESSAGE"));
        assert!(!is_request(b"INVITE sip:x SIP/2.0\r\n\r\n", "MESSAGE"));
    }

    #[test]
    fn header_values_are_case_insensitive_and_multi() {
        let frame = b"SIP/2.0 200 OK\r\nVia: a\r\nvia: b\r\nContact: <sip:x@h>\r\n\r\n";
        assert_eq!(header_values(frame, "via"), vec!["a", "b"]);
        assert_eq!(header_value(frame, "CONTACT").as_deref(), Some("<sip:x@h>"));
    }

    #[test]
    fn sip_header_uri_extracts_bracketed_and_bare() {
        let frame = b"MESSAGE sip:x SIP/2.0\r\nFrom: <sip:+8613800138000@h>;tag=abc\r\n\r\n";
        assert_eq!(
            sip_header_uri(frame, "From").as_deref(),
            Some("sip:+8613800138000@h")
        );
        let bare = b"MESSAGE sip:x SIP/2.0\r\nFrom: sip:user@h;tag=abc\r\n\r\n";
        assert_eq!(sip_header_uri(bare, "From").as_deref(), Some("sip:user@h"));
    }

    #[test]
    fn rp_ack_targets_inbound_from_uri() {
        let inbound = b"MESSAGE sip:me SIP/2.0\r\nFrom: <sip:+8613800138000@h>;tag=x\r\n\r\nBODY";
        let frame = build_rp_ack(
            &ident(),
            &route_udp(),
            None,
            inbound,
            &[0x02, 0x00],
            "sip:fallback@h",
            None,
        );
        let text = String::from_utf8_lossy(&frame);
        assert!(text.starts_with("MESSAGE sip:+8613800138000@h SIP/2.0\r\n"));
        assert_eq!(sip_body(&frame), &[0x02, 0x00]);
    }

    #[test]
    fn invite_carries_sdp_and_mmtel_service_tags() {
        let dialog = DialogIds::fresh();
        let sdp = b"v=0\r\no=- 1 1 IN IP4 10.0.0.2\r\ns=SimAdmin\r\n";
        let frame = build_invite(
            &ident(),
            &route_udp(),
            Some("<sip:service-route.ims.example;lr>"),
            &dialog,
            "sip:+8613800138000@ims.mnc000.mcc460.3gppnetwork.org",
            sdp,
            None,
        );
        let text = String::from_utf8_lossy(&frame);
        assert!(text.starts_with(
            "INVITE sip:+8613800138000@ims.mnc000.mcc460.3gppnetwork.org SIP/2.0\r\n"
        ));
        assert!(text.contains("CSeq: 1 INVITE\r\n"));
        assert!(text.contains("P-Preferred-Service: urn:urn-7:3gpp-service.ims.icsi.mmtel\r\n"));
        assert!(text.contains("Content-Type: application/sdp\r\n"));
        assert!(text.contains("Route: <sip:service-route.ims.example;lr>\r\n"));
        assert!(text.contains(&format!("Content-Length: {}\r\n", sdp.len())));
        // SDP body preserved verbatim after the header terminator.
        assert_eq!(sip_body(&frame), &sdp[..]);
    }

    #[test]
    fn ack_uses_remote_tag_and_invite_cseq() {
        let mut dialog = DialogIds::fresh();
        dialog.set_remote_tag("remotetag123");
        let frame = build_ack(
            &ident(),
            &route_udp(),
            None,
            &dialog,
            "sip:+8613800138000@h",
        );
        let text = String::from_utf8_lossy(&frame);
        assert!(text.starts_with("ACK sip:+8613800138000@h SIP/2.0\r\n"));
        assert!(text.contains("To: <sip:+8613800138000@h>;tag=remotetag123\r\n"));
        assert!(text.contains("CSeq: 1 ACK\r\n"));
        assert!(text.ends_with("Content-Length: 0\r\n\r\n"));
    }

    #[test]
    fn prack_references_reliable_provisional_response() {
        let mut dialog = DialogIds::fresh();
        dialog.set_remote_tag("early-tag");
        let frame = build_prack(
            &ident(),
            &route_udp(),
            None,
            &dialog,
            "sip:+8613800138000@h",
            2,
            77,
            1,
        );
        let text = String::from_utf8(frame).unwrap();
        assert!(text.starts_with("PRACK sip:+8613800138000@h SIP/2.0\r\n"));
        assert!(text.contains("To: <sip:+8613800138000@h>;tag=early-tag\r\n"));
        assert!(text.contains("RAck: 77 1 INVITE\r\n"));
        assert!(text.contains("CSeq: 2 PRACK\r\n"));
    }

    #[test]
    fn bye_increments_cseq_and_targets_remote_tag() {
        let mut dialog = DialogIds::fresh();
        dialog.set_remote_tag("rt");
        let frame = build_bye(
            &ident(),
            &route_udp(),
            None,
            &dialog,
            "sip:+8613800138000@h",
            2,
        );
        let text = String::from_utf8_lossy(&frame);
        assert!(text.starts_with("BYE sip:+8613800138000@h SIP/2.0\r\n"));
        assert!(text.contains("CSeq: 2 BYE\r\n"));
        assert!(text.contains("To: <sip:+8613800138000@h>;tag=rt\r\n"));
    }

    #[test]
    fn access_specific_dialog_messages_use_the_requested_user_agent() {
        let identity = ident();
        let route = route_udp();
        let mut dialog = DialogIds::fresh();
        dialog.set_remote_tag("remote-tag");
        let callee = "sip:+8613800138000@h";
        let user_agent = "SimAdmin VoWiFi Test";
        let frames = [
            build_ack_for_access(&identity, &route, None, &dialog, callee, user_agent),
            build_prack_for_access(
                &identity, &route, None, &dialog, callee, 2, 77, 1, user_agent,
            ),
            build_bye_for_access(
                &identity, &route, None, &dialog, callee, 2, user_agent,
            ),
            build_cancel_for_access(
                &identity,
                &route,
                None,
                &dialog,
                callee,
                "z9hG4bKinvite",
                user_agent,
            ),
            build_response_for_access(
                b"OPTIONS sip:user@h SIP/2.0\r\nVia: SIP/2.0/UDP 10.0.0.1:5060;branch=z9hG4bKrequest\r\nFrom: <sip:user@h>;tag=from-tag\r\nTo: <sip:me@h>\r\nCall-ID: request-call-id\r\nCSeq: 1 OPTIONS\r\nContent-Length: 0\r\n\r\n",
                200,
                "OK",
                Some("local-tag"),
                None,
                None,
                user_agent,
            ),
        ];

        for frame in frames {
            assert_eq!(
                header_value(&frame, "User-Agent").as_deref(),
                Some(user_agent)
            );
            assert_ne!(
                header_value(&frame, "User-Agent").as_deref(),
                Some(USER_AGENT)
            );
        }
    }

    #[test]
    fn dtmf_info_uses_confirmed_dialog_and_dtmf_relay_body() {
        let mut dialog = DialogIds::fresh();
        dialog.set_remote_tag("remote-tag");
        let frame = build_dtmf_info(
            &ident(),
            &route_udp(),
            None,
            &dialog,
            "sip:+8613800138000@h",
            3,
            '5',
            240,
        )
        .unwrap();
        let text = String::from_utf8(frame).unwrap();
        assert!(text.starts_with("INFO sip:+8613800138000@h SIP/2.0\r\n"));
        assert!(text.contains("To: <sip:+8613800138000@h>;tag=remote-tag\r\n"));
        assert!(text.contains("CSeq: 3 INFO\r\n"));
        assert!(text.contains("Content-Type: application/dtmf-relay\r\n"));
        assert!(text.ends_with("Signal=5\r\nDuration=240\r\n"));
    }

    #[test]
    fn dtmf_info_rejects_invalid_digit_and_duration() {
        let dialog = DialogIds::fresh();
        assert!(build_dtmf_info(
            &ident(),
            &route_udp(),
            None,
            &dialog,
            "sip:+8613800138000@h",
            2,
            'Z',
            160,
        )
        .is_err());
        assert!(build_dtmf_info(
            &ident(),
            &route_udp(),
            None,
            &dialog,
            "sip:+8613800138000@h",
            2,
            '1',
            10,
        )
        .is_err());
    }
}
