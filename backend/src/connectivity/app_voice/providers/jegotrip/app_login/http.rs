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

pub(super) struct WireRequest { pub url: Url, pub body: Zeroizing<Vec<u8>> }
pub(super) struct WireResponse { pub status: u16, pub body: Zeroizing<Vec<u8>> }
pub(super) type Exchange<'a> = Pin<Box<dyn Future<Output=Result<WireResponse,Error>>+Send+'a>>;
pub(super) trait Transport: Send+Sync { fn exchange(&self, request:WireRequest)->Exchange<'_>; }
pub(super) trait TransportFactory: Send+Sync { fn build(&self, config:&HttpDeployment)->Result<AppClient,Error>; }
pub(super) struct LiveFactory;

/// Exact end-entity certificate pin, mirroring the App's bundled `assets/app2.cer`.
/// The operator supplies one concrete server certificate; no system root and no
/// CA path is consulted, and any other certificate fails the handshake.
#[derive(Debug)]
struct PinnedLeaf {
    leaf: Vec<u8>,
    provider: Arc<tokio_rustls::rustls::crypto::CryptoProvider>,
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
    fn verify_tls12_signature(&self, message: &[u8], cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct)
        -> Result<tokio_rustls::rustls::client::danger::HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        tokio_rustls::rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct)
        -> Result<tokio_rustls::rustls::client::danger::HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        tokio_rustls::rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
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

fn pinned_tls(der: Vec<u8>) -> Result<tokio_rustls::rustls::ClientConfig, Error> {
    eprintln!("app_login_pinned_tls installing pinned_len={}", der.len());
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let config = tokio_rustls::rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions().map_err(|_| Error::new("app_login_deployment_invalid"))?
        .dangerous().with_custom_certificate_verifier(Arc::new(PinnedLeaf { leaf: der, provider })).with_no_client_auth();
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
            builder=builder.tls_built_in_root_certs(false).use_preconfigured_tls(pinned_tls(der)?);
        }
        if let Some(agent)=&config.user_agent { builder=builder.user_agent(agent.expose_to_backend()); }
        let client=builder.build().map_err(|_|Error::new("app_login_deployment_invalid"))?;
        AppClient::new(config.clone(),Arc::new(LiveTransport(client)))
    }
}
impl Transport for LiveTransport {
    fn exchange(&self, request:WireRequest)->Exchange<'_> { Box::pin(async move {
        let response=self.0.post(request.url).header(reqwest::header::CONTENT_TYPE,"application/json; charset=utf-8")
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
impl BoundNumber {
    pub fn identity(&self)->Zeroizing<String> { Zeroizing::new(format!("+{}{}",self.country,self.mobile.expose_to_backend())) }
    pub fn masked(&self)->String { let s=self.mobile.expose_to_backend();format!("***{}",&s[s.len().saturating_sub(4)..]) }
}
pub(super) struct AppClient { config:HttpDeployment, transport:Arc<dyn Transport>, session:Option<Secret> }
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
    pub fn new(config:HttpDeployment,transport:Arc<dyn Transport>)->Result<Self,Error> { config.validate()?;Ok(Self {config,transport,session:None}) }
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
        }
        let response=self.transport.exchange(WireRequest {url,body:request_body}).await?;
        if response.status==429 { return Err(Error::new("app_login_rate_limited")); }
        if response.status!=200 { return Err(Error::new("app_login_transport_failed")); }
        let mut doc=crypto::parse(&response.body)?;
        // Outer App codes are inspected independently from body decryption. In
        // particular 2006 must NOT trigger the APK's implicit closeVoipForward.
        crypto::success(&doc)?; crypto::decrypt_body(&mut doc,protection)?; Ok(doc)
    }
    pub async fn login(&mut self,request:&LoginRequest)->Result<(),Error> {
        let hash=crypto::password_hash(&self.config.password_salt,&request.password);
        let mut payload=JsonDoc(serde_json::json!({"loginName":request.username.expose_to_backend().trim(),
            "password":hash.expose_to_backend(),"userType":"0","site":1}));
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
        let identity=bound.identity();let payload=JsonDoc(serde_json::json!({"accountId":&**identity}));
        let bytes=Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?);
        let token_doc=self.post(&self.config.endpoints.phone_token,&bytes,Authentication::Session,true).await?;
        let token=crypto::secret_field(crypto::body(&token_doc)?,"token")?;
        let sip_doc=self.post(&self.config.endpoints.voip_status,b"",Authentication::Session,false).await?;
        let encrypted=crypto::secret_field(crypto::body(&sip_doc)?,"sip_password")?;
        let sip_password=crypto::sip_password(&encrypted,&self.config)?;
        Ok(PhoneMaterial {token,sip_password})
    }
    pub async fn business_enabled(&self)->Result<bool,Error> {
        let payload=JsonDoc(serde_json::json!({"device_id":self.config.device_id.expose_to_backend()}));
        let bytes=Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?);
        let doc=self.post(&self.config.endpoints.call_status,&bytes,Authentication::Session,false).await?;
        let value=crypto::body(&doc)?.get("cs_forward").ok_or(Error::new("app_login_response_invalid"))?;
        match value.as_str() {
            Some("2")=>Ok(true),Some("1")=>Ok(false),_=>match value.as_u64() {Some(2)=>Ok(true),Some(1)=>Ok(false),_=>Err(Error::new("app_login_response_invalid"))},
        }
    }
    pub async fn enable_business(&self)->Result<(),Error> {
        let payload=JsonDoc(serde_json::json!({"cs_forward":"2","device_id":self.config.device_id.expose_to_backend()}));
        let bytes=Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?);
        self.post(&self.config.endpoints.enable_hosting,&bytes,Authentication::Session,true).await?;Ok(())
    }
}
