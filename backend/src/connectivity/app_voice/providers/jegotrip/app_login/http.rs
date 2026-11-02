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
        if let Some(tls)=item.downcast_ref::<tokio_rustls::rustls::Error>() { chain.push(format!("rustls:{tls:?}")); }
        else { chain.push(format!("{}: {}", short_type(item), item)); }
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
#[derive(Clone, Copy)]
enum Authentication { Anonymous, Session }
pub(super) struct BoundNumber { country:String, mobile:Secret }
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
    pub fn new(config:HttpDeployment,transport:Arc<dyn Transport>)->Result<Self,Error> { config.validate()?;Ok(Self {config,transport,session:None,version_store:None,version_cache:std::sync::Mutex::new(None)}) }
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
        // Bounded, sanitized field dump for rejected business calls: the operator
        // needs the server's own reason fields (message/msg/detail) to calibrate
        // deployment values, and they never contain our credentials.
        if let Some(code)=crypto::outer_code(&doc) {
            if code!=0 {
                let mut fields=Vec::new();
                if let Some(map)=doc.0.as_object() {
                    for (key,value) in map.iter().take(12) {
                        let text=match value {
                            serde_json::Value::String(text)=>text.chars().filter(|c|!c.is_control()).take(120).collect::<String>(),
                            other=>other.to_string().chars().take(60).collect(),
                        };
                        fields.push(format!("{key}={text:?}"));
                    }
                }
                eprintln!("app_login_reject_fields endpoint={path} code={code} {}",fields.join(" "));
            }
        }
        // Outer App codes are inspected independently from body decryption. In
        // particular 2006 must NOT trigger the APK's implicit closeVoipForward.
        crypto::success(&doc)?; crypto::decrypt_body(&mut doc,protection)?; Ok(doc)
    }
    pub async fn login(&mut self,request:&LoginRequest)->Result<(),Error> {
        let hash=crypto::password_hash(&self.config.password_salt,&request.password);
        let mut payload=self.payload(serde_json::json!({"loginName":request.username.expose_to_backend().trim(),
            "password":hash.expose_to_backend(),"userType":"0","site":1}),self.version_value().await.as_deref());
        if let Some(country)=&request.country_code { payload.0["countryCode"]=serde_json::Value::String(country.clone()); }
        if let Some(id)=&self.config.enroll_id { payload.0["enrollId"]=serde_json::Value::String(id.expose_to_backend().to_owned()); }
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
        Ok(BoundNumber {country:country.to_owned(),mobile:Secret::from_owned(mobile.to_owned())})
    }
    pub async fn phone_credentials(&self,bound:&BoundNumber)->Result<PhoneMaterial,Error> {
        let identity=bound.identity();let payload=self.payload(serde_json::json!({"accountId":&**identity}),self.version_value().await.as_deref());
        let bytes=Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?);
        let token_doc=self.post(&self.config.endpoints.phone_token,&bytes,Authentication::Session,true).await?;
        let token=crypto::secret_field(crypto::body(&token_doc)?,"token")?;
        let sip_doc=self.post(&self.config.endpoints.voip_status,b"",Authentication::Session,false).await?;
        let encrypted=crypto::secret_field(crypto::body(&sip_doc)?,"sip_password")?;
        let sip_password=crypto::sip_password(&encrypted,&self.config)?;
        Ok(PhoneMaterial {token,sip_password})
    }
    /// The original App verifies that THIS device is already trusted before it
    /// touches the business switch (`CallLogic.querySafeDevice` sends exactly
    /// `sso_token`, `equipment_type=1` and `device_id`). `Ok(None)` means the
    /// installation did not configure the optional endpoint; the caller then
    /// keeps its previous behaviour instead of inventing a trust result.
    pub async fn verify_trusted_device(&self)->Result<Option<TrustDecision>,Error> {
        let Some(path)=self.config.endpoints.verification_trustful_device.clone() else { return Ok(None); };
        let token=self.session.as_ref().ok_or(Error::new("app_login_unavailable"))?;
        let payload=self.payload(serde_json::json!({
            "sso_token":token.expose_to_backend(),
            "equipment_type":1,
            "device_id":self.config.device_id.expose_to_backend()}),self.version_value().await.as_deref());
        let bytes=Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?);
        let doc=self.post(&path,&bytes,Authentication::Session,true).await?;
        let body=crypto::body(&doc)?;
        // The App accepts either a JSON number or a numeric string; anything
        // else stays unknown rather than becoming an implicit grant.
        let authentication_code=match body.get("authentication_code") {
            Some(serde_json::Value::Number(number))=>number.as_i64(),
            Some(serde_json::Value::String(text))=>text.trim().parse::<i64>().ok(),
            _=>None,
        };
        Ok(Some(TrustDecision { authentication_code }))
    }
    pub async fn business_enabled(&self)->Result<bool,Error> {        let payload=self.payload(serde_json::json!({"device_id":self.config.device_id.expose_to_backend()}),self.version_value().await.as_deref());
        let bytes=Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?);
        let doc=self.post(&self.config.endpoints.call_status,&bytes,Authentication::Session,false).await?;
        let value=crypto::body(&doc)?.get("cs_forward").ok_or(Error::new("app_login_response_invalid"))?;
        match value.as_str() {
            Some("2")=>Ok(true),Some("1")=>Ok(false),_=>match value.as_u64() {Some(2)=>Ok(true),Some(1)=>Ok(false),_=>Err(Error::new("app_login_response_invalid"))},
        }
    }
    pub async fn enable_business(&self)->Result<(),Error> {
        let payload=self.payload(serde_json::json!({"cs_forward":"2","device_id":self.config.device_id.expose_to_backend()}),self.version_value().await.as_deref());
        let bytes=Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?);
        self.post(&self.config.endpoints.enable_hosting,&bytes,Authentication::Session,true).await?;Ok(())
    }
}
