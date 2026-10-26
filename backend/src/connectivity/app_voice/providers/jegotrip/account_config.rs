//! Owned imports for independent application accounts, never SIM provisioning.
//! Private JSON follows the machine-local data.db convention; it is NOT encrypted
//! at rest and must never be exposed through an API or diagnostic/export path.
use super::{
    credentials::{ImportedCredentials, Secret},
    legacy_config::ConfigDecodeKey,
    packet_transport::PacketRoute,
    rpc_stream::{RpcProperty, MAX_FIELD_BYTES, MAX_MAP_ENTRIES},
    runtime_backend::{RuntimeBootstrapInputs, RuntimeProfile, RuntimeProfileInputs},
    runtime_provision::{AccountTrust, AccountTrustInputs},
    runtime_connector::{validate_explicit_profile, ConnectionInput},
};
use serde::{ser::SerializeSeq, Deserialize, Serialize, Serializer};
use std::{fmt, io, net::IpAddr, time::Duration};
use zeroize::{Zeroize, Zeroizing};

const INVALID: &str = "app_account_configuration_invalid";
const MAX_PRIVATE_BYTES: usize = 256 * 1024;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountConfig {
    pub credentials: ImportedCredentials,
    pub profile: ProfileConfig,
    pub bootstrap: Option<BootstrapConfig>,
    pub call_media: Option<CallMediaConfig>,
}
impl fmt::Debug for AccountConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AccountConfig(<redacted>)") }
}

/// Only the already-supported, explicit native TLS carrier can be imported.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileRoute { DirectTlsCarrier }
impl fmt::Debug for ProfileRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ProfileRoute(<redacted>)") }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileProperty { pub key: Vec<u8>, pub value: Vec<u8> }
impl Drop for ProfileProperty {
    fn drop(&mut self) { self.key.zeroize(); self.value.zeroize(); }
}
impl fmt::Debug for ProfileProperty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ProfileProperty(<redacted>)") }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncomingBinding {
    pub object_id: Vec<u8>,
    pub domain: Vec<u8>,
    pub receiver_kind: Vec<u8>,
}
impl Drop for IncomingBinding {
    fn drop(&mut self) { self.object_id.zeroize(); self.domain.zeroize(); self.receiver_kind.zeroize(); }
}
impl fmt::Debug for IncomingBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("IncomingBinding(<redacted>)") }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallIncomingConfig{pub object_id:Vec<u8>,pub domain:Vec<u8>}
impl Drop for CallIncomingConfig{fn drop(&mut self){self.object_id.zeroize();self.domain.zeroize();}}
impl fmt::Debug for CallIncomingConfig{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CallIncomingConfig(<redacted>)")}}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapConfig {
    pub account_authority: Secret,
    pub account_root_der_base64: Secret,
    pub account_enable_sni: bool,
    pub routing: Vec<ProfileProperty>,
    pub handoff_timeout_ms: u64,
}
impl fmt::Debug for BootstrapConfig {
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("BootstrapConfig(<redacted>)")}
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfig {
    pub route: ProfileRoute,
    pub authority: Secret,
    pub enable_sni: bool,
    pub identity_type: Secret,
    pub domain_name: Option<Secret>,
    pub domain_id: Option<Secret>,
    pub application_id: Option<Secret>,
    pub device_info_cc: Option<Secret>,
    pub client_properties: Vec<ProfileProperty>,
    pub include_device_id: bool,
    pub login_routing: Vec<ProfileProperty>,
    pub gateway_uri: Secret,
    pub gateway_metadata: Vec<ProfileProperty>,
    pub poll_routing: Vec<ProfileProperty>,
    pub logout_routing: Vec<ProfileProperty>,
    pub incoming: Option<IncomingBinding>,
    pub call_incoming:Option<CallIncomingConfig>,
    pub call_control:Option<Vec<ProfileProperty>>,
    pub connect_timeout_ms: u64,
    pub handshake_timeout_ms: u64,
    pub partial_timeout_ms: u64,
    pub io_timeout_ms: u64,
    pub rpc_timeout_ms: u64,
}
impl fmt::Debug for ProfileConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ProfileConfig(<redacted>)") }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileBytes { pub value: Vec<u8> }
impl Drop for ProfileBytes { fn drop(&mut self) { self.value.zeroize(); } }
impl fmt::Debug for ProfileBytes { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ProfileBytes(<redacted>)") } }

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallMediaConfig {
    pub authority: Secret, pub root_der_base64: Secret, pub enable_sni: bool,
    pub routing: Vec<ProfileProperty>, pub features: Vec<ProfileBytes>, pub properties: Vec<ProfileProperty>,
    pub reference_decode_key: Option<Secret>, pub audio_bind_ip: IpAddr, pub allow_plain_media: bool,
    pub trunk: Option<crate::platform::config::TrunkProfileConfig>,
}
impl fmt::Debug for CallMediaConfig { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("CallMediaConfig(<redacted>)") } }
impl Drop for CallMediaConfig { fn drop(&mut self) { if let Some(trunk) = &mut self.trunk { trunk.secret.zeroize(); } } }

fn required_secret(value:&Option<Secret>)->Result<&str,&'static str>{
    let value=value.as_ref().ok_or(INVALID)?.expose_to_backend();
    if value.trim().is_empty(){return Err(INVALID);}Ok(value)
}
fn borrowed(properties: &[ProfileProperty]) -> Result<Vec<RpcProperty<'_>>, &'static str> {
    if properties.len() > MAX_MAP_ENTRIES { return Err(INVALID); }
    Ok(properties.iter().map(|p| RpcProperty { key: &p.key, value: &p.value }).collect())
}
impl ProfileConfig {
    fn validate_timeouts(&self) -> Result<(), &'static str> {
        for (value, maximum) in [(self.connect_timeout_ms, 10_000),
            (self.handshake_timeout_ms, 30_000), (self.partial_timeout_ms, 30_000),
            (self.io_timeout_ms, 30_000), (self.rpc_timeout_ms, 60_000)] {
            if value == 0 || value > maximum { return Err(INVALID); }
        }
        Ok(())
    }
}
impl AccountConfig {
    fn reference_key(&self) -> Result<Option<ConfigDecodeKey>, &'static str> {
        self.credentials.sdk.reference_decode_key.as_ref().map(|key| {
            if key.expose_to_backend().trim().is_empty() { return Err(INVALID); }
            ConfigDecodeKey::supplied(key.expose_to_backend().as_bytes()).map_err(|_| INVALID)
        }).transpose()
    }

    /// Copies the EXACT supplied binary properties, never re-encodes them as text.
    /// The reference decode context comes exclusively from sdk.reference_decode_key.
    pub fn runtime_profile(&self) -> Result<RuntimeProfile, &'static str> {
        let p = &self.profile;
        p.validate_timeouts()?;
        for required in [&p.authority, &p.identity_type, &p.gateway_uri] {
            if required.expose_to_backend().trim().is_empty() { return Err(INVALID); }
        }
        let (domain_name,domain_id,application_id,cc)=if self.bootstrap.is_some(){
            if [&p.domain_name,&p.domain_id,&p.application_id,&p.device_info_cc].iter().any(|value|value.is_some()){return Err(INVALID);}
            ("","","","")
        }else{(required_secret(&p.domain_name)?,required_secret(&p.domain_id)?,required_secret(&p.application_id)?,
            p.device_info_cc.as_ref().map(Secret::expose_to_backend).unwrap_or(""))};
        let client = borrowed(&p.client_properties)?;
        let login = borrowed(&p.login_routing)?;
        let gateway = borrowed(&p.gateway_metadata)?;
        let poll = borrowed(&p.poll_routing)?;
        let logout = borrowed(&p.logout_routing)?;
        let inputs=RuntimeProfileInputs {
            route: match p.route { ProfileRoute::DirectTlsCarrier => PacketRoute::DirectTlsCarrier },
            authority: p.authority.expose_to_backend(), enable_sni: p.enable_sni,
            identity_type: p.identity_type.expose_to_backend(), domain_name,
            domain_id, application_id,
            device_info_cc: cc, client_properties: &client,
            include_device_id: p.include_device_id, login_routing: &login,
            gateway_uri: p.gateway_uri.expose_to_backend(), gateway_metadata: &gateway,
            poll_routing: &poll, logout_routing: &logout,
            incoming: p.incoming.as_ref().map(|b| super::rpc_incoming::IncomingBinding {
                object_id: &b.object_id, domain: &b.domain, receiver_kind: &b.receiver_kind,
            }),
            reference_decode_key: self.reference_key()?,
            handshake_timeout: Duration::from_millis(p.handshake_timeout_ms),
            partial_timeout: Duration::from_millis(p.partial_timeout_ms),
            io_timeout: Duration::from_millis(p.io_timeout_ms),
            rpc_timeout: Duration::from_millis(p.rpc_timeout_ms),
        };
        let profile=if let Some(bootstrap)=&self.bootstrap{
            if bootstrap.handoff_timeout_ms==0||bootstrap.handoff_timeout_ms>30_000
                ||bootstrap.account_root_der_base64.expose_to_backend().len()>16_384{return Err(INVALID);}
            let routing=borrowed(&bootstrap.routing)?;
            let account_trust=AccountTrust::supplied(AccountTrustInputs{authority:bootstrap.account_authority.expose_to_backend(),
                root_der_base64:bootstrap.account_root_der_base64.expose_to_backend(),enable_sni:bootstrap.account_enable_sni}).map_err(|_|INVALID)?;
            let client_key=self.credentials.sdk.client_config_decode_key.as_ref().map(|key|ConfigDecodeKey::supplied(key.expose_to_backend().as_bytes()).map_err(|_|INVALID)).transpose()?;
            RuntimeProfile::supplied_bootstrap(inputs,RuntimeBootstrapInputs{account_trust,routing:&routing,client_config_decode_key:client_key,
                handoff_timeout:Duration::from_millis(bootstrap.handoff_timeout_ms)}).map_err(|_|INVALID)?
        }else{RuntimeProfile::supplied(inputs).map_err(|_|INVALID)?};
        let profile=profile.with_call_receiver(p.call_incoming.as_ref().map(|binding|super::call_binding::CallIncomingBinding{object_id:&binding.object_id,domain:&binding.domain})).map_err(|_|INVALID)?;
        if let Some(properties)=&p.call_control{profile.with_call_control(&borrowed(properties)?).map_err(|_|INVALID)}else{Ok(profile)}
    }

    pub(super) fn runtime_calls(&self) -> Result<Option<super::call_settings::CallSettings>, &'static str> {
        let Some(media) = &self.call_media else { return Ok(None); };
        if self.profile.call_incoming.is_none() || self.profile.call_control.is_none() { return Err(INVALID); }
        let routing = borrowed(&media.routing)?; let properties = borrowed(&media.properties)?;
        let features = media.features.iter().map(|feature| feature.value.as_slice()).collect::<Vec<_>>();
        let key = media.reference_decode_key.as_ref().map(|key| ConfigDecodeKey::supplied(key.expose_to_backend().as_bytes()).map_err(|_| INVALID)).transpose()?;
        super::call_settings::CallSettings::supplied(super::call_settings::Inputs {
            authority: media.authority.expose_to_backend(), root: media.root_der_base64.expose_to_backend(), enable_sni: media.enable_sni,
            routing: &routing, features: &features, properties: &properties, key,
            connect_timeout: Duration::from_millis(self.profile.connect_timeout_ms), handshake_timeout: Duration::from_millis(self.profile.handshake_timeout_ms),
            partial_timeout: Duration::from_millis(self.profile.partial_timeout_ms), io_timeout: Duration::from_millis(self.profile.io_timeout_ms),
            rpc_timeout: Duration::from_millis(self.profile.rpc_timeout_ms), bind_ip: media.audio_bind_ip, allow_plain_media: media.allow_plain_media,
            trunk: media.trunk.as_ref(),
        }).map(Some)
    }

    /// Pure admission only: no DNS, network, SDK execution or readiness state.
    pub fn validate(&self, now: u64) -> Result<(), &'static str> {
        self.credentials.validate(now).map_err(|_| INVALID)?;
        let sdk = &self.credentials.sdk;
        let hosting = &self.credentials.hosting;
        let selector = sdk.selected_session_id.as_ref().ok_or(INVALID)?;
        // Match NativeHostingBackend's LoginCapture text and identity rules.
        for value in [&sdk.account_identity, &sdk.phone_login_token, &sdk.application_key,
            &sdk.root_der_base64, selector].into_iter().chain(sdk.device_id.iter()) {
            let text = value.expose_to_backend();
            if text.trim().is_empty() || text.contains("ENC(") || text.len() > MAX_FIELD_BYTES {
                return Err(INVALID);
            }
        }
        for value in [&sdk.server_address, &hosting.sip_number, &hosting.sip_password,
            &hosting.authname_prefix, &hosting.channel_id].into_iter()
            .chain(sdk.client_config_decode_key.iter()).chain(sdk.reference_decode_key.iter()) {
            if value.expose_to_backend().trim().is_empty() { return Err(INVALID); }
        }
        if hosting.delete_int_code.trim().is_empty()
            || sdk.account_identity.expose_to_backend().bytes().any(|b| matches!(b, b':' | b'[' | b']' | b'@'))
            || sdk.device_id.is_some() && !self.profile.include_device_id
            || self.profile.identity_type.expose_to_backend().len()
                .checked_add(1).and_then(|n| n.checked_add(sdk.account_identity.expose_to_backend().len()))
                .map_or(true, |n| n > MAX_FIELD_BYTES) {
            return Err(INVALID);
        }
        // Constructing the profile is bounded and validates all property maps,
        // incoming binding, required text, supported route and explicit deadlines.
        let _profile = self.runtime_profile()?;
        let _calls = self.runtime_calls()?;
        let key = self.reference_key()?;
        validate_explicit_profile(ConnectionInput {
            server_reference: sdk.server_address.expose_to_backend(),
            authority: self.profile.authority.expose_to_backend(),
            root_der_base64: sdk.root_der_base64.expose_to_backend(),
            enable_sni: self.profile.enable_sni,
            reference_decode_key: key.as_ref(),
        }).map_err(|_| INVALID)
    }

    /// Internal persistence only, deliberately NOT a Serialize implementation.
    pub fn encode_private(&self) -> Result<Zeroizing<Vec<u8>>, &'static str> {
        let mirror = PrivateEnvelopeRef { format_version: 1, config: PrivateConfigRef::new(self) };
        // Reserve the entire bounded buffer to avoid secret-bearing old Vec
        // allocations being freed unwiped during growth.
        let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_PRIVATE_BYTES));
        // Bound the writer itself, so failures/oversized imports wipe partial JSON.
        serde_json::to_writer(PrivateWriter(&mut bytes), &mirror).map_err(|_| INVALID)?;
        Ok(bytes)
    }

    /// Bounded versioned private import. Errors never include serde diagnostics.
    /// Input copy and owned fields wipe on drop. serde's internal parser scratch
    /// allocations cannot be controlled here; this is not a secure-memory claim.
    pub fn decode_private(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.is_empty() || bytes.len() > MAX_PRIVATE_BYTES { return Err(INVALID); }
        let input = Zeroizing::new(bytes.to_vec());
        let envelope: PrivateEnvelope = serde_json::from_slice(&input).map_err(|_| INVALID)?;
        if envelope.format_version != 1 { return Err(INVALID); }
        Ok(envelope.config)
    }
}

struct PrivateWriter<'a>(&'a mut Zeroizing<Vec<u8>>);
impl io::Write for PrivateWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_PRIVATE_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, INVALID));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivateEnvelope { format_version: u32, config: AccountConfig }
#[derive(Serialize)]
struct PrivateEnvelopeRef<'a> { format_version: u32, config: PrivateConfigRef<'a> }
#[derive(Serialize)]
struct PrivateConfigRef<'a> { credentials: PrivateCredentialsRef<'a>, profile: PrivateProfileRef<'a>, bootstrap:Option<PrivateBootstrapRef<'a>>,
    call_media: Option<PrivateCallMediaRef<'a>> }
#[derive(Serialize)]
struct PrivateCallMediaRef<'a> { authority: &'a str, root_der_base64: &'a str, enable_sni: bool,
    routing: PrivateProperties<'a>, features: PrivateFeatures<'a>, properties: PrivateProperties<'a>, reference_decode_key: Option<&'a str>,
    audio_bind_ip: IpAddr, allow_plain_media: bool, trunk: Option<&'a crate::platform::config::TrunkProfileConfig> }
#[derive(Serialize)]
struct PrivateFeatureRef<'a> { value: &'a [u8] }
struct PrivateFeatures<'a>(&'a [ProfileBytes]);
impl Serialize for PrivateFeatures<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut values = serializer.serialize_seq(Some(self.0.len()))?;
        for feature in self.0 { values.serialize_element(&PrivateFeatureRef { value: &feature.value })?; } values.end()
    }
}
#[derive(Serialize)]
struct PrivateBootstrapRef<'a>{account_authority:&'a str,account_root_der_base64:&'a str,account_enable_sni:bool,
    routing:PrivateProperties<'a>,handoff_timeout_ms:u64}
#[derive(Serialize)]
struct PrivateCredentialsRef<'a> { sdk: PrivateSdkRef<'a>, hosting: PrivateHostingRef<'a> }
#[derive(Serialize)]
struct PrivateSdkRef<'a> {
    account_identity: &'a str, phone_login_token: &'a str, application_key: &'a str,
    server_address: &'a str, device_id: Option<&'a str>, selected_session_id: Option<&'a str>,
    root_der_base64: &'a str, client_config_decode_key: Option<&'a str>, reference_decode_key: Option<&'a str>,
    tls_only: bool, expires_at_unix_seconds: Option<u64>,
}
#[derive(Serialize)]
struct PrivateHostingRef<'a> {
    sip_number: &'a str, sip_password: &'a str, authname_prefix: &'a str, channel_id: &'a str,
    delete_int_code: &'a str, enrollment_confirmed: bool,
}
#[derive(Serialize)]
struct PrivatePropertyRef<'a> { key: &'a [u8], value: &'a [u8] }
struct PrivateProperties<'a>(&'a [ProfileProperty]);
impl Serialize for PrivateProperties<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for p in self.0 {
            seq.serialize_element(&PrivatePropertyRef { key: &p.key, value: &p.value })?;
        }
        seq.end()
    }
}
#[derive(Serialize)]
struct PrivateIncomingRef<'a> { object_id: &'a [u8], domain: &'a [u8], receiver_kind: &'a [u8] }
#[derive(Serialize)]
struct PrivateCallIncomingRef<'a>{object_id:&'a [u8],domain:&'a [u8]}
#[derive(Serialize)]
struct PrivateProfileRef<'a> {
    route: &'static str, authority: &'a str, enable_sni: bool, identity_type: &'a str,
    domain_name: Option<&'a str>, domain_id: Option<&'a str>, application_id: Option<&'a str>, device_info_cc: Option<&'a str>,
    client_properties: PrivateProperties<'a>, include_device_id: bool,
    login_routing: PrivateProperties<'a>, gateway_uri: &'a str,
    gateway_metadata: PrivateProperties<'a>, poll_routing: PrivateProperties<'a>,
    logout_routing: PrivateProperties<'a>, incoming: Option<PrivateIncomingRef<'a>>,call_incoming:Option<PrivateCallIncomingRef<'a>>,call_control:Option<PrivateProperties<'a>>,
    connect_timeout_ms: u64, handshake_timeout_ms: u64, partial_timeout_ms: u64,
    io_timeout_ms: u64, rpc_timeout_ms: u64,
}
fn private_properties(properties: &[ProfileProperty]) -> PrivateProperties<'_> {
    PrivateProperties(properties)
}
fn exposed(secret: &Option<Secret>) -> Option<&str> { secret.as_ref().map(Secret::expose_to_backend) }
impl<'a> PrivateConfigRef<'a> {
    fn new(config: &'a AccountConfig) -> Self {
        let s = &config.credentials.sdk;
        let h = &config.credentials.hosting;
        let p = &config.profile;
        Self {
            credentials: PrivateCredentialsRef {
                sdk: PrivateSdkRef {
                    account_identity: s.account_identity.expose_to_backend(), phone_login_token: s.phone_login_token.expose_to_backend(),
                    application_key: s.application_key.expose_to_backend(), server_address: s.server_address.expose_to_backend(),
                    device_id: exposed(&s.device_id), selected_session_id: exposed(&s.selected_session_id),
                    root_der_base64: s.root_der_base64.expose_to_backend(), client_config_decode_key: exposed(&s.client_config_decode_key),
                    reference_decode_key: exposed(&s.reference_decode_key), tls_only: s.tls_only,
                    expires_at_unix_seconds: s.expires_at_unix_seconds,
                },
                hosting: PrivateHostingRef {
                    sip_number: h.sip_number.expose_to_backend(), sip_password: h.sip_password.expose_to_backend(),
                    authname_prefix: h.authname_prefix.expose_to_backend(), channel_id: h.channel_id.expose_to_backend(),
                    delete_int_code: &h.delete_int_code, enrollment_confirmed: h.enrollment_confirmed,
                },
            },
            profile: PrivateProfileRef {
                route: match p.route { ProfileRoute::DirectTlsCarrier => "direct_tls_carrier" },
                authority: p.authority.expose_to_backend(), enable_sni: p.enable_sni,
                identity_type: p.identity_type.expose_to_backend(), domain_name: exposed(&p.domain_name),
                domain_id: exposed(&p.domain_id), application_id: exposed(&p.application_id),
                device_info_cc: exposed(&p.device_info_cc), client_properties: private_properties(&p.client_properties),
                include_device_id: p.include_device_id, login_routing: private_properties(&p.login_routing),
                gateway_uri: p.gateway_uri.expose_to_backend(), gateway_metadata: private_properties(&p.gateway_metadata),
                poll_routing: private_properties(&p.poll_routing), logout_routing: private_properties(&p.logout_routing),
                incoming: p.incoming.as_ref().map(|b| PrivateIncomingRef { object_id: &b.object_id, domain: &b.domain, receiver_kind: &b.receiver_kind }),
                call_incoming:p.call_incoming.as_ref().map(|binding|PrivateCallIncomingRef{object_id:&binding.object_id,domain:&binding.domain}),
                call_control:p.call_control.as_ref().map(|properties|PrivateProperties(properties)),
                connect_timeout_ms: p.connect_timeout_ms, handshake_timeout_ms: p.handshake_timeout_ms,
                partial_timeout_ms: p.partial_timeout_ms, io_timeout_ms: p.io_timeout_ms, rpc_timeout_ms: p.rpc_timeout_ms,
            },
            call_media: config.call_media.as_ref().map(|media| PrivateCallMediaRef { authority: media.authority.expose_to_backend(),
                root_der_base64: media.root_der_base64.expose_to_backend(), enable_sni: media.enable_sni,
                routing: PrivateProperties(&media.routing), features: PrivateFeatures(&media.features), properties: PrivateProperties(&media.properties),
                reference_decode_key: exposed(&media.reference_decode_key), audio_bind_ip: media.audio_bind_ip, allow_plain_media: media.allow_plain_media,
                trunk: media.trunk.as_ref() }),
            bootstrap:config.bootstrap.as_ref().map(|b|PrivateBootstrapRef{account_authority:b.account_authority.expose_to_backend(),
                account_root_der_base64:b.account_root_der_base64.expose_to_backend(),account_enable_sni:b.account_enable_sni,
                routing:private_properties(&b.routing),handoff_timeout_ms:b.handoff_timeout_ms}),
        }
    }
}

#[cfg(test)]
#[path = "account_config_tests.rs"]
mod tests;
