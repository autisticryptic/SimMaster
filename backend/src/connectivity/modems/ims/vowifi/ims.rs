#![allow(dead_code)]

use serde::Serialize;

use super::profiles::CarrierProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisterPhase {
    Idle,
    InitialRegister,
    ChallengePending,
    SecurityAgreement,
    AuthenticatedRegister,
    Registered,
    Failed,
}

impl RegisterPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::InitialRegister => "initial_register",
            Self::ChallengePending => "challenge_pending",
            Self::SecurityAgreement => "security_agreement",
            Self::AuthenticatedRegister => "authenticated_register",
            Self::Registered => "registered",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecAgreeMechanismPlan {
    pub mechanism: &'static str,
    pub integrity: &'static str,
    pub encryption: &'static str,
    pub protocol: &'static str,
    pub mode: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImsRegisterPlan {
    pub profile_id: &'static str,
    pub plmn: &'static str,
    pub domain: &'static str,
    pub realm: &'static str,
    pub registrar: Option<&'static str>,
    pub pcscf: Option<&'static str>,
    pub transport: &'static str,
    pub local_port: u16,
    pub user_agent_family: &'static str,
    pub identity_source: &'static str,
    pub supported_header: &'static str,
    pub include_pani_authenticated: bool,
    pub strict_security_server_offer: bool,
    pub enable_initial_reject_fallback: bool,
    pub security_client_mechanisms: Vec<SecAgreeMechanismPlan>,
    pub sms_receiver_transport: &'static str,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImsRuntimeState {
    pub phase: &'static str,
    pub register_trace_id: String,
    pub selected_security_mechanism: Option<String>,
    pub last_sip_status: Option<u16>,
    pub registered_expires_seconds: Option<u32>,
    pub sms_ready: bool,
    pub last_error: Option<String>,
}

impl Default for ImsRuntimeState {
    fn default() -> Self {
        Self {
            phase: RegisterPhase::Idle.as_str(),
            register_trace_id: String::new(),
            selected_security_mechanism: None,
            last_sip_status: None,
            registered_expires_seconds: None,
            sms_ready: false,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SipMessageSummary {
    pub direction: &'static str,
    pub message_kind: &'static str,
    pub method: Option<&'static str>,
    pub status_code: Option<u16>,
    pub transport: &'static str,
    pub target_domain: &'static str,
    pub authorization_present: bool,
    pub security_client_present: bool,
    pub security_server_present: bool,
    pub security_verify_present: bool,
    pub digest_challenge_present: bool,
    pub body_bytes: usize,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DigestChallengeSummary {
    pub header_kind: &'static str,
    pub algorithm: String,
    pub realm_matches_profile: bool,
    pub challenge_token_present: bool,
    pub qop_present: bool,
    pub opaque_present: bool,
    pub stale: bool,
    pub values_redacted: bool,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AkaDigestPublicState {
    pub algorithm: String,
    pub provider: &'static str,
    pub challenge_accepted: bool,
    pub auth_proof_present: bool,
    pub auth_proof_bytes: usize,
    pub sec_agree_key_source_ready: bool,
    pub exported_secret_values: bool,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecAgreeMechanismSummary {
    pub mechanism: String,
    pub integrity: String,
    pub encryption: String,
    pub protocol: String,
    pub mode: String,
    pub local_sa_identifier_present: bool,
    pub remote_sa_identifier_present: bool,
    pub local_port_present: bool,
    pub remote_port_present: bool,
    pub values_redacted: bool,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecAgreePublicState {
    pub security_mode: &'static str,
    pub client_offer_count: usize,
    pub server_offer_count: usize,
    pub selected_security_mechanism: Option<String>,
    pub server_offer_selected: bool,
    pub security_verify_ready: bool,
    pub protected_transport_ready: bool,
    pub local_sa_identifier_present: bool,
    pub remote_sa_identifier_present: bool,
    pub policy_installed: bool,
    pub exported_secret_values: bool,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SipResponseSummary {
    pub status_code: u16,
    pub reason: String,
    pub digest_challenge: Option<DigestChallengeSummary>,
    pub security_server_offers: Vec<SecAgreeMechanismSummary>,
    pub security_verify_present: bool,
    pub warning_present: bool,
    pub unsupported: Vec<String>,
    pub require: Vec<String>,
    pub proxy_require: Vec<String>,
    pub expires_seconds: Option<u32>,
    pub service_route_present: bool,
    pub associated_uri_count: usize,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImsRegisterPublicState {
    pub profile_id: &'static str,
    pub plmn: &'static str,
    pub phase: &'static str,
    pub transport: &'static str,
    pub target_domain: &'static str,
    pub last_sip_status: Option<u16>,
    pub register_200_received: bool,
    pub registered_expires_seconds: Option<u32>,
    pub security_mode: &'static str,
    pub selected_security_mechanism: Option<String>,
    pub challenge: Option<DigestChallengeSummary>,
    pub aka_digest: Option<AkaDigestPublicState>,
    pub sec_agree: Option<SecAgreePublicState>,
    pub service_route_present: bool,
    pub associated_uri_count: usize,
    pub sms_ready: bool,
    pub transcript: Vec<SipMessageSummary>,
    pub last_error: Option<String>,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImsRegisterError {
    EmptySecurityClientMechanisms,
    MissingDigestChallenge,
    MissingSecurityServerOffer,
    NoMatchingSecurityMechanism,
    SipResponseMalformed,
    UnexpectedSipStatus(u16),
    InvalidPhase {
        expected: &'static str,
        actual: &'static str,
    },
}

impl std::fmt::Display for ImsRegisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySecurityClientMechanisms => write!(f, "profile has no sec-agree offer"),
            Self::MissingDigestChallenge => write!(f, "SIP challenge is missing digest data"),
            Self::MissingSecurityServerOffer => write!(f, "SIP challenge has no security offer"),
            Self::NoMatchingSecurityMechanism => write!(f, "no compatible security offer"),
            Self::SipResponseMalformed => write!(f, "malformed SIP response"),
            Self::UnexpectedSipStatus(status) => write!(f, "unexpected SIP status {status}"),
            Self::InvalidPhase { expected, actual } => {
                write!(f, "invalid IMS phase expected={expected} actual={actual}")
            }
        }
    }
}

impl std::error::Error for ImsRegisterError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImsRegisterStateMachine {
    profile: &'static CarrierProfile,
    phase: RegisterPhase,
    plan: ImsRegisterPlan,
    transcript: Vec<SipMessageSummary>,
    last_sip_status: Option<u16>,
    registered_expires_seconds: Option<u32>,
    challenge: Option<DigestChallengeSummary>,
    aka_digest: Option<AkaDigestPublicState>,
    sec_agree: Option<SecAgreePublicState>,
    service_route_present: bool,
    associated_uri_count: usize,
    last_error: Option<String>,
}

impl ImsRegisterStateMachine {
    pub fn new(profile: &'static CarrierProfile) -> Self {
        Self {
            profile,
            phase: RegisterPhase::Idle,
            plan: build_register_plan(profile),
            transcript: Vec::new(),
            last_sip_status: None,
            registered_expires_seconds: None,
            challenge: None,
            aka_digest: None,
            sec_agree: None,
            service_route_present: false,
            associated_uri_count: 0,
            last_error: None,
        }
    }

    pub fn snapshot(&self) -> ImsRegisterPublicState {
        ImsRegisterPublicState {
            profile_id: self.profile.meta.profile_id,
            plmn: self.profile.meta.plmn,
            phase: self.phase.as_str(),
            transport: self.plan.transport,
            target_domain: self.target_domain(),
            last_sip_status: self.last_sip_status,
            register_200_received: self.last_sip_status == Some(200),
            registered_expires_seconds: self.registered_expires_seconds,
            security_mode: self
                .sec_agree
                .as_ref()
                .map(|state| state.security_mode)
                .unwrap_or("none"),
            selected_security_mechanism: self
                .sec_agree
                .as_ref()
                .and_then(|state| state.selected_security_mechanism.clone()),
            challenge: self.challenge.clone(),
            aka_digest: self.aka_digest.clone(),
            sec_agree: self.sec_agree.clone(),
            service_route_present: self.service_route_present,
            associated_uri_count: self.associated_uri_count,
            sms_ready: false,
            transcript: self.transcript.clone(),
            last_error: self.last_error.clone(),
            sensitive_values_policy: "sip_identity_auth_material_and_sa_values_not_serialized",
        }
    }

    pub fn build_initial_register(&mut self) -> Result<SipMessageSummary, ImsRegisterError> {
        self.require_phase(RegisterPhase::Idle)?;
        if self.plan.security_client_mechanisms.is_empty() {
            return self.fail(ImsRegisterError::EmptySecurityClientMechanisms);
        }
        self.phase = RegisterPhase::InitialRegister;
        let summary = SipMessageSummary {
            direction: "outbound",
            message_kind: "sip_request",
            method: Some("REGISTER"),
            status_code: None,
            transport: self.plan.transport,
            target_domain: self.target_domain(),
            authorization_present: false,
            security_client_present: true,
            security_server_present: false,
            security_verify_present: false,
            digest_challenge_present: false,
            body_bytes: 0,
            sensitive_values_policy: "sip_identity_headers_not_serialized",
        };
        self.transcript.push(summary.clone());
        Ok(summary)
    }

    pub fn accept_challenge_response(
        &mut self,
        response: &str,
    ) -> Result<SipResponseSummary, ImsRegisterError> {
        self.require_phase(RegisterPhase::InitialRegister)?;
        let parsed = parse_sip_response(response, self.plan.realm)?;
        if parsed.status_code != 401 && parsed.status_code != 407 {
            return self.fail(ImsRegisterError::UnexpectedSipStatus(parsed.status_code));
        }

        let challenge = parsed
            .digest_challenge
            .clone()
            .ok_or(ImsRegisterError::MissingDigestChallenge)?;
        let selected = match select_security_offer(&self.plan, &parsed.security_server_offers) {
            Ok(selected) => selected,
            Err(err) => return self.fail(err),
        };
        let aka_digest = build_aka_digest_public_state(&challenge, self.plan.realm);
        let sec_agree = build_sec_agree_state(
            &self.plan,
            &selected,
            parsed.security_server_offers.len(),
            aka_digest.sec_agree_key_source_ready,
        );

        self.phase = RegisterPhase::ChallengePending;
        self.last_sip_status = Some(parsed.status_code);
        self.challenge = Some(challenge);
        self.aka_digest = Some(aka_digest);
        self.sec_agree = Some(sec_agree);
        self.transcript.push(SipMessageSummary {
            direction: "inbound",
            message_kind: "sip_response",
            method: None,
            status_code: Some(parsed.status_code),
            transport: self.plan.transport,
            target_domain: self.target_domain(),
            authorization_present: false,
            security_client_present: false,
            security_server_present: !parsed.security_server_offers.is_empty(),
            security_verify_present: parsed.security_verify_present,
            digest_challenge_present: parsed.digest_challenge.is_some(),
            body_bytes: 0,
            sensitive_values_policy: "sip_challenge_values_redacted",
        });
        self.phase = RegisterPhase::SecurityAgreement;
        Ok(parsed)
    }

    pub fn build_authenticated_register(&mut self) -> Result<SipMessageSummary, ImsRegisterError> {
        self.require_phase(RegisterPhase::SecurityAgreement)?;
        let security_verify_ready = self
            .sec_agree
            .as_ref()
            .map(|state| state.security_verify_ready)
            .unwrap_or(false);
        let summary = SipMessageSummary {
            direction: "outbound",
            message_kind: "sip_request",
            method: Some("REGISTER"),
            status_code: None,
            transport: self.plan.transport,
            target_domain: self.target_domain(),
            authorization_present: true,
            security_client_present: true,
            security_server_present: false,
            security_verify_present: security_verify_ready,
            digest_challenge_present: false,
            body_bytes: 0,
            sensitive_values_policy: "authorization_and_security_values_not_serialized",
        };
        self.phase = RegisterPhase::AuthenticatedRegister;
        self.transcript.push(summary.clone());
        Ok(summary)
    }

    pub fn accept_success_response(
        &mut self,
        response: &str,
    ) -> Result<SipResponseSummary, ImsRegisterError> {
        self.require_phase(RegisterPhase::AuthenticatedRegister)?;
        let parsed = parse_sip_response(response, self.plan.realm)?;
        if parsed.status_code != 200 {
            return self.fail(ImsRegisterError::UnexpectedSipStatus(parsed.status_code));
        }
        self.phase = RegisterPhase::Registered;
        self.last_sip_status = Some(parsed.status_code);
        self.registered_expires_seconds = parsed.expires_seconds;
        self.service_route_present = parsed.service_route_present;
        self.associated_uri_count = parsed.associated_uri_count;
        if let Some(sec_agree) = self.sec_agree.as_mut() {
            sec_agree.policy_installed = true;
            sec_agree.protected_transport_ready = true;
        }
        self.transcript.push(SipMessageSummary {
            direction: "inbound",
            message_kind: "sip_response",
            method: None,
            status_code: Some(parsed.status_code),
            transport: self.plan.transport,
            target_domain: self.target_domain(),
            authorization_present: false,
            security_client_present: false,
            security_server_present: false,
            security_verify_present: parsed.security_verify_present,
            digest_challenge_present: false,
            body_bytes: 0,
            sensitive_values_policy: "sip_success_headers_metadata_only",
        });
        Ok(parsed)
    }

    fn target_domain(&self) -> &'static str {
        self.plan.registrar.unwrap_or(self.plan.domain)
    }

    fn require_phase(&self, expected: RegisterPhase) -> Result<(), ImsRegisterError> {
        if self.phase == expected {
            Ok(())
        } else {
            Err(ImsRegisterError::InvalidPhase {
                expected: expected.as_str(),
                actual: self.phase.as_str(),
            })
        }
    }

    fn fail<T>(&mut self, error: ImsRegisterError) -> Result<T, ImsRegisterError> {
        self.phase = RegisterPhase::Failed;
        self.last_error = Some(error.to_string());
        Err(error)
    }
}

pub fn build_register_plan(profile: &'static CarrierProfile) -> ImsRegisterPlan {
    ImsRegisterPlan {
        profile_id: profile.meta.profile_id,
        plmn: profile.meta.plmn,
        domain: profile.ims.domain,
        realm: profile.ims.realm,
        registrar: profile.ims.registrar,
        pcscf: profile.ims.pcscf,
        transport: profile.ims.transport,
        local_port: profile.ims.local_port,
        user_agent_family: "simadmin_vowifi",
        identity_source: profile.ims.identity_source,
        supported_header: profile.ims.register.supported_header,
        include_pani_authenticated: profile.ims.register.include_pani_authenticated,
        strict_security_server_offer: profile.ims.register.strict_security_server_offer,
        enable_initial_reject_fallback: profile.ims.register.enable_initial_reject_fallback,
        security_client_mechanisms: profile
            .ims
            .register
            .security_client_mechanisms
            .iter()
            .map(|mechanism| parse_security_mechanism(mechanism))
            .collect(),
        sms_receiver_transport: profile.sms.receiver_transport,
        sensitive_values_policy: "opaque_runtime_only",
    }
}

pub fn build_dry_run_register_snapshot(profile: &'static CarrierProfile) -> ImsRegisterPublicState {
    let mut machine = ImsRegisterStateMachine::new(profile);
    if machine.build_initial_register().is_err() {
        return machine.snapshot();
    }
    if machine
        .accept_challenge_response(&synthetic_challenge_response(profile))
        .is_err()
    {
        return machine.snapshot();
    }
    if machine.build_authenticated_register().is_err() {
        return machine.snapshot();
    }
    if machine
        .accept_success_response(&synthetic_success_response(profile))
        .is_err()
    {
        return machine.snapshot();
    }
    machine.snapshot()
}

pub fn parse_sip_response(
    response: &str,
    expected_realm: &str,
) -> Result<SipResponseSummary, ImsRegisterError> {
    let mut lines = response
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let status_line = lines.next().ok_or(ImsRegisterError::SipResponseMalformed)?;
    let mut status_parts = status_line.splitn(3, ' ');
    if status_parts.next() != Some("SIP/2.0") {
        return Err(ImsRegisterError::SipResponseMalformed);
    }
    let status_code = status_parts
        .next()
        .and_then(|part| part.parse::<u16>().ok())
        .ok_or(ImsRegisterError::SipResponseMalformed)?;
    let reason = status_parts.next().unwrap_or("").to_string();

    let mut digest_challenge = None;
    let mut security_server_offers = Vec::new();
    let mut security_verify_present = false;
    let mut warning_present = false;
    let mut unsupported = Vec::new();
    let mut require = Vec::new();
    let mut proxy_require = Vec::new();
    let mut expires_seconds = None;
    let mut service_route_present = false;
    let mut associated_uri_count = 0usize;

    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            "www-authenticate" | "proxy-authenticate" => {
                digest_challenge = Some(parse_digest_challenge(
                    if name == "www-authenticate" {
                        "www_authenticate"
                    } else {
                        "proxy_authenticate"
                    },
                    value,
                    expected_realm,
                ));
            }
            "security-server" => {
                security_server_offers.extend(
                    split_header_values(value)
                        .into_iter()
                        .map(|item| parse_security_server_offer(&item)),
                );
            }
            "security-verify" => {
                security_verify_present = true;
            }
            "warning" => {
                warning_present = true;
            }
            "unsupported" => {
                unsupported.extend(redacted_token_list(value));
            }
            "require" => {
                require.extend(redacted_token_list(value));
            }
            "proxy-require" => {
                proxy_require.extend(redacted_token_list(value));
            }
            "expires" => {
                expires_seconds = value.parse::<u32>().ok();
            }
            "service-route" => {
                service_route_present = true;
            }
            "p-associated-uri" => {
                associated_uri_count =
                    associated_uri_count.saturating_add(split_header_values(value).len().max(1));
            }
            _ => {}
        }
    }

    Ok(SipResponseSummary {
        status_code,
        reason,
        digest_challenge,
        security_server_offers,
        security_verify_present,
        warning_present,
        unsupported,
        require,
        proxy_require,
        expires_seconds,
        service_route_present,
        associated_uri_count,
        sensitive_values_policy: "sip_response_values_redacted",
    })
}

fn redacted_token_list(value: &str) -> Vec<String> {
    split_header_values(value)
        .into_iter()
        .filter_map(|item| {
            let token = item.trim().to_ascii_lowercase();
            (!token.is_empty()
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_')))
            .then_some(token)
        })
        .collect()
}

fn parse_security_mechanism(mechanism: &'static str) -> SecAgreeMechanismPlan {
    let tokens = mechanism.split('/').collect::<Vec<_>>();
    SecAgreeMechanismPlan {
        mechanism,
        integrity: tokens.first().copied().unwrap_or("profile_default"),
        encryption: tokens.get(1).copied().unwrap_or("profile_default"),
        protocol: tokens.get(2).copied().unwrap_or("profile_default"),
        mode: tokens.get(3).copied().unwrap_or("profile_default"),
    }
}

fn parse_security_server_offer(value: &str) -> SecAgreeMechanismSummary {
    let parts = value.split(';').map(str::trim).collect::<Vec<_>>();
    if parts.len() == 1 && parts[0].contains('/') {
        let tokens = parts[0].split('/').collect::<Vec<_>>();
        return SecAgreeMechanismSummary {
            mechanism: parts[0].to_string(),
            integrity: tokens
                .first()
                .copied()
                .unwrap_or("profile_default")
                .to_string(),
            encryption: tokens
                .get(1)
                .copied()
                .unwrap_or("profile_default")
                .to_string(),
            protocol: tokens
                .get(2)
                .copied()
                .unwrap_or("profile_default")
                .to_string(),
            mode: tokens
                .get(3)
                .copied()
                .unwrap_or("profile_default")
                .to_string(),
            local_sa_identifier_present: false,
            remote_sa_identifier_present: false,
            local_port_present: false,
            remote_port_present: false,
            values_redacted: true,
            sensitive_values_policy: "security_offer_metadata_only",
        };
    }

    let params = parts
        .iter()
        .skip(1)
        .filter_map(|part| {
            let (key, value) = part.split_once('=')?;
            Some((
                key.trim().to_ascii_lowercase(),
                trim_param_value(value).to_string(),
            ))
        })
        .collect::<Vec<_>>();

    let param = |name: &str| {
        params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let integrity = param("alg").unwrap_or("profile_default").to_string();
    let encryption = param("ealg").unwrap_or("profile_default").to_string();
    let protocol = param("prot").unwrap_or("profile_default").to_string();
    let mode = param("mod").unwrap_or("profile_default").to_string();
    let mechanism = format!("{integrity}/{encryption}/{protocol}/{mode}");

    SecAgreeMechanismSummary {
        mechanism,
        integrity,
        encryption,
        protocol,
        mode,
        local_sa_identifier_present: param("spi-c").is_some(),
        remote_sa_identifier_present: param("spi-s").is_some(),
        local_port_present: param("port-c").is_some(),
        remote_port_present: param("port-s").is_some(),
        values_redacted: true,
        sensitive_values_policy: "security_offer_metadata_only_sa_values_redacted",
    }
}

fn parse_digest_challenge(
    header_kind: &'static str,
    value: &str,
    expected_realm: &str,
) -> DigestChallengeSummary {
    let params = parse_digest_params(value);
    let get = |name: &str| {
        params
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    };
    let algorithm = get("algorithm").unwrap_or("AKAv1-MD5").to_string();
    let realm_matches_profile = get("realm")
        .map(|realm| realm == expected_realm)
        .unwrap_or(false);
    let stale = get("stale")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    DigestChallengeSummary {
        header_kind,
        algorithm,
        realm_matches_profile,
        challenge_token_present: get("nonce").is_some(),
        qop_present: get("qop").is_some(),
        opaque_present: get("opaque").is_some(),
        stale,
        values_redacted: true,
        sensitive_values_policy: "digest_challenge_values_not_serialized",
    }
}

fn parse_digest_params(value: &str) -> Vec<(String, String)> {
    let trimmed = value
        .trim()
        .strip_prefix("Digest")
        .map(str::trim)
        .unwrap_or(value.trim());
    split_header_values(trimmed)
        .into_iter()
        .filter_map(|part| {
            let (key, value) = part.split_once('=')?;
            Some((key.trim().to_string(), trim_param_value(value).to_string()))
        })
        .collect()
}

fn split_header_values(value: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;

    for ch in value.chars() {
        match ch {
            '"' => {
                in_quote = !in_quote;
                current.push(ch);
            }
            ',' if !in_quote => {
                let item = current.trim();
                if !item.is_empty() {
                    values.push(item.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    let item = current.trim();
    if !item.is_empty() {
        values.push(item.to_string());
    }
    values
}

fn trim_param_value(value: &str) -> &str {
    value.trim().trim_matches('"')
}

fn select_security_offer(
    plan: &ImsRegisterPlan,
    server_offers: &[SecAgreeMechanismSummary],
) -> Result<SecAgreeMechanismSummary, ImsRegisterError> {
    if plan.security_client_mechanisms.is_empty() {
        return Err(ImsRegisterError::EmptySecurityClientMechanisms);
    }
    if server_offers.is_empty() {
        return Err(ImsRegisterError::MissingSecurityServerOffer);
    }

    for client in &plan.security_client_mechanisms {
        if let Some(offer) = server_offers
            .iter()
            .find(|offer| mechanism_matches(client, offer))
        {
            return Ok(offer.clone());
        }
    }

    if plan.strict_security_server_offer {
        Err(ImsRegisterError::NoMatchingSecurityMechanism)
    } else {
        Ok(server_offers[0].clone())
    }
}

fn mechanism_matches(client: &SecAgreeMechanismPlan, offer: &SecAgreeMechanismSummary) -> bool {
    client.integrity == offer.integrity
        && client.encryption == offer.encryption
        && client.protocol == offer.protocol
        && client.mode == offer.mode
}

fn build_aka_digest_public_state(
    challenge: &DigestChallengeSummary,
    realm: &str,
) -> AkaDigestPublicState {
    let method = "REGISTER";
    let uri = format!("sip:{realm}");
    let a1 = format!("simadmin-redacted:{realm}:aka-result-redacted");
    let a2 = format!("{method}:{uri}");
    let ha1 = format!("{:x}", md5::compute(a1.as_bytes()));
    let ha2 = format!("{:x}", md5::compute(a2.as_bytes()));
    let proof_input = format!("{ha1}:challenge-token-redacted:{ha2}");
    let proof = md5::compute(proof_input.as_bytes());

    AkaDigestPublicState {
        algorithm: challenge.algorithm.clone(),
        provider: "ims_aka_provider",
        challenge_accepted: challenge.realm_matches_profile && challenge.challenge_token_present,
        auth_proof_present: true,
        auth_proof_bytes: proof.0.len(),
        sec_agree_key_source_ready: true,
        exported_secret_values: false,
        sensitive_values_policy: "aka_digest_inputs_and_output_not_serialized",
    }
}

fn build_sec_agree_state(
    plan: &ImsRegisterPlan,
    selected: &SecAgreeMechanismSummary,
    server_offer_count: usize,
    key_source_ready: bool,
) -> SecAgreePublicState {
    SecAgreePublicState {
        security_mode: "ipsec3gpp",
        client_offer_count: plan.security_client_mechanisms.len(),
        server_offer_count,
        selected_security_mechanism: Some(selected.mechanism.clone()),
        server_offer_selected: true,
        security_verify_ready: true,
        protected_transport_ready: key_source_ready,
        local_sa_identifier_present: selected.local_sa_identifier_present,
        remote_sa_identifier_present: selected.remote_sa_identifier_present,
        policy_installed: false,
        exported_secret_values: false,
        sensitive_values_policy: "ipsec3gpp_sa_identifiers_and_keys_not_serialized",
    }
}

fn synthetic_challenge_response(profile: &'static CarrierProfile) -> String {
    let mechanism = profile
        .ims
        .register
        .security_client_mechanisms
        .first()
        .copied()
        .unwrap_or("hmac-sha-1-96/aes-cbc/esp/trans");
    let parsed = parse_security_mechanism(mechanism);
    format!(
        concat!(
            "SIP/2.0 401 Unauthorized\r\n",
            "WWW-Authenticate: Digest realm=\"{}\", algorithm=AKAv1-MD5, nonce=\"redacted\", qop=\"auth\"\r\n",
            "Security-Server: ipsec-3gpp;alg={};ealg={};prot={};mod={};spi-c=1;spi-s=2;port-c=5062;port-s=5063\r\n",
            "Content-Length: 0\r\n\r\n"
        ),
        profile.ims.realm, parsed.integrity, parsed.encryption, parsed.protocol, parsed.mode
    )
}

fn synthetic_success_response(profile: &'static CarrierProfile) -> String {
    format!(
        concat!(
            "SIP/2.0 200 OK\r\n",
            "Expires: 3600\r\n",
            "Security-Verify: ipsec-3gpp\r\n",
            "Service-Route: <sip:pcscf.{};lr>\r\n",
            "P-Associated-URI: <sip:registered-user@{}>\r\n",
            "Content-Length: 0\r\n\r\n"
        ),
        profile.ims.domain, profile.ims.domain
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::ims::vowifi::profiles::{GB_EE_23433, US_ATT_310410};

    #[test]
    fn builds_register_plan_with_sec_agree_and_sms_transport() {
        let plan = build_register_plan(&GB_EE_23433);

        assert_eq!(plan.profile_id, "gb_ee_23433");
        assert_eq!(plan.domain, "ims.mnc033.mcc234.3gppnetwork.org");
        assert_eq!(plan.transport, "tcp");
        assert_eq!(plan.identity_source, "carrier_device_model");
        assert_eq!(
            plan.security_client_mechanisms[0].integrity,
            "hmac-sha-1-96"
        );
        assert_eq!(plan.security_client_mechanisms[0].encryption, "aes-cbc");
        assert_eq!(plan.security_client_mechanisms[0].protocol, "esp");
        assert_eq!(plan.security_client_mechanisms[0].mode, "trans");
        assert_eq!(plan.sms_receiver_transport, "tcp");
    }

    #[test]
    fn captures_us_initial_reject_fallback_without_operator_secrets() {
        let plan = build_register_plan(&US_ATT_310410);

        assert!(plan.enable_initial_reject_fallback);
        assert!(plan.include_pani_authenticated);
        assert!(plan.strict_security_server_offer);
    }

    #[test]
    fn parses_401_digest_and_security_server_without_exposing_values() {
        let response = synthetic_challenge_response(&GB_EE_23433);
        let parsed = parse_sip_response(&response, GB_EE_23433.ims.realm).expect("parse 401");

        assert_eq!(parsed.status_code, 401);
        let challenge = parsed.digest_challenge.as_ref().expect("digest challenge");
        assert_eq!(challenge.algorithm, "AKAv1-MD5");
        assert!(challenge.realm_matches_profile);
        assert!(challenge.challenge_token_present);
        assert_eq!(parsed.security_server_offers.len(), 1);
        assert_eq!(
            parsed.security_server_offers[0].mechanism,
            "hmac-sha-1-96/aes-cbc/esp/trans"
        );
        assert!(parsed.security_server_offers[0].local_sa_identifier_present);
        assert!(parsed.security_server_offers[0].remote_sa_identifier_present);

        let json = serde_json::to_string(&parsed).expect("serialize parsed response");
        for forbidden in [
            "\"nonce\"",
            "\"authorization\"",
            "\"response\"",
            "\"spi\"",
            "\"ck\"",
            "\"ik\"",
            "spi-c=1",
            "spi-s=2",
        ] {
            assert!(
                !json.to_ascii_lowercase().contains(forbidden),
                "parsed response must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn register_state_machine_reaches_200_with_ipsec3gpp_sec_agree() {
        let mut machine = ImsRegisterStateMachine::new(&GB_EE_23433);

        let initial = machine.build_initial_register().expect("initial REGISTER");
        assert!(initial.security_client_present);
        machine
            .accept_challenge_response(&synthetic_challenge_response(&GB_EE_23433))
            .expect("accept challenge");
        let authenticated = machine
            .build_authenticated_register()
            .expect("authenticated REGISTER");
        assert!(authenticated.authorization_present);
        assert!(authenticated.security_verify_present);
        machine
            .accept_success_response(&synthetic_success_response(&GB_EE_23433))
            .expect("200 OK");

        let snapshot = machine.snapshot();
        assert_eq!(snapshot.phase, "registered");
        assert_eq!(snapshot.last_sip_status, Some(200));
        assert!(snapshot.register_200_received);
        assert_eq!(snapshot.security_mode, "ipsec3gpp");
        assert_eq!(
            snapshot.selected_security_mechanism.as_deref(),
            Some("hmac-sha-1-96/aes-cbc/esp/trans")
        );
        assert_eq!(snapshot.registered_expires_seconds, Some(3600));
        assert!(snapshot.service_route_present);
        assert_eq!(snapshot.associated_uri_count, 1);
        assert_eq!(snapshot.transcript.len(), 4);
        assert!(snapshot
            .sec_agree
            .as_ref()
            .map(|state| state.policy_installed)
            .unwrap_or(false));
    }

    #[test]
    fn strict_sec_agree_rejects_unmatched_server_offer() {
        let mut machine = ImsRegisterStateMachine::new(&GB_EE_23433);
        machine.build_initial_register().expect("initial REGISTER");
        let response = format!(
            concat!(
                "SIP/2.0 401 Unauthorized\r\n",
                "WWW-Authenticate: Digest realm=\"{}\", algorithm=AKAv1-MD5, nonce=\"redacted\"\r\n",
                "Security-Server: ipsec-3gpp;alg=hmac-sha-1-96;ealg=null;prot=esp;mod=trans;spi-c=1;spi-s=2\r\n",
                "Content-Length: 0\r\n\r\n"
            ),
            GB_EE_23433.ims.realm
        );

        let err = machine
            .accept_challenge_response(&response)
            .expect_err("strict policy rejects mismatched security offer");
        assert!(matches!(err, ImsRegisterError::NoMatchingSecurityMechanism));
        assert_eq!(machine.snapshot().phase, "failed");
    }

    #[test]
    fn dry_run_snapshot_serializes_register_200_without_auth_material() {
        let snapshot = build_dry_run_register_snapshot(&US_ATT_310410);

        assert_eq!(snapshot.profile_id, "us_att_310410");
        assert_eq!(snapshot.phase, "registered");
        assert_eq!(snapshot.last_sip_status, Some(200));
        assert_eq!(snapshot.security_mode, "ipsec3gpp");
        assert!(snapshot
            .aka_digest
            .as_ref()
            .map(|state| state.auth_proof_present)
            .unwrap_or(false));

        let json = serde_json::to_string(&snapshot).expect("serialize IMS snapshot");
        for forbidden in [
            "\"imsi\"",
            "\"iccid\"",
            "\"msisdn\"",
            "\"phone_number\"",
            "\"authorization\"",
            "\"nonce\"",
            "\"response\"",
            "\"ck\"",
            "\"ik\"",
            "\"spi\"",
            "simadmin-redacted",
            "aka-result",
            "challenge-token",
            "spi-c=1",
            "spi-s=2",
        ] {
            assert!(
                !json.to_ascii_lowercase().contains(forbidden),
                "IMS snapshot must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn dry_run_snapshot_reports_invalid_profile_without_panicking() {
        let mut profile = US_ATT_310410;
        profile.ims.register.security_client_mechanisms = &[];
        let profile = Box::leak(Box::new(profile));

        let snapshot = build_dry_run_register_snapshot(profile);

        assert_eq!(snapshot.phase, "failed");
        assert_eq!(snapshot.last_sip_status, None);
        assert!(!snapshot.register_200_received);
        assert_eq!(
            snapshot.last_error.as_deref(),
            Some("profile has no sec-agree offer")
        );
    }

    #[test]
    fn serialized_plan_has_no_auth_material_or_phone_identity_fields() {
        let plan = build_register_plan(&GB_EE_23433);
        let json = serde_json::to_string(&plan).expect("serialize ims plan");

        for forbidden_key in [
            "imsi",
            "iccid",
            "msisdn",
            "phone_number",
            "authorization",
            "nonce",
            "response",
            "ck",
            "ik",
        ] {
            assert!(
                !json
                    .to_ascii_lowercase()
                    .contains(&format!("\"{forbidden_key}\"")),
                "plan must not contain a {forbidden_key} field"
            );
        }
    }
}
