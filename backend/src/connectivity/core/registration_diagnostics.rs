//! Read-only IMS execution facts shared by cellular/WiFi APIs and UI.
//! Producers use attempt/epoch tokens. This projection never chooses a profile,
//! authenticates, reads a SIM, opens sockets or changes a recovery decision.
use std::time::{Duration, Instant};
use serde::Serialize;
use super::{context::SipTransport, register::{RegisterFailureKind, RegisterFailureMetadata, RegisterFailureStage}};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FactSource {
    #[default] Unknown, Database, CarrierCatalog, Derived, SimOverride, Isim, Usim,
    Environment, BearerPco, Profile, AccessConfiguration, Dns, Dhcp, Runtime,
}
impl FactSource {
    pub fn profile(value: &str) -> Self {
        match value { "database" => Self::Database, "carrier_catalog" => Self::CarrierCatalog,
            "derived" => Self::Derived, _ => Self::Unknown }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IpFamily {
    #[default] Unknown, Ipv4, Ipv6, Ipv4v6,
}
impl IpFamily {
    pub fn parse(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "ipv4" | "4" => Self::Ipv4, "ipv6" | "6" => Self::Ipv6,
            "ipv4v6" | "ipv4+ipv6" | "dual" => Self::Ipv4v6, _ => Self::Unknown,
        }
    }
    pub fn granted(v4: bool, v6: bool) -> Self {
        match (v4, v6) { (true,true) => Self::Ipv4v6, (true,false) => Self::Ipv4, (false,true) => Self::Ipv6, _ => Self::Unknown }
    }
    pub fn address(address: std::net::IpAddr) -> Self { if address.is_ipv4() { Self::Ipv4 } else { Self::Ipv6 } }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalTransport { #[default] Unknown, Udp, Tcp, Tls }
impl From<SipTransport> for SignalTransport {
    fn from(value: SipTransport) -> Self { match value { SipTransport::Udp => Self::Udp, SipTransport::Tcp => Self::Tcp, SipTransport::Tls => Self::Tls } }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityOrigin { #[default] Unknown, Isim, UsimDerived, SimOverrideDerived }
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityPreference { #[default] Unknown, Isim, Derived, CarrierDeviceModel, Other }
impl IdentityPreference {
    pub fn parse(value: &str) -> Self {
        match value { "isim" => Self::Isim, "derived" | "derived_imsi" => Self::Derived,
            "carrier_device_model" => Self::CarrierDeviceModel, "" => Self::Unknown, _ => Self::Other }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityRuleStatus { #[default] NotObserved, UiccResolved, StandardDerived, DeclaredOnly }

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityMode { #[default] Unknown, Plain, Ipsec, Tls }
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsVerification { #[default] NotObserved, NotApplicable, Pending, Verified, Failed }

/// Library-provided TLS enum names only, never a certificate, subject or SAN.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TlsAlgorithms { pub version: Option<String>, pub cipher_suite: Option<String> }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityAlgorithm { HmacSha1_96, HmacMd5_96 }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EncryptionAlgorithm { AesCbc, Null }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct IpsecAlgorithms { pub integrity: IntegrityAlgorithm, pub encryption: EncryptionAlgorithm }

/// Parse only whitelisted algorithm tokens from an already generated/validated
/// Security-Client/Verify value. SPI, ports, keys and unknown text never survive.
pub fn ipsec_algorithms(value: &str) -> Vec<IpsecAlgorithms> {
    if value.len() > 8192 { return Vec::new(); }
    let mut result = Vec::new();
    for mechanism in value.split(',').take(16) {
        let mut fields = mechanism.trim().split(';');
        if !fields.next().is_some_and(|name| name.trim().eq_ignore_ascii_case("ipsec-3gpp")) { continue; }
        let mut integrity = None; let mut encryption = None; let mut invalid = false;
        for field in fields {
            let Some((name, value)) = field.trim().split_once('=') else { continue; };
            match name.trim().to_ascii_lowercase().as_str() {
                "alg" => {
                    if integrity.is_some() { invalid = true; }
                    integrity = match value.trim().trim_matches('"').to_ascii_lowercase().as_str() {
                        "hmac-sha-1-96" | "hmac-sha1-96" => Some(IntegrityAlgorithm::HmacSha1_96),
                        "hmac-md5-96" => Some(IntegrityAlgorithm::HmacMd5_96), _ => { invalid = true; None },
                    };
                }
                "ealg" => {
                    if encryption.is_some() { invalid = true; }
                    encryption = match value.trim().trim_matches('"').to_ascii_lowercase().as_str() {
                        "aes-cbc" => Some(EncryptionAlgorithm::AesCbc), "null" => Some(EncryptionAlgorithm::Null),
                        _ => { invalid = true; None },
                    };
                }
                _ => {},
            }
        }
        if let (Some(integrity), Some(encryption), false) = (integrity, encryption, invalid) {
            let algorithms = IpsecAlgorithms { integrity, encryption };
            if !result.contains(&algorithms) { result.push(algorithms); }
        }
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DigestAlgorithm { Akav1Md5, Akav2Md5, Akav2Sha256, PlainMd5 }
impl DigestAlgorithm {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "akav1-md5" => Some(Self::Akav1Md5), "akav2-md5" => Some(Self::Akav2Md5),
            "akav2-sha-256" => Some(Self::Akav2Sha256), "md5" => Some(Self::PlainMd5), _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureScope { #[default] Unknown, Endpoint, Bearer, Binding, Authentication, Negotiation, Local, Ownership }
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationProcedure { #[default] NotObserved, Initial, Refresh, Unregister, RegistrationEvent }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStage { Initial, Authentication, Authenticated }
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailureFact {
    pub scope: FailureScope,
    pub reason_code: Option<String>,
    pub sip_status: Option<u16>,
    pub auth_stage: Option<AuthStage>,
    pub lease_negotiation_observed: bool,
    /// What the response requested, distinct from the actual enforced wait.
    pub response_retry_after_seconds: Option<u32>,
    pub response_retry_after_invalid: bool,
    pub procedure: RegistrationProcedure,
    pub endpoint_index: Option<u16>,
}
impl FailureFact {
    pub fn new(code: &str, metadata: Option<RegisterFailureMetadata>) -> Self {
        let reason_code = safe_reason(code);
        let mut scope = metadata.map(|meta| match meta.kind {
            RegisterFailureKind::TemporaryEndpoint | RegisterFailureKind::Transport => FailureScope::Endpoint,
            RegisterFailureKind::FlowFailed => FailureScope::Binding,
            RegisterFailureKind::AuthenticationRejected | RegisterFailureKind::IdentityRejected => FailureScope::Authentication,
            RegisterFailureKind::Negotiation => FailureScope::Negotiation,
            RegisterFailureKind::TerminalResponse => FailureScope::Binding,
            RegisterFailureKind::LocalFailure => FailureScope::Local,
        }).unwrap_or(FailureScope::Unknown);
        if matches!(reason_code.as_deref(), Some("ims_uicc_binding_invalid" | "ims_uicc_binding_changed"))
        { scope = FailureScope::Ownership; }
        Self { scope, reason_code, sip_status: metadata.and_then(|meta| meta.sip_status),
            auth_stage: metadata.map(|meta| match meta.stage {
                RegisterFailureStage::Initial => AuthStage::Initial, RegisterFailureStage::Authentication => AuthStage::Authentication,
                RegisterFailureStage::Authenticated => AuthStage::Authenticated,
            }), lease_negotiation_observed: metadata.is_some_and(|meta| meta.min_expires.is_some()),
            response_retry_after_seconds: metadata.and_then(|meta| match meta.retry_after {
                super::register::RetryAfter::DelaySeconds(seconds) => Some(seconds), _ => None,
            }),
            response_retry_after_invalid: metadata.is_some_and(|meta| matches!(meta.retry_after, super::register::RetryAfter::Invalid(_))),
            procedure: RegistrationProcedure::NotObserved, endpoint_index: None }
    }
}
fn safe_reason(value: &str) -> Option<String> {
    let token = value.split(':').next()?;
    if token.is_empty() || token.len() > 128 || !token.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') { return None; }
    ["ims_", "cellular_ims_", "vowifi_", "digest_", "sim_", "live_", "ike_", "eap_", "epdg_", "carrier_", "native_", "qmi_"]
        .iter().any(|prefix| token.starts_with(prefix)).then(|| token.to_owned())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitStatus { #[default] Unknown, Ready, Waiting, Stopped }
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WaitFact {
    pub status: WaitStatus,
    pub retry_after_seconds: Option<u64>,
    pub not_before: Option<String>,
}

/// Additive API v1. No identity values, raw SIP, keys, AIDs, tokens or peer
/// certificate material are members of this type. Profile ID is a config ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegistrationDiagnostics {
    pub version: u8,
    pub observed: bool,
    pub procedure: RegistrationProcedure,
    pub profile_id: Option<String>,
    pub profile_source: FactSource,
    pub identity_source: IdentityOrigin,
    pub declared_identity_source: IdentityPreference,
    pub identity_rule_status: IdentityRuleStatus,
    pub domain_source: FactSource,
    pub realm_source: FactSource,
    pub requested_family: IpFamily,
    pub granted_family: IpFamily,
    pub signaling_family: IpFamily,
    pub pcscf_source: FactSource,
    pub transport: SignalTransport,
    /// Discovered logical endpoint port (not an IPsec-renegotiated port).
    pub pcscf_port: Option<u16>,
    pub signaling_port: Option<u16>,
    pub socket_ready: bool,
    pub endpoint_index: Option<u16>,
    pub endpoint_count: Option<u16>,
    pub tls_verification: TlsVerification,
    pub tls_algorithms: Option<TlsAlgorithms>,
    pub offered_ipsec: Option<Vec<IpsecAlgorithms>>,
    pub negotiated_ipsec: Vec<IpsecAlgorithms>,
    pub security_mode: SecurityMode,
    pub digest_algorithm: Option<DigestAlgorithm>,
    pub rspauth_verified: Option<bool>,
    pub failure: Option<FailureFact>,
    pub wait: WaitFact,
    pub negotiated_from_success: bool,
    pub updated_at: Option<String>,
}
impl Default for RegistrationDiagnostics {
    fn default() -> Self {
        Self { version: 1, observed: false, procedure: RegistrationProcedure::NotObserved, profile_id: None, profile_source: FactSource::Unknown,
            identity_source: IdentityOrigin::Unknown, declared_identity_source: IdentityPreference::Unknown,
            identity_rule_status: IdentityRuleStatus::NotObserved, domain_source: FactSource::Unknown, realm_source: FactSource::Unknown,
            requested_family: IpFamily::Unknown, granted_family: IpFamily::Unknown, signaling_family: IpFamily::Unknown,
            pcscf_source: FactSource::Unknown, transport: SignalTransport::Unknown, pcscf_port: None, signaling_port: None, socket_ready: false,
            endpoint_index: None, endpoint_count: None, tls_verification: TlsVerification::NotObserved, tls_algorithms: None,
            offered_ipsec: None, negotiated_ipsec: Vec::new(), security_mode: SecurityMode::Unknown,
            digest_algorithm: None, rspauth_verified: None, failure: None, wait: WaitFact::default(),
            negotiated_from_success: false, updated_at: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticsToken { pub generation: u64, pub serial: u64 }

#[derive(Debug, Clone, Default)]
pub struct DiagnosticsState {
    scope: Option<DiagnosticsToken>,
    pub facts: RegistrationDiagnostics,
    deadline: Option<Instant>,
}
impl DiagnosticsState {
    pub fn begin(&mut self, scope: DiagnosticsToken) -> bool {
        if self.scope.is_some_and(|current| scope.generation < current.generation
            || (scope.generation == current.generation && scope.serial <= current.serial)) { return false; }
        *self = Self { scope: Some(scope), ..Self::default() };
        self.facts.observed = true;
        self.facts.procedure = RegistrationProcedure::Initial;
        self.touch();
        true
    }
    pub fn apply(&mut self, token: DiagnosticsToken, generation: u64, change: impl FnOnce(&mut Self)) -> bool {
        if token.generation != generation || self.scope != Some(token) { return false; }
        change(self); self.touch(); true
    }
    pub fn token(&self, generation: u64) -> Option<DiagnosticsToken> { self.scope.filter(|token| token.generation == generation) }
    pub fn discard_stale(&mut self, generation: u64) {
        if self.scope.is_some_and(|scope| scope.generation != generation) { *self = Self::default(); }
    }
    pub fn report(&self) -> RegistrationDiagnostics { self.report_at(Instant::now()) }
    pub fn report_at(&self, now: Instant) -> RegistrationDiagnostics {
        if self.scope.is_none() { return RegistrationDiagnostics::default(); }
        let mut result = self.facts.clone();
        if let Some(deadline) = self.deadline {
            let remaining = deadline.saturating_duration_since(now);
            if remaining.is_zero() { result.wait = WaitFact { status: WaitStatus::Ready, ..Default::default() }; }
            else { result.wait.retry_after_seconds = Some(remaining.as_secs().saturating_add(u64::from(remaining.subsec_nanos() != 0))); }
        }
        result
    }
    pub fn profile(&mut self, id: &str, source: FactSource, identity_preference: &str) {
        self.facts.profile_id = (!id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)).then(|| id.to_owned());
        self.facts.profile_source = source;
        self.facts.declared_identity_source = IdentityPreference::parse(identity_preference);
    }
    pub fn identity(&mut self, actual: IdentityOrigin, domain: FactSource, realm: FactSource) {
        self.facts.identity_source = actual; self.facts.domain_source = domain; self.facts.realm_source = realm;
        self.facts.identity_rule_status = match self.facts.declared_identity_source {
            IdentityPreference::CarrierDeviceModel | IdentityPreference::Other => IdentityRuleStatus::DeclaredOnly,
            _ if actual == IdentityOrigin::Isim => IdentityRuleStatus::UiccResolved,
            _ if actual != IdentityOrigin::Unknown => IdentityRuleStatus::StandardDerived,
            _ => IdentityRuleStatus::NotObserved,
        };
    }
    pub fn start_discovery(&mut self) {
        self.facts.pcscf_source = FactSource::Unknown; self.facts.transport = SignalTransport::Unknown;
        self.facts.pcscf_port = None; self.facts.signaling_port = None; self.facts.socket_ready = false;
        self.facts.endpoint_index = None; self.facts.endpoint_count = None;
        self.facts.signaling_family = IpFamily::Unknown; self.facts.tls_verification = TlsVerification::NotObserved;
        self.facts.tls_algorithms = None;
    }
    pub fn endpoint(&mut self, source: FactSource, transport: SipTransport, address: std::net::SocketAddr, index: usize, count: usize) {
        self.facts.pcscf_source = source; self.facts.transport = transport.into();
        self.facts.pcscf_port = Some(address.port()); self.facts.signaling_port = None; self.facts.socket_ready = false;
        self.facts.signaling_family = IpFamily::address(address.ip());
        self.facts.endpoint_index = u16::try_from(index).ok(); self.facts.endpoint_count = u16::try_from(count).ok();
        self.facts.tls_verification = if transport == SipTransport::Tls { TlsVerification::Pending } else { TlsVerification::NotApplicable };
        self.facts.tls_algorithms = None;
    }
    pub fn signaling_route(&mut self, route: super::context::ImsRoute) {
        self.facts.transport = route.transport.into(); self.facts.signaling_family = IpFamily::address(route.local_addr.ip());
        self.facts.signaling_port = Some(route.pcscf_addr.port()); self.facts.socket_ready = true;
    }
    pub fn offer(&mut self, value: Option<&str>) { self.facts.offered_ipsec = Some(value.map(ipsec_algorithms).unwrap_or_default()); }
    pub fn established_tls(&mut self, info: Option<TlsAlgorithms>) {
        if let Some(info) = info { self.facts.tls_verification = TlsVerification::Verified; self.facts.tls_algorithms = Some(info); }
    }
    pub fn accepted(&mut self, transport: SipTransport, security_verify: Option<&str>, digest: Option<&str>, rspauth: Option<bool>) {
        self.facts.security_mode = if security_verify.is_some() { SecurityMode::Ipsec }
            else if transport == SipTransport::Tls { SecurityMode::Tls } else { SecurityMode::Plain };
        self.facts.negotiated_ipsec = security_verify.map(ipsec_algorithms).unwrap_or_default();
        self.facts.digest_algorithm = digest.and_then(DigestAlgorithm::parse);
        self.facts.rspauth_verified = rspauth; self.facts.failure = None;
        self.facts.negotiated_from_success = true;
        self.wait_ready();
    }
    pub fn failure_in_scope(&mut self, code: &str, metadata: Option<RegisterFailureMetadata>, fallback: FailureScope) {
        self.failure(code, metadata);
        if let Some(failure) = self.facts.failure.as_mut() {
            if failure.scope == FailureScope::Unknown { failure.scope = fallback; }
        }
    }
    pub fn failure(&mut self, code: &str, metadata: Option<RegisterFailureMetadata>) {
        let mut failure = FailureFact::new(code, metadata);
        failure.procedure = self.facts.procedure;
        failure.endpoint_index = self.facts.endpoint_index;
        if failure.scope == FailureScope::Unknown {
            if let Some(previous) = self.facts.failure.as_ref().filter(|previous|
                previous.reason_code == failure.reason_code && previous.endpoint_index == failure.endpoint_index && previous.procedure == failure.procedure)
            { failure.scope = previous.scope; }
        }
        self.facts.failure = Some(failure);
    }
    pub fn wait_ready(&mut self) { self.deadline = None; self.facts.wait = WaitFact { status: WaitStatus::Ready, ..Default::default() }; }
    pub fn wait_until(&mut self, deadline: Instant) {
        self.deadline = Some(deadline);
        let duration = deadline.saturating_duration_since(Instant::now());
        let not_before = chrono::Duration::from_std(duration).ok().and_then(|delta| chrono::Utc::now().checked_add_signed(delta)).map(|time| time.to_rfc3339());
        self.facts.wait = WaitFact { status: WaitStatus::Waiting, retry_after_seconds: None, not_before };
    }
    pub fn wait_stopped(&mut self) { self.deadline = None; self.facts.wait = WaitFact { status: WaitStatus::Stopped, ..Default::default() }; }
    fn touch(&mut self) { self.facts.updated_at = Some(chrono::Utc::now().to_rfc3339()); }
}

#[cfg(test)]
#[path = "registration_diagnostics_tests.rs"]
mod tests;
