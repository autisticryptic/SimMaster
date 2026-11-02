//! Private installation/deployment context. Never a public credentials export.
use super::{Error, crypto::PhoneMaterial};
use super::super::{
    credentials::{Secret, SdkCredentials, HostingCredentials, ImportedCredentials},
    account_config::{AccountConfig, ProfileConfig, BootstrapConfig, CallMediaConfig, ReconnectPolicy},
};
use serde::{ser::SerializeSeq, Deserialize, Serialize};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{fmt, io::Read, path::Path};
use zeroize::Zeroizing;
use url::Url;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Deployment {
    pub version: u32,
    pub http: HttpDeployment,
    pub runtime: RuntimeTemplate,
}
impl fmt::Debug for Deployment { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AppLoginDeployment(<redacted>)") } }

/// Private database installation context. HTTP-only configuration permits App
/// authentication, never native/hosting readiness or a placeholder SDK login.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DatabaseConfiguration {
    pub(super) version: u32,
    pub(super) http: HttpDeployment,
    pub(super) runtime: Option<RuntimeTemplate>,
}
impl fmt::Debug for DatabaseConfiguration { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("JegotripDatabaseConfiguration(<redacted>)") } }
impl From<Deployment> for DatabaseConfiguration {
    fn from(value: Deployment) -> Self { Self { version:value.version, http:value.http, runtime:Some(value.runtime) } }
}
impl DatabaseConfiguration {
    pub(super) fn validate(&self) -> Result<(), Error> {
        if self.version != 1 { return Err(Error::new("app_login_deployment_invalid")); }
        self.http.validate()
    }
    pub(super) fn encode_private(&self) -> Result<Zeroizing<Vec<u8>>, Error> {
        self.validate()?;
        let mirror = PrivateDatabaseRef { version: self.version, http: PrivateHttpRef::new(&self.http),
            runtime: self.runtime.as_ref().map(RuntimeTemplate::private_mirror) };
        // Bound the serializer itself, not only its completed allocation. Secret
        // never implements Serialize; only private borrowed mirrors write here.
        let mut bytes = Zeroizing::new(Vec::with_capacity(256 * 1024));
        serde_json::to_writer(PrivateDatabaseWriter(&mut bytes), &mirror).map_err(|_| Error::new("app_login_deployment_invalid"))?;
        Ok(bytes)
    }
    pub(super) fn decode_private(bytes:&[u8]) -> Result<Self,Error> {
        if bytes.is_empty() || bytes.len()>256*1024 { return Err(Error::new("app_login_deployment_invalid")); }
        let value:Self=serde_json::from_slice(bytes).map_err(|_|Error::new("app_login_deployment_invalid"))?;
        value.validate()?; Ok(value)
    }
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BodyProtection { pub secret_id: Secret, pub secret_value: Secret }
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Endpoints {
    pub login: String, pub user_info: String, pub phone_token: String,
    pub voip_status: String, pub call_status: String, pub enable_hosting: String,
}
pub(super) fn valid_version_page(url:&str)->bool {
    if url.len()>512 || url.chars().any(char::is_control) { return false; }
    let Some(rest)=url.strip_prefix("https://") else { return false; };
    let host=rest.split(['/','?','#']).next().unwrap_or_default().to_ascii_lowercase();
    let host=host.split('@').next_back().unwrap_or_default();
    let host=host.split(':').next().unwrap_or_default();
    !host.is_empty() && (host=="jegotrip.cn" || host.ends_with(".jegotrip.cn"))
}

pub(super) fn valid_app_version(value:&str)->bool {
    !value.is_empty() && value.len()<=24 && value.bytes().all(|b|b.is_ascii_digit()||b==b'.')
        && value.split('.').all(|part|!part.is_empty())
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HttpPair { pub name: Secret, pub value: Secret }
impl fmt::Debug for HttpPair { fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result { f.write_str("HttpPair(<redacted>)") } }

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HttpDeployment {
    pub base_url: Secret,
    pub anonymous_token: Secret,
    pub signature_salt: Secret,
    pub password_salt: Secret,
    pub sip_password_key: Secret,
    pub device_id: Secret,
    pub endpoints: Endpoints,
    /// None is the original non-encrypted AppConfig branch, not a TLS downgrade.
    pub body_protection: Option<BodyProtection>,
    pub root_der_base64: Option<Secret>,
    /// Exact end-entity certificate pin (the App bundles one such leaf in
    /// `assets/app2.cer`). Mutually exclusive with `root_der_base64`; when set,
    /// only this exact server certificate is accepted and no system root is
    /// consulted. It is an explicit installation value, never a fallback.
    #[serde(default)]
    pub pinned_server_certificate_der_base64: Option<Secret>,
    /// PKCS#1 RSAPublicKey DER of the pinned leaf. Required with the pin because
    /// the bundled v1 certificate cannot be parsed by webpki, so handshake
    /// signatures are verified with ring against this key.
    #[serde(default)]
    pub pinned_server_public_key_der_base64: Option<Secret>,
    /// Explicit installation values the reviewed service requires on every App
    /// call (for example a client-version gate). Bounded names/values only; both
    /// lists are opt-in and never inferred from the APK or a provider response.
    #[serde(default)]
    pub extra_query: Vec<HttpPair>,
    #[serde(default)]
    pub extra_headers: Vec<HttpPair>,
    /// Extra fields merged into every App JSON body before encryption.
    #[serde(default)]
    pub extra_body: Vec<HttpPair>,
    /// Client version used to replace the `$APP_VERSION` placeholder inside the
    /// extra query/header/body pairs. Either a static value or a file that is
    /// re-read on EVERY request, so an official App upgrade only needs the file
    /// (or stored configuration) to change - never a SimAdmin rebuild.
    #[serde(default)]
    pub app_version: Option<Secret>,
    #[serde(default)]
    pub app_version_file: Option<Secret>,
    /// Optional official page used to discover the current App version. The URL
    /// is restricted to `https://…jegotrip.cn/…`, and any failure falls back to
    /// the file or stored value so a page redesign can never block a login.
    #[serde(default)]
    pub app_version_web_url: Option<Secret>,
    pub user_agent: Option<Secret>,
    pub enroll_id: Option<Secret>,
}
impl fmt::Debug for HttpDeployment { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AppHttpDeployment(<redacted>)") } }
impl HttpDeployment {
    pub fn validate(&self) -> Result<(), Error> {
        let invalid = || Error::new("app_login_deployment_invalid");
        let base = self.base()?;
        if !base.path().ends_with('/') { return Err(invalid()); }
        for value in [&self.anonymous_token, &self.signature_salt, &self.password_salt, &self.sip_password_key, &self.device_id] {
            let text=value.expose_to_backend();
            if text.is_empty() || text.len()>16_384 || text.chars().any(char::is_control) { return Err(invalid()); }
        }
        if self.signature_salt.expose_to_backend().len()<8 || self.sip_password_key.expose_to_backend().len()!=24 { return Err(invalid()); }
        if let Some(p)=&self.body_protection {
            for v in [&p.secret_id,&p.secret_value] {
                let s=v.expose_to_backend(); if s.is_empty() || s.len()>4096 || s.chars().any(char::is_control) || s.contains(';') { return Err(invalid()); }
            }
        }
        for s in [&self.user_agent,&self.enroll_id].into_iter().flatten() {
            if s.expose_to_backend().len()>1024 || s.expose_to_backend().chars().any(char::is_control) { return Err(invalid()); }
        }
        if self.root_der_base64.is_some() && self.pinned_server_certificate_der_base64.is_some() { return Err(invalid()); }
        if self.pinned_server_certificate_der_base64.is_some() != self.pinned_server_public_key_der_base64.is_some() { return Err(invalid()); }
        if let Some(pin)=&self.pinned_server_certificate_der_base64 {
            let text=pin.expose_to_backend();
            if text.is_empty() || text.len()>87_400 || text.chars().any(char::is_control) { return Err(invalid()); }
            let der=STANDARD.decode(text).map_err(|_|invalid())?;
            if der.len()<200 || der.len()>65_536 || der.first()!=Some(&0x30) { return Err(invalid()); }
            let key=self.pinned_server_public_key_der_base64.as_ref().ok_or_else(invalid)?;
            let key_text=key.expose_to_backend();
            if key_text.is_empty() || key_text.len()>16_384 || key_text.chars().any(char::is_control) { return Err(invalid()); }
            let key_der=STANDARD.decode(key_text).map_err(|_|invalid())?;
            if key_der.len()<100 || key_der.len()>8_192 || key_der.first()!=Some(&0x30) { return Err(invalid()); }
        }
        for p in [&self.endpoints.login,&self.endpoints.user_info,&self.endpoints.phone_token,&self.endpoints.voip_status,&self.endpoints.call_status,&self.endpoints.enable_hosting] { self.endpoint(p)?; }
        if self.extra_query.len()>16 || self.extra_headers.len()>16 || self.extra_body.len()>16 { return Err(invalid()); }
        let reserved=["n_token","lang","timestamp","sign","token"];
        for pair in self.extra_query.iter().chain(self.extra_headers.iter()).chain(self.extra_body.iter()) {
            let name=pair.name.expose_to_backend();let value=pair.value.expose_to_backend();
            if name.is_empty() || name.len()>64 || !name.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'-'|b'_'))
                || value.is_empty() || value.len()>256 || value.chars().any(|c|c.is_control()) { return Err(invalid()); }
        }
        if self.extra_query.iter().any(|pair|reserved.contains(&pair.name.expose_to_backend())) { return Err(invalid()); }
        if self.app_version.is_some() && self.app_version_file.is_some() { return Err(invalid()); }
        if let Some(url)=&self.app_version_web_url {
            let text=url.expose_to_backend();
            if !valid_version_page(text) { return Err(invalid()); }
        }
        if let Some(value)=&self.app_version {
            if !valid_app_version(value.expose_to_backend()) { return Err(invalid()); }
        }
        if let Some(path)=&self.app_version_file {
            let text=path.expose_to_backend();
            if text.is_empty() || text.len()>512 || text.chars().any(char::is_control) || !text.starts_with('/')
                || text.split('/').any(|part|part==".."||part==".") { return Err(invalid()); }
        }
        Ok(())
    }
    pub fn base(&self) -> Result<Url,Error> {
        let url=Url::parse(self.base_url.expose_to_backend()).map_err(|_|Error::new("app_login_deployment_invalid"))?;
        if url.scheme()!="https" || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some()
            || url.query().is_some() || url.fragment().is_some() { return Err(Error::new("app_login_deployment_invalid")); }
        Ok(url)
    }
    pub fn endpoint(&self, path: &str) -> Result<Url, Error> {
        if path.is_empty() || path.len()>256 || path.starts_with('/') || path.contains(['?', '#', ':', '%', '\\'])
            || path.split('/').any(|p| p==".." || p==".") || path.chars().any(char::is_control) {
            return Err(Error::new("app_login_deployment_invalid"));
        }
        let base=self.base()?;
        let url=base.join(path).map_err(|_|Error::new("app_login_deployment_invalid"))?;
        if url.origin()!=base.origin() { return Err(Error::new("app_login_deployment_invalid")); }
        Ok(url)
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SdkContext {
    pub application_key: Secret, pub server_address: Secret, pub device_id: Option<Secret>,
    pub selected_session_id: Option<Secret>, pub root_der_base64: Secret,
    pub client_config_decode_key: Option<Secret>, pub reference_decode_key: Option<Secret>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HostingContext {
    pub authname_prefix: Secret, pub channel_id: Secret, pub delete_int_code: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeTemplate {
    pub sdk: SdkContext, pub hosting: HostingContext, pub profile: ProfileConfig,
    pub bootstrap: Option<BootstrapConfig>, pub call_media: Option<CallMediaConfig>,
    #[serde(default)] pub cleanup_on_disconnect: bool,
    pub reconnect: Option<ReconnectPolicy>,
}
impl RuntimeTemplate {
    fn private_mirror(&self) -> impl Serialize + '_ {
        use super::super::account_config::{private_profile_ref, private_bootstrap_ref, private_call_media_ref};
        PrivateRuntimeRef {
            sdk: PrivateSdkContextRef { application_key: self.sdk.application_key.expose_to_backend(), server_address: self.sdk.server_address.expose_to_backend(),
                device_id: self.sdk.device_id.as_ref().map(Secret::expose_to_backend), selected_session_id: self.sdk.selected_session_id.as_ref().map(Secret::expose_to_backend),
                root_der_base64: self.sdk.root_der_base64.expose_to_backend(), client_config_decode_key: self.sdk.client_config_decode_key.as_ref().map(Secret::expose_to_backend),
                reference_decode_key: self.sdk.reference_decode_key.as_ref().map(Secret::expose_to_backend) },
            hosting: PrivateHostingContextRef { authname_prefix: self.hosting.authname_prefix.expose_to_backend(), channel_id: self.hosting.channel_id.expose_to_backend(),
                delete_int_code: &self.hosting.delete_int_code },
            profile: private_profile_ref(&self.profile), bootstrap: self.bootstrap.as_ref().map(private_bootstrap_ref),
            call_media: self.call_media.as_ref().map(private_call_media_ref), cleanup_on_disconnect: self.cleanup_on_disconnect, reconnect: self.reconnect,
        }
    }
    /// Pure construction and validation before the business request. The result
    /// stays private to the attempt and cannot be stored/started until the server
    /// has confirmed business enablement. No placeholder account credentials.
    pub fn bind(self, identity: &str, material: PhoneMaterial) -> Result<AccountConfig, Error> {
        let config=AccountConfig {
            credentials: ImportedCredentials {
                sdk: SdkCredentials { account_identity: Secret::from_owned(identity.to_owned()), phone_login_token: material.token,
                    application_key:self.sdk.application_key,server_address:self.sdk.server_address,device_id:self.sdk.device_id,
                    selected_session_id:self.sdk.selected_session_id,root_der_base64:self.sdk.root_der_base64,
                    client_config_decode_key:self.sdk.client_config_decode_key,reference_decode_key:self.sdk.reference_decode_key,
                    tls_only:true,expires_at_unix_seconds:None },
                hosting: HostingCredentials {sip_number:Secret::from_owned(identity.to_owned()),sip_password:material.sip_password,
                    authname_prefix:self.hosting.authname_prefix,channel_id:self.hosting.channel_id,delete_int_code:self.hosting.delete_int_code,
                    enrollment_confirmed:true},
            }, profile:self.profile,bootstrap:self.bootstrap,call_media:self.call_media,
            cleanup_on_disconnect:self.cleanup_on_disconnect,reconnect:self.reconnect,
        };
        Ok(config)
    }
}

#[derive(Serialize)]
struct PrivateDatabaseRef<'a, R: Serialize> { version: u32, http: PrivateHttpRef<'a>, runtime: Option<R> }
#[derive(Serialize)]
struct PrivateBodyProtectionRef<'a> { secret_id: &'a str, secret_value: &'a str }
#[derive(Serialize)]
struct PrivateHttpPairRef<'a> { name: &'a str, value: &'a str }
struct PrivateHttpPairs<'a>(&'a [HttpPair]);
impl Serialize for PrivateHttpPairs<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for pair in self.0 { SerializeSeq::serialize_element(&mut seq, &PrivateHttpPairRef { name: pair.name.expose_to_backend(), value: pair.value.expose_to_backend() })?; }
        SerializeSeq::end(seq)
    }
}
#[derive(Serialize)]
struct PrivateHttpRef<'a> {
    base_url: &'a str, anonymous_token: &'a str, signature_salt: &'a str, password_salt: &'a str,
    sip_password_key: &'a str, device_id: &'a str, endpoints: &'a Endpoints,
    body_protection: Option<PrivateBodyProtectionRef<'a>>, root_der_base64: Option<&'a str>,
    pinned_server_certificate_der_base64: Option<&'a str>,
    pinned_server_public_key_der_base64: Option<&'a str>,
    extra_query: PrivateHttpPairs<'a>, extra_headers: PrivateHttpPairs<'a>, extra_body: PrivateHttpPairs<'a>,
    app_version: Option<&'a str>, app_version_file: Option<&'a str>, app_version_web_url: Option<&'a str>,
    user_agent: Option<&'a str>, enroll_id: Option<&'a str>,
}
impl<'a> PrivateHttpRef<'a> {
    fn new(h: &'a HttpDeployment) -> Self {
        Self { base_url: h.base_url.expose_to_backend(), anonymous_token: h.anonymous_token.expose_to_backend(),
            signature_salt: h.signature_salt.expose_to_backend(), password_salt: h.password_salt.expose_to_backend(),
            sip_password_key: h.sip_password_key.expose_to_backend(), device_id: h.device_id.expose_to_backend(), endpoints: &h.endpoints,
            body_protection: h.body_protection.as_ref().map(|b| PrivateBodyProtectionRef { secret_id: b.secret_id.expose_to_backend(), secret_value: b.secret_value.expose_to_backend() }),
            root_der_base64: h.root_der_base64.as_ref().map(Secret::expose_to_backend),
            pinned_server_certificate_der_base64: h.pinned_server_certificate_der_base64.as_ref().map(Secret::expose_to_backend),
            pinned_server_public_key_der_base64: h.pinned_server_public_key_der_base64.as_ref().map(Secret::expose_to_backend),
            extra_query: PrivateHttpPairs(&h.extra_query), extra_headers: PrivateHttpPairs(&h.extra_headers), extra_body: PrivateHttpPairs(&h.extra_body),
            app_version: h.app_version.as_ref().map(Secret::expose_to_backend),
            app_version_file: h.app_version_file.as_ref().map(Secret::expose_to_backend),
            app_version_web_url: h.app_version_web_url.as_ref().map(Secret::expose_to_backend),
            user_agent: h.user_agent.as_ref().map(Secret::expose_to_backend),
            enroll_id: h.enroll_id.as_ref().map(Secret::expose_to_backend) }
    }
}
#[derive(Serialize)]
struct PrivateSdkContextRef<'a> { application_key: &'a str, server_address: &'a str, device_id: Option<&'a str>, selected_session_id: Option<&'a str>,
    root_der_base64: &'a str, client_config_decode_key: Option<&'a str>, reference_decode_key: Option<&'a str> }
#[derive(Serialize)]
struct PrivateHostingContextRef<'a> { authname_prefix: &'a str, channel_id: &'a str, delete_int_code: &'a str }
#[derive(Serialize)]
struct PrivateRuntimeRef<'a, P: Serialize, B: Serialize, M: Serialize> { sdk: PrivateSdkContextRef<'a>, hosting: PrivateHostingContextRef<'a>,
    profile: P, bootstrap: Option<B>, call_media: Option<M>, cleanup_on_disconnect: bool, reconnect: Option<ReconnectPolicy> }
pub(super) struct PrivateDatabaseWriter<'a>(pub(super) &'a mut Zeroizing<Vec<u8>>);
impl std::io::Write for PrivateDatabaseWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > (256 * 1024usize).saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("app_login_deployment_invalid"));
        }
        self.0.extend_from_slice(bytes); Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

pub(super) fn read_deployment(path: &Path) -> Result<Deployment,Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let file=std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(path)
        .map_err(|_|Error::new("app_login_deployment_missing"))?;
    if !file.metadata().map_err(|_|Error::new("app_login_deployment_invalid"))?.is_file() { return Err(Error::new("app_login_deployment_invalid")); }
    let mut bytes=Zeroizing::new(Vec::new());
    file.take(256*1024+1).read_to_end(&mut bytes).map_err(|_|Error::new("app_login_deployment_invalid"))?;
    if bytes.len()>256*1024 { return Err(Error::new("app_login_deployment_invalid")); }
    let config:Deployment=serde_json::from_slice(&bytes).map_err(|_|Error::new("app_login_deployment_invalid"))?;
    if config.version!=1 { return Err(Error::new("app_login_deployment_invalid")); }
    config.http.validate()?; Ok(config)
}
