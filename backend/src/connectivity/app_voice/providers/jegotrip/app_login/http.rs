//! Owned, bounded App HTTPS exchange; no redirects, telemetry, system SIM reads
//! or MobileAuth fallback. Transport errors never format credential-bearing URLs.
use super::{Error, LoginRequest, config::HttpDeployment, crypto::{self, JsonDoc, PhoneMaterial}};
use super::super::credentials::Secret;
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use ring::rand::{SecureRandom, SystemRandom};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use url::Url;
use zeroize::Zeroizing;
#[path="communication_http.rs"]
mod communication_ops;

pub(super) struct WireRequest { pub url: Url, pub body: Zeroizing<Vec<u8>>, pub headers: Vec<(String, String)> }
pub(super) struct WireResponse { pub status: u16, pub body: Zeroizing<Vec<u8>> }
pub(super) type Exchange<'a> = Pin<Box<dyn Future<Output=Result<WireResponse,Error>>+Send+'a>>;
pub(super) trait Transport: Send+Sync { fn exchange(&self, request:WireRequest)->Exchange<'_>; }
pub(super) trait TransportFactory: Send+Sync { fn build(&self, config:&HttpDeployment)->Result<AppClient,Error>; }
pub(super) struct LiveFactory;

/// Exact end-entity certificate pin, mirroring the App's bundled `assets/app2.cer`.
/// The operator supplies one concrete server certificate; no system root and no
/// CA path is consulted, and any other certificate fails the handshake. The
/// bundled leaf is X.509 v1, which webpki cannot parse, so the handshake
/// signature is verified with ring against the pinned certificate's own RSA key
/// (also supplied explicitly). That keeps real signature verification instead of
/// accepting any signature.
#[derive(Debug)]
struct PinnedLeaf {
    leaf: Vec<u8>,
    public_key: Vec<u8>,
}
fn rsa_algorithm(scheme: tokio_rustls::rustls::SignatureScheme)
    -> Option<&'static dyn ring::signature::VerificationAlgorithm> {
    use tokio_rustls::rustls::SignatureScheme as S;
    Some(match scheme {
        S::RSA_PSS_SHA256 => &ring::signature::RSA_PSS_2048_8192_SHA256,
        S::RSA_PSS_SHA384 => &ring::signature::RSA_PSS_2048_8192_SHA384,
        S::RSA_PSS_SHA512 => &ring::signature::RSA_PSS_2048_8192_SHA512,
        S::RSA_PKCS1_SHA256 => &ring::signature::RSA_PKCS1_2048_8192_SHA256,
        S::RSA_PKCS1_SHA384 => &ring::signature::RSA_PKCS1_2048_8192_SHA384,
        S::RSA_PKCS1_SHA512 => &ring::signature::RSA_PKCS1_2048_8192_SHA512,
        _ => return None,
    })
}
impl PinnedLeaf {
    fn verify(&self, message: &[u8], dss: &tokio_rustls::rustls::DigitallySignedStruct)
        -> Result<tokio_rustls::rustls::client::danger::HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        let Some(algorithm) = rsa_algorithm(dss.scheme) else {
            return Err(tokio_rustls::rustls::Error::PeerIncompatible(
                tokio_rustls::rustls::PeerIncompatible::NoSignatureSchemesInCommon));
        };
        match ring::signature::UnparsedPublicKey::new(algorithm, &self.public_key).verify(message, dss.signature()) {            Ok(()) => Ok(tokio_rustls::rustls::client::danger::HandshakeSignatureValid::assertion()),
            Err(_) => Err(tokio_rustls::rustls::Error::InvalidCertificate(
                tokio_rustls::rustls::CertificateError::BadSignature)),
        }
    }
}
impl tokio_rustls::rustls::client::danger::ServerCertVerifier for PinnedLeaf {
    fn verify_server_cert(&self, end_entity: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[tokio_rustls::rustls::pki_types::CertificateDer<'_>],
        _server_name: &tokio_rustls::rustls::pki_types::ServerName<'_>, _ocsp: &[u8],
        _now: tokio_rustls::rustls::pki_types::UnixTime)
        -> Result<tokio_rustls::rustls::client::danger::ServerCertVerified, tokio_rustls::rustls::Error> {
        if end_entity.as_ref() != self.leaf.as_slice() {
            eprintln!("app_login_pinned_verify mismatch presented_len={} pinned_len={}", end_entity.as_ref().len(), self.leaf.len());
            return Err(tokio_rustls::rustls::Error::InvalidCertificate(
                tokio_rustls::rustls::CertificateError::UnknownIssuer));
        }
        eprintln!("app_login_pinned_verify matched len={}", self.leaf.len());
        Ok(tokio_rustls::rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, message: &[u8], _cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct)
        -> Result<tokio_rustls::rustls::client::danger::HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        self.verify(message, dss)
    }
    fn verify_tls13_signature(&self, message: &[u8], _cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct)
        -> Result<tokio_rustls::rustls::client::danger::HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        self.verify(message, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        use tokio_rustls::rustls::SignatureScheme as S;
        vec![S::RSA_PSS_SHA256, S::RSA_PSS_SHA384, S::RSA_PSS_SHA512,
             S::RSA_PKCS1_SHA256, S::RSA_PKCS1_SHA384, S::RSA_PKCS1_SHA512]
    }
}
fn transport_failure(error:&reqwest::Error)->Error {
    // Sanitized diagnostics only: the URL/query (n_token/sign) is never printed.
    // Source type names and the rustls variant identify TLS/DNS/connect causes.
    let mut chain:Vec<String>=Vec::new();
    let mut source:Option<&(dyn std::error::Error+'static)>=std::error::Error::source(error);
    let mut depth=0;
    while let Some(item)=source {
        if depth>=8 { break; }
        if item.downcast_ref::<tokio_rustls::rustls::Error>().is_some() { chain.push("rustls".to_owned()); }
        else { chain.push(short_type(item)); }
        source=item.source(); depth+=1;
    }
    eprintln!("app_login_transport_failed connect={} timeout={} request={} body={} chain={chain:?}",
        error.is_connect(),error.is_timeout(),error.is_request(),error.is_body());
    Error::new("app_login_transport_failed")
}
pub(super) fn short_type(value:&(dyn std::error::Error+'static))->String {
    std::any::type_name_of_val(value).rsplit("::").next().unwrap_or("error").to_owned()
}

fn pinned_tls(der: Vec<u8>, public_key: Vec<u8>) -> Result<tokio_rustls::rustls::ClientConfig, Error> {
    eprintln!("app_login_pinned_tls installing pinned_len={} key_len={}", der.len(), public_key.len());
    let config = tokio_rustls::rustls::ClientConfig::builder_with_provider(Arc::new(tokio_rustls::rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions().map_err(|_| Error::new("app_login_deployment_invalid"))?
        .dangerous().with_custom_certificate_verifier(Arc::new(PinnedLeaf { leaf: der, public_key })).with_no_client_auth();
    // reqwest drives HTTP/1.1 here; advertise only what this client speaks.
    let mut config = config;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}
struct LiveTransport(reqwest::Client);
impl TransportFactory for LiveFactory {
    fn build(&self, config:&HttpDeployment)->Result<AppClient,Error> {
        config.validate()?;
        let mut builder=crate::platform::dns::http_client_builder().no_proxy().redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(12)).connect_timeout(Duration::from_secs(8)).connection_verbose(false);
        if let Some(root)=&config.root_der_base64 {
            let bytes=STANDARD.decode(root.expose_to_backend()).map_err(|_|Error::new("app_login_deployment_invalid"))?;
            if bytes.len()>65_536 { return Err(Error::new("app_login_deployment_invalid")); }
            let cert=reqwest::Certificate::from_der(&bytes).map_err(|_|Error::new("app_login_deployment_invalid"))?;
            builder=builder.tls_built_in_root_certs(false).add_root_certificate(cert);
        }
        if let Some(pin)=&config.pinned_server_certificate_der_base64 {
            let der=STANDARD.decode(pin.expose_to_backend()).map_err(|_|Error::new("app_login_deployment_invalid"))?;
            let key=config.pinned_server_public_key_der_base64.as_ref().ok_or_else(||Error::new("app_login_deployment_invalid"))?;
            let key=STANDARD.decode(key.expose_to_backend()).map_err(|_|Error::new("app_login_deployment_invalid"))?;
            builder=builder.tls_built_in_root_certs(false).use_preconfigured_tls(pinned_tls(der,key)?);
        }
        if let Some(agent)=&config.user_agent { builder=builder.user_agent(agent.expose_to_backend()); }
        let client=builder.build().map_err(|_|Error::new("app_login_deployment_invalid"))?;
        AppClient::new(config.clone(),Arc::new(LiveTransport(client)))
    }
}
impl Transport for LiveTransport {
    fn exchange(&self, request:WireRequest)->Exchange<'_> { Box::pin(async move {
        let response=self.0.post(request.url).header(reqwest::header::CONTENT_TYPE,"application/json; charset=utf-8")
            .headers({let mut map=reqwest::header::HeaderMap::new();
                for (name,value) in &request.headers {
                    if let (Ok(name),Ok(value))=(reqwest::header::HeaderName::from_bytes(name.as_bytes()),reqwest::header::HeaderValue::from_str(value)){map.insert(name,value);}
                }map})
            .body(request.body.to_vec()).send().await.map_err(|error| transport_failure(&error))?;
        let status=response.status().as_u16();
        if response.content_length().is_some_and(|n|n>crypto::MAX_BODY as u64) { return Err(Error::new("app_login_response_invalid")); }
        let mut stream=response.bytes_stream();let mut bytes=Zeroizing::new(Vec::new());
        while let Some(chunk)=stream.next().await {
            let chunk=chunk.map_err(|_|Error::new("app_login_transport_failed"))?;
            if chunk.len()>crypto::MAX_BODY.saturating_sub(bytes.len()) { return Err(Error::new("app_login_response_invalid")); }
            bytes.extend_from_slice(&chunk);
        }
        Ok(WireResponse {status,body:bytes})
    }) }
}
pub(super) fn rejected_response_diagnostic(path:&str,doc:&JsonDoc)->Option<String> {
    let code=crypto::outer_code(doc)?;
    (code!=0).then(||format!("app_login_reject_fields endpoint={path} code={code}"))
}
fn bounded_code(value:&serde_json::Value)->Option<i64> {
    let code=if let Some(number)=value.as_i64() {number} else {
        let text=value.as_str()?;
        if text.is_empty() || text.len()>6 || !text.bytes().all(|b|b.is_ascii_digit()) {return None;}
        let number:i64=text.parse().ok()?;
        // 原 App 对 optString 后的 "1" 做精确比较；"01" 不能变成授信成功。
        if number.to_string()!=text {return None;}
        number
    };
    (0..=999999).contains(&code).then_some(code)
}
pub(super) fn trust_code(body:&serde_json::Map<String,serde_json::Value>)->Option<i64> {
    bounded_code(body.get("authentication_code")?)
}
pub(super) fn device_trust_shape(label:&str,body:&serde_json::Map<String,serde_json::Value>)->String {
    let mut fields=Vec::new();
    for key in ["enrollId","expireTime","authentication_code"] {
        let Some(value)=body.get(key) else {continue;};
        let shape=if key=="authentication_code" {
            trust_code(body).map(|n|format!("num:{n}")).unwrap_or_else(||"unknown".into())
        } else if let Some(text)=value.as_str() {format!("str:{}",text.len())} else {"other".into()};
        fields.push(format!("{key}={shape}"));
    }
    format!("app_login_device_trust_shape step={label} {}",fields.join(" "))
}
#[derive(Clone, Copy)]
enum Authentication { Anonymous, Session }
pub(super) struct BoundNumber { country:String, mobile:Secret, app_account_id:Option<Secret> }
/// Finite fields of the SAME getCallStatus response, not SDK readiness.
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) struct CallStatus { pub authentication_code:Option<i64>, pub hosting_enabled:bool, pub local_cs_forward:Option<i64> }
pub(super) fn decode_call_status(body:&serde_json::Map<String,serde_json::Value>)->Result<CallStatus,Error> {
    let value=body.get("cs_forward").ok_or(Error::new("app_login_response_invalid"))?;
    let enabled=match value.as_str() {
        Some("2")=>true,Some("1")=>false,_=>match value.as_u64() {Some(2)=>true,Some(1)=>false,_=>return Err(Error::new("app_login_response_invalid"))},
    };
    let local=body.get("localCsForward").and_then(bounded_code);
    Ok(CallStatus {authentication_code:trust_code(body),hosting_enabled:enabled,local_cs_forward:local})
}
/// Server answer of the App's trusted-device verification. Nothing here is a
/// grant: `authentication_code` is only what the server reported.
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) struct TrustDecision { pub authentication_code:Option<i64> }
impl BoundNumber {
    pub fn identity(&self)->Zeroizing<String> { Zeroizing::new(format!("+{}{}",self.country,self.mobile.expose_to_backend())) }
    pub fn masked(&self)->String { let s=self.mobile.expose_to_backend();format!("***{}",&s[s.len().saturating_sub(4)..]) }
}
pub(super) struct AppClient { config:HttpDeployment, transport:Arc<dyn Transport>, session:Option<Secret>,
    version_store:Option<Arc<dyn super::app_version::VersionStore>>,
    /// `enrollId` returned by the App's own `device/sec/v1/enroll`. The original
    /// LoginActivity enrols before signing in and then sends this value in the
    /// login body, so it lives with the client for this flow only.
    enroll_id:std::sync::Mutex<Option<Secret>>,
    /// Per-account App device identity resolved from the installation store.
    /// SDK identity remains a separate explicit runtime parameter.
    device_id_override:Option<Secret>,
    /// Reuse is an operation policy, never a locally granted trust bit.
    /// Separate local SDK preparation policy. This flag is never a grant and
    /// cannot be supplied by a response or persisted diagnostic observation.
    local_receiver:bool,
    communication:Option<super::communication::CommunicationOperation>,
    /// Actions-only deterministic public-version fixture, never App/SDK I/O.
    #[cfg(test)]
    pub(super) version_lookup:Option<Arc<dyn Fn(&str)->Option<Vec<u8>>+Send+Sync>>,
    trusted_reuse:bool,
    trusted_reuse_write_started:std::sync::atomic::AtomicBool,
    version_cache:std::sync::Mutex<Option<(String,&'static str,std::time::Instant)>> }
/// The server's explicit "your App is too old" business code. It is the ONLY
/// trigger for re-learning the version and replaying a request.
pub(super) const VERSION_GATE_CODE:u32=524;
/// Parse the iOS lookup payload (`{"resultCount":1,"results":[{"version":"8.9.8"}]}`).
/// Only a validated `8.x[.y[.z]]` literal is accepted; nothing is inferred.
pub(super) fn pick_ios_version(text:&str)->Option<String> {
    let value:serde_json::Value=serde_json::from_str(text).ok()?;
    let version=value.get("results")?.as_array()?.first()?.get("version")?.as_str()?.trim();
    (super::config::valid_app_version(version) && version.split('.').next()==Some("8")).then(||version.to_owned())
}
/// Pick the newest `8.x.y[.z]` literal out of an official page. Pure and
/// deterministic so a page redesign degrades to "no value" instead of a guess.
pub(super) fn pick_version(text:&str)->Option<String> {
    let bytes=text.as_bytes();
    let mut best:Option<(Vec<u32>,String)>=None;
    let mut index=0usize;
    while index<bytes.len() {
        if !bytes[index].is_ascii_digit() { index+=1; continue; }
        let start=index;
        while index<bytes.len() && (bytes[index].is_ascii_digit() || bytes[index]==b'.') { index+=1; }
        let candidate=text[start..index].trim_end_matches('.');
        if super::config::valid_app_version(candidate) {
            let parts:Vec<u32>=candidate.split('.').filter_map(|part|part.parse().ok()).collect();
            if parts.len()>=3 && parts[0]==8 && parts.iter().all(|part|*part<10_000) {
                let newer=best.as_ref().map_or(true,|(key,_)|parts> *key);
                if newer { best=Some((parts,candidate.to_owned())); }
            }
        }
    }
    best.map(|(_,value)|value)
}
impl AppClient {
    /// Private stable identity key for this App realm and the server-returned
    /// bound number. Neither this key nor the full number is a public DTO.
    pub fn account_key(&self,bound:&BoundNumber)->Result<Vec<u8>,Error> {
        let realm=self.config.endpoint(&self.config.endpoints.login)?;
        let identity=bound.identity();
        let mut hash=ring::digest::Context::new(&ring::digest::SHA256);
        hash.update(realm.as_str().as_bytes());hash.update(&[0]);hash.update(identity.as_bytes());
        Ok(hash.finish().as_ref().to_vec())
    }
    /// Separate realm-scoped lookup key for an unconfirmed login alias. It can
    /// become a number binding only after authenticated user-info confirms it.
    pub fn login_alias_key(&self,request:&LoginRequest)->Result<Vec<u8>,Error> {
        let realm=self.config.endpoint(&self.config.endpoints.login)?;
        let mut hash=ring::digest::Context::new(&ring::digest::SHA256);
        hash.update(b"jegotrip-device-alias-v1\0");hash.update(realm.as_str().as_bytes());
        hash.update(&[0]);hash.update(&request.identity_key());
        Ok(hash.finish().as_ref().to_vec())
    }
    pub fn new(config:HttpDeployment,transport:Arc<dyn Transport>)->Result<Self,Error> { config.validate()?;Ok(Self {config,transport,session:None,version_store:None,enroll_id:std::sync::Mutex::new(None),device_id_override:None,local_receiver:false,communication:None,trusted_reuse:false,trusted_reuse_write_started:std::sync::atomic::AtomicBool::new(false),version_cache:std::sync::Mutex::new(None),#[cfg(test)] version_lookup:None}) }
    pub(super) fn with_trusted_reuse(mut self)->Result<Self,Error> {
        // A private extra field must not sneak a stale enrolment, carrier
        // credential or a different report owner into this explicit policy.
        if self.config.extra_body.iter().any(|pair|matches!(pair.name.expose_to_backend(),
            "enrollId"|"sso_token"|"device_id"|"deviceId"|"imei"|"userId"|"accountid"|"authentication_code")) {
            return Err(Error::new("app_login_deployment_invalid"));
        }
        self.trusted_reuse=true;Ok(self)
    }
    pub(super) fn with_local_receiver(self)->Result<Self,Error> {
        if self.local_receiver || self.trusted_reuse || self.session.is_some() {return Err(Error::new("app_login_request_invalid"));}
        if self.config.endpoints.verification_trustful_device.is_none() {return Err(Error::new("app_login_receiver_device_check_missing"));}
        // Reuse the immutable original-UUID/no-enrolment HTTP policy, not its
        // remote call-forwarding workflow. Only this mode may perform the
        // current empty-carrier-token existing-device check below.
        let mut client=self.with_trusted_reuse()?;client.local_receiver=true;Ok(client)
    }
    pub(super) fn is_local_receiver(&self)->bool {self.local_receiver}
    pub(super) fn trusted_reuse_write_started(&self)->bool { self.trusted_reuse_write_started.load(std::sync::atomic::Ordering::SeqCst) }
    /// Bind a checked installation-store identity before any network request.
    pub fn with_device_id(mut self,device_id:&str)->Self { self.use_device_identity(device_id); self }
    pub fn use_device_identity(&mut self,device_id:&str) {
        self.session=None;
        if let Ok(mut slot)=self.enroll_id.lock() { *slot=None; }
        self.device_id_override=Some(Secret::from_owned(device_id.to_owned()));
    }
    fn device_id(&self)->Result<&str,Error> {
        let value=self.device_id_override.as_ref().unwrap_or(&self.config.device_id).expose_to_backend();
        if !super::device_identity::valid_device_id(value) { return Err(Error::new("app_login_deployment_invalid")); }
        Ok(value)
    }
    /// Attach the durable shared-state store. Without it the version still
    /// resolves from file/page/static, but nothing is persisted.
    pub fn with_version_store(mut self,store:Arc<dyn super::app_version::VersionStore>)->Self { self.version_store=Some(store); self }
    fn store_load(&self)->Option<super::app_version::StoredVersion> { self.version_store.as_ref().and_then(|store|store.load()) }
    fn store_save(&self,version:&str,source:&str) {
        if !self.config.app_version_store { return; }
        let Some(store)=&self.version_store else { return; };
        let now=chrono::Utc::now().timestamp().max(0);
        store.save(&super::app_version::StoredVersion {version:version.to_owned(),source:source.to_owned(),observed_at:now});
    }
    /// Resolve `$APP_VERSION` for the CURRENT request. Order: explicit file
    /// (re-read every call), shared stored value, iOS App Store, official page,
    /// static configuration. A stored value older than the store TTL is
    /// re-checked but still used when the re-check fails, so an official App
    /// upgrade needs no SimAdmin rebuild and an offline start still works.
    async fn app_version(&self)->Option<(String,&'static str)> {
        use super::app_version::{SOURCE_FILE,SOURCE_STATIC,SOURCE_STORE,STORE_TTL_SECONDS};
        if let Some(path)=&self.config.app_version_file {
            if let Ok(bytes)=std::fs::read(path.expose_to_backend()) {
                if bytes.len()<=64 {
                    let value=String::from_utf8(bytes).ok().map(|text|text.trim().to_owned());
                    if let Some(value)=value.filter(|v|super::config::valid_app_version(v)) { return Some((value,SOURCE_FILE)); }
                }
            }
            return None;
        }
        let cached=self.version_cache.lock().ok().and_then(|slot|slot.clone());
        if let Some((value,source,at))=cached {
            if at.elapsed()<std::time::Duration::from_secs(super::app_version::MEMORY_TTL_SECONDS) { return Some((value,source)); }
        }
        if let Some(stored)=self.store_load() {
            let age=chrono::Utc::now().timestamp().max(0)-stored.observed_at;
            if age<STORE_TTL_SECONDS { self.remember_version(&stored.version,SOURCE_STORE); return Some((stored.version,SOURCE_STORE)); }
            if let Some((value,source))=self.discover_remote().await {
                self.remember_version(&value,source); return Some((value,source));
            }
            eprintln!("app_login_version_keeping_stale age={age}");
            self.remember_version(&stored.version,SOURCE_STORE); return Some((stored.version,SOURCE_STORE));
        }
        if let Some((value,source))=self.discover_remote().await { self.remember_version(&value,source); return Some((value,source)); }
        let value=self.config.app_version.as_ref()?.expose_to_backend().to_owned();
        super::config::valid_app_version(&value).then(||(value,SOURCE_STATIC))
    }
    /// Force a fresh remote lookup, bypassing both caches. Used before the first
    /// hosted login (so a brand-new installation starts with a stored value) and
    /// when the server answers the version gate.
    pub(super) async fn refresh_app_version(&self)->Option<(String,&'static str)> {
        let found=self.discover_remote().await;
        match found { Some((value,source))=>{ self.remember_version(&value,source); Some((value,source)) }, None=>None }
    }
    /// First-run pre-flight: a brand-new installation has no stored value yet.
    /// Learning it here means the login itself already carries a real version.
    pub(super) async fn ensure_app_version(&self) {
        if self.config.app_version_file.is_some() || self.config.app_version.is_some() { return; }
        if !self.config.app_version_store && self.config.app_version_ios_app_id.is_none() && self.config.app_version_web_url.is_none() { return; }
        if self.store_load().is_some() { return; }
        let (value,source)=self.app_version().await.unwrap_or_else(||(String::new(),"none"));
        eprintln!("app_login_version_preflight len={} source={source}",value.len());
    }
    /// iOS App Store first (a stable, machine-readable literal), then the
    /// official page. Only a validated literal is persisted; no guess, and a
    /// failure keeps whatever the store already had.
    async fn discover_remote(&self)->Option<(String,&'static str)> {
        use super::app_version::{SOURCE_IOS,SOURCE_PAGE};
        if let Some(id)=&self.config.app_version_ios_app_id {
            let url=format!("https://itunes.apple.com/lookup?id={}&country=cn",id.expose_to_backend());
            if let Some(bytes)=self.fetch_text(&url).await {
                match pick_ios_version(&String::from_utf8_lossy(&bytes)) {
                    Some(value)=>{ self.store_save(&value,SOURCE_IOS); return Some((value,SOURCE_IOS)); },
                    None=>eprintln!("app_login_version_discovery_failed stage=ios_parse bytes={}",bytes.len()),
                }
            }
        }
        if let Some(url)=&self.config.app_version_web_url {
            if let Some(bytes)=self.fetch_text(url.expose_to_backend()).await {
                match pick_version(&String::from_utf8_lossy(&bytes)) {
                    Some(value)=>{ self.store_save(&value,SOURCE_PAGE); return Some((value,SOURCE_PAGE)); },
                    None=>eprintln!("app_login_version_discovery_failed stage=page_parse bytes={}",bytes.len()),
                }
            }
        }
        None
    }
    fn remember_version(&self,value:&str,source:&'static str) {
        if let Ok(mut slot)=self.version_cache.lock() { *slot=Some((value.to_owned(),source,std::time::Instant::now())); }
    }
    /// Value-only view used by the per-call placeholders.
    pub(super) async fn version_value(&self)->Option<String> { self.app_version().await.map(|(value,_)|value) }
    /// Enrolment identifier learned from `device/sec/v1/enroll` in this flow.
    fn remembered_enroll_id(&self)->Option<String> {
        self.enroll_id.lock().ok().and_then(|slot|slot.clone()).map(|value|value.expose_to_backend().to_owned())
    }
    fn remember_enroll_id(&self,value:&str) {
        if let Ok(mut slot)=self.enroll_id.lock() { *slot=Some(Secret::from_owned(value.to_owned())); }
    }
    /// A replay of the same login attempt must not reuse a previous App session
    /// token; each flow starts from the anonymous login again.
    pub(super) fn reset_session(&mut self) { self.session=None; }
    /// Provenance of the value that will be placed in the next request, for the
    /// bounded operator log line (never a secret).
    pub(super) async fn version_provenance(&self)->String {
        match self.app_version().await { Some((value,source))=>format!("{source}:{value}"), None=>"none".to_owned() }
    }
    /// Fetch a discovery URL with public roots (not the pinned App leaf),
    /// bounded body/time, no redirects; only the version literal is kept.
    async fn fetch_text(&self,url:&str)->Option<Vec<u8>> {
        #[cfg(test)]
        if let Some(lookup)=&self.version_lookup{return lookup(url).filter(|bytes|bytes.len()<=524_288);}
        // Shared factory keeps the project's DNS boundary (no ad-hoc clients).
        let builder=crate::platform::dns::http_client_builder()
            .timeout(std::time::Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none());
        let client=match builder.build() { Ok(client)=>client, Err(error)=>{ eprintln!("app_login_version_discovery_failed stage=client reason={}",short_type(&error)); return None; } };
        let response=match client.get(url).send().await {
            Ok(response)=>match response.error_for_status() {
                Ok(response)=>response,
                Err(error)=>{ eprintln!("app_login_version_discovery_failed stage=status code={:?}",error.status()); return None; }
            },
            Err(error)=>{ eprintln!("app_login_version_discovery_failed stage=request connect={} timeout={} reason={}",
                error.is_connect(),error.is_timeout(),short_type(&error)); return None; }
        };
        if response.content_length().unwrap_or(0)>524_288 { eprintln!("app_login_version_discovery_failed stage=size"); return None; }
        let bytes=match response.bytes().await { Ok(bytes)=>bytes, Err(_)=>{ eprintln!("app_login_version_discovery_failed stage=body"); return None; } };
        if bytes.len()>524_288 { eprintln!("app_login_version_discovery_failed stage=size"); return None; }
        Some(bytes.to_vec())
    }
    fn expand(value:&str,version:Option<&str>)->Option<String> {
        if value=="$APP_VERSION" { return version.map(str::to_owned); }
        Some(value.replace("$APP_VERSION",version.unwrap_or_default()))
    }
    /// Every App JSON body passes through here so the installation's explicit
    /// extra fields (for example a client-version gate) are merged once, before
    /// encryption. Extras never overwrite fields the caller produced.
    fn payload(&self,value:serde_json::Value,version:Option<&str>)->JsonDoc {
        let mut value=value;
        if let Some(map)=value.as_object_mut() {
            for pair in &self.config.extra_body {
                let key=pair.name.expose_to_backend();
                if map.contains_key(key) { continue; }
                if let Some(expanded)=Self::expand(pair.value.expose_to_backend(),version.as_deref()) {
                    map.insert(key.to_owned(),serde_json::Value::String(expanded));
                }
            }
        }
        JsonDoc(value)
    }
    fn nonce(&self)->Result<String,Error> {
        let mut byte=[0];
        for _ in 0..16 {
            SystemRandom::new().fill(&mut byte).map_err(|_|Error::new("app_login_entropy_failed"))?;
            if byte[0]<200 { return Ok(format!("{}{}",chrono::Utc::now().timestamp_millis().max(0),100+u16::from(byte[0])%100)); }
        }
        Err(Error::new("app_login_entropy_failed"))
    }
    async fn post(&self,path:&str,content:&[u8],auth:Authentication,encrypted:bool)->Result<JsonDoc,Error> {
        let protection=if encrypted {self.config.body_protection.as_ref()} else {None};
        let request_body=crypto::request_body(content,protection,&self.nonce()?)?;
        let mut url=self.config.endpoint(path)?;
        let millis=chrono::Utc::now().timestamp_millis().max(0) as u64;
        let signature=crypto::url_signature(&self.config.signature_salt,millis);
        let version=self.version_value().await;
        {
            let mut query=url.query_pairs_mut();
            match auth {
                Authentication::Anonymous=>{query.append_pair("n_token",self.config.anonymous_token.expose_to_backend()).append_pair("lang","zh_CN");},
                Authentication::Session=>{
                    let token=self.session.as_ref().ok_or(Error::new("app_login_unavailable"))?;
                    query.append_pair("lang","zh_CN").append_pair("token",token.expose_to_backend());
                },
            }
            query.append_pair("timestamp",&millis.to_string()).append_pair("sign",&signature);
            for pair in &self.config.extra_query {
                if let Some(expanded)=Self::expand(pair.value.expose_to_backend(),version.as_deref()) {
                    query.append_pair(pair.name.expose_to_backend(),&expanded);
                }
            }
        }
        let headers={self.config.extra_headers.iter()
            .filter_map(|pair|Self::expand(pair.value.expose_to_backend(),version.as_deref())
                .map(|value|(pair.name.expose_to_backend().to_owned(),value))).collect()};
        let response=self.transport.exchange(WireRequest {url,body:request_body,headers}).await?;
        if response.status==429 { return Err(Error::new("app_login_rate_limited")); }
        if response.status!=200 { eprintln!("app_login_http_status endpoint={path} status={} bytes={}",response.status,response.body.len()); return Err(Error::new("app_login_transport_failed")); }
        let mut doc=match crypto::parse(&response.body) {
            Ok(doc)=>doc,
            Err(error)=>{ eprintln!("app_login_body_invalid endpoint={path} bytes={} reason={}",response.body.len(),error.code); return Err(error); }
        };
        // Even a rejected server response can echo request secrets. Log only
        // its canonical business code, never arbitrary fields/message/body.
        if let Some(line)=rejected_response_diagnostic(path,&doc) {eprintln!("{line}");}
        // Outer App codes are inspected independently from body decryption. In
        // particular 2006 must NOT trigger the APK's implicit closeVoipForward.
        crypto::success(&doc)?; crypto::decrypt_body(&mut doc,protection)?; Ok(doc)
    }
    pub async fn login(&mut self,request:&LoginRequest)->Result<(),Error> {
        let hash=crypto::password_hash(&self.config.password_salt,&request.password);
        let mut payload=self.payload(serde_json::json!({"loginName":request.username.expose_to_backend().trim(),
            "password":hash.expose_to_backend(),"userType":"0","site":1}),self.version_value().await.as_deref());
        if let Some(country)=&request.country_code { payload.0["countryCode"]=serde_json::Value::String(country.clone()); }
        // Reuse never applies a static/old enrolment to the copied phone UUID.
        if !self.trusted_reuse {
            if let Some(id)=&self.config.enroll_id { payload.0["enrollId"]=serde_json::Value::String(id.expose_to_backend().to_owned()); }
            // Ordinary LoginActivity compatibility retains its fresh enrolment.
            if let Some(id)=self.remembered_enroll_id() { payload.0["enrollId"]=serde_json::Value::String(id); }
        }
        // No campaign context is invented; null Gson fields are omitted.
        let bytes=Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?);
        let doc=self.post(&self.config.endpoints.login,&bytes,Authentication::Anonymous,true).await?;
        self.session=Some(crypto::secret_field(crypto::body(&doc)?,"token")?);
        // App expiredAt is not a proven SDK-token expiry; only this bounded
        // attempt retains the App session. No permanent/renewed grant is inferred.
        Ok(())
    }
    pub async fn bound_number(&self,request:&LoginRequest)->Result<BoundNumber,Error> {
        let doc=self.post(&self.config.endpoints.user_info,b"",Authentication::Session,true).await?;
        let body=crypto::body(&doc)?;
        let mobile=body.get("mobile").and_then(|v|v.as_str()).ok_or(Error::new("app_login_phone_binding_required"))?;
        let country=body.get("country_code").and_then(|v|v.as_str()).ok_or(Error::new("app_login_phone_binding_required"))?.trim_start_matches('+');
        if mobile.is_empty() || mobile.len()>20 || !mobile.bytes().all(|b|b.is_ascii_digit())
            || country.is_empty() || country.len()>4 || !country.bytes().all(|b|b.is_ascii_digit()) { return Err(Error::new("app_login_phone_binding_required")); }
        if let Some(requested_country)=&request.country_code {
            if country!=requested_country || mobile!=request.username.expose_to_backend().trim() { return Err(Error::new("app_login_identity_mismatch")); }
        }
        // Original SysApplication.getUserId reads User.accountid. No alias to
        // user_id/phone number/SDK identity is inferred for the device report.
        let app_account_id=body.get("accountid").and_then(|v|v.as_str()).filter(|s|
            !s.is_empty() && s.len()<=254 && !s.chars().any(char::is_control)).map(|s|Secret::from_owned(s.to_owned()));
        Ok(BoundNumber {country:country.to_owned(),mobile:Secret::from_owned(mobile.to_owned()),app_account_id})
    }
    pub async fn phone_credentials(&self,bound:&BoundNumber)->Result<PhoneMaterial,Error> {
        if self.local_receiver {return Err(Error::new("app_login_request_invalid"));}
        self.fetch_phone_material(bound).await
    }
    pub(super) async fn receiver_phone_credentials(&self,bound:&BoundNumber)->Result<PhoneMaterial,Error> {
        if !self.local_receiver {return Err(Error::new("app_login_request_invalid"));}
        self.fetch_phone_material(bound).await
    }
    async fn fetch_phone_material(&self,bound:&BoundNumber)->Result<PhoneMaterial,Error> {
        let identity=bound.identity();let payload=self.payload(serde_json::json!({"accountId":&**identity}),self.version_value().await.as_deref());
        let bytes=Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?);
        let token_doc=self.post(&self.config.endpoints.phone_token,&bytes,Authentication::Session,true).await?;
        let token=crypto::secret_field(crypto::body(&token_doc)?,"token")?;
        let sip_doc=self.post(&self.config.endpoints.voip_status,b"",Authentication::Session,false).await?;
        let body=crypto::body(&sip_doc)?;
        // JustalkManager.login/updateStatus use the actual phone/SIP material.
        // VoipStatus.status in one UI callback is not a general SDK grant or
        // prerequisite; empty/malformed material and HTTP errors still fail.
        let encrypted=crypto::secret_field(body,"sip_password")?;
        let sip_password=crypto::sip_password(&encrypted,&self.config)?;
        Ok(PhoneMaterial {token,sip_password})
    }
    /// Original receiveFail fallback: query whether the server already trusts
    /// THIS device, with an empty MobileAuth token. The App login session is
    /// only the HTTPS authentication query; it is never a MobileAuth credential.
    /// Original CallLogic uses ordinary JSON here, and equipment_type is "1".
    pub async fn verify_trusted_device(&self)->Result<Option<TrustDecision>,Error> {
        if self.trusted_reuse { return Err(Error::new("app_login_request_invalid")); }
        self.query_existing_device().await
    }
    pub(super) async fn receiver_device_recognition(&self)->Result<Option<TrustDecision>,Error> {
        if !self.local_receiver {return Err(Error::new("app_login_request_invalid"));}
        self.query_existing_device().await
    }
    async fn query_existing_device(&self)->Result<Option<TrustDecision>,Error> {
        let Some(path)=self.config.endpoints.verification_trustful_device.clone() else { return Ok(None); };
        if self.session.is_none() {return Err(Error::new("app_login_unavailable"));}
        let payload=self.payload(serde_json::json!({
            "sso_token":"",
            "equipment_type":"1",
            "device_id":self.device_id()?}),self.version_value().await.as_deref());
        let bytes=Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?);
        let doc=self.post(&path,&bytes,Authentication::Session,false).await?;
        let body=crypto::body(&doc)?;
        // Only a small public code is reported; reflected numeric identifiers
        // or arbitrary strings stay unknown, never a fabricated grant.
        Ok(Some(TrustDecision { authentication_code:trust_code(body) }))
    }
    /// Security-device enrolment (`CmiLogic.enroll`): the App posts exactly
    /// `{"deviceId": …}`. Optional; a missing path sends nothing.
    pub async fn enroll_device(&self)->Result<Option<TrustDecision>,Error> {
        if self.trusted_reuse { return Err(Error::new("app_login_request_invalid")); }
        let path=self.config.endpoints.device_enroll.clone();
        let Some(path)=path else { return Ok(None); };
        let payload=self.payload(serde_json::json!({"deviceId":self.device_id()?}),self.version_value().await.as_deref());
        let bytes=Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?);
        let doc=self.post(&path,&bytes,Authentication::Anonymous,true).await?;
        let body=crypto::body(&doc)?;
        Self::log_body_shape("enroll",body);
        // The App stores the returned enrolment id and sends it in the next
        // login body; only a bounded, control-free value is kept.
        if let Some(serde_json::Value::String(value))=body.get("enrollId") {
            let trimmed=value.trim();
            if !trimmed.is_empty() && trimmed.len()<=256 && !trimmed.chars().any(char::is_control) {
                self.remember_enroll_id(trimmed);
                eprintln!("app_login_device_enroll id_len={}",trimmed.len());
            }
        }
        Ok(Some(TrustDecision { authentication_code:trust_code(body) }))
    }
    /// The original LoginActivity enrols first, so the login body carries the
    /// fresh `enrollId`. Best-effort: a failed enrolment never blocks the login,
    /// it just leaves the configured/static value in place.
    pub(super) async fn ensure_device_enrollment(&self) {
        if self.trusted_reuse || self.config.endpoints.device_enroll.is_none() || self.remembered_enroll_id().is_some() { return; }
        match self.enroll_device().await {
            Ok(Some(_))=>eprintln!("app_login_device_enroll ok={}",self.remembered_enroll_id().is_some()),
            Ok(None)=>eprintln!("app_login_device_enroll not_configured"),
            Err(error)=>eprintln!("app_login_device_enroll_failed code={} server_code={:?}",error.code,error.server_code),
        }
    }
    /// Device security verification (`CmiLogic.sendSecurityVerify`): same
    /// `{"deviceId": …}` body. Optional; never inferred from a previous call.
    pub async fn security_verify_device(&self)->Result<Option<TrustDecision>,Error> {
        self.device_only_call("security_verify",self.config.endpoints.security_verify.clone()).await
    }
    /// Both App calls above send one field only; sharing the body keeps them
    /// provably identical to the reviewed DEX shape. Only field NAMES (and
    /// value lengths) of the answer are logged: the operator learns the shape
    /// without any token or device material reaching the journal.
    async fn device_only_call(&self,label:&str,path:Option<String>)->Result<Option<TrustDecision>,Error> {
        let Some(path)=path else { return Ok(None); };
        let payload=self.payload(serde_json::json!({
            "deviceId":self.device_id()?}),self.version_value().await.as_deref());
        let bytes=Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?);
        let doc=self.post(&path,&bytes,Authentication::Session,true).await?;
        let body=crypto::body(&doc)?;
        Self::log_body_shape(label,body);
        Ok(Some(TrustDecision { authentication_code:trust_code(body) }))
    }
    fn log_body_shape(label:&str,body:&serde_json::Map<String,serde_json::Value>) {
        eprintln!("{}",device_trust_shape(label,body));
    }
    pub async fn call_status(&self)->Result<CallStatus,Error> {
        let payload=self.payload(serde_json::json!({"device_id":self.device_id()?}),self.version_value().await.as_deref());
        let bytes=Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?);
        let doc=self.post(&self.config.endpoints.call_status,&bytes,Authentication::Session,false).await?;
        decode_call_status(crypto::body(&doc)?)
    }
    pub async fn business_enabled(&self)->Result<bool,Error> { Ok(self.call_status().await?.hosting_enabled) }
    pub(super) fn device_sync_configured(&self)->bool { self.config.device_sync_profile.is_some() }
    pub(super) fn validate_device_sync_context(&self,bound:&BoundNumber)->Result<(),Error> {
        let Some(profile)=&self.config.device_sync_profile else { return Ok(()); };
        let account=bound.app_account_id.as_ref().ok_or(Error::new("app_login_device_sync_context_missing"))?;
        let _=profile.payload(self.device_id()?,account.expose_to_backend())?;Ok(())
    }
    pub(super) async fn sync_device(&self,bound:&BoundNumber,status:CallStatus)->Result<(),Error> {
        if self.local_receiver {return Err(Error::new("app_login_request_invalid"));}
        if !self.trusted_reuse || status.authentication_code!=Some(1) { return Err(Error::new("app_login_device_untrusted")); }
        let profile=self.config.device_sync_profile.as_ref().ok_or(Error::new("app_login_device_sync_context_missing"))?;
        let account=bound.app_account_id.as_ref().ok_or(Error::new("app_login_device_sync_context_missing"))?;
        let path=self.config.endpoints.device_sync.as_deref().ok_or(Error::new("app_login_device_sync_context_missing"))?;
        let payload=profile.payload(self.device_id()?,account.expose_to_backend())?;
        let payload=self.payload(payload.0.clone(),self.version_value().await.as_deref());
        let bytes=Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?);
        // The ledger must be uncertain BEFORE this method. A later version
        // gate cannot replay a device report that may already have been applied.
        self.trusted_reuse_write_started.store(true,std::sync::atomic::Ordering::SeqCst);
        self.post(path,&bytes,Authentication::Session,true).await?;Ok(())
    }
    pub async fn enable_business(&self)->Result<(),Error> {
        if self.local_receiver {return Err(Error::new("app_login_request_invalid"));}
        let payload=self.payload(serde_json::json!({"cs_forward":"2","device_id":self.device_id()?}),self.version_value().await.as_deref());
        let bytes=Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?);
        if self.trusted_reuse { self.trusted_reuse_write_started.store(true,std::sync::atomic::Ordering::SeqCst); }
        self.post(&self.config.endpoints.enable_hosting,&bytes,Authentication::Session,true).await?;Ok(())
    }
}
