//! Original-attempt connector for the explicit native PacketSender/TLS profile.
//!
//! A TCP connection is NOT a TLS/SDK/hosting/channel acknowledgement. Only an
//! explicit direct tcp reference with the ORIGINAL TLS authority is admitted;
//! adapters, locators and alternate native transports stay unsupported. There
//! are no built-in hosts, roots, ports, credentials, redirects or fallback TLS.
use super::{
    legacy_config::{decode_configuration, ConfigDecodeKey},
    object_reference::parse_object_reference,
    rpc_pending::RpcConnectionScope,
    session::AttemptOwner,
    tls_engine::valid_authority,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{fmt, future::{poll_fn, Future}, net::{IpAddr, SocketAddr}, pin::Pin, task::Poll, time::Duration};
use tokio::{io::{AsyncRead, AsyncWrite}, net::TcpStream};
use crate::platform::dns::prepare_socket_addrs;
use zeroize::Zeroizing;

const MAX_TARGET_BYTES: usize = 65_536;
const MAX_ROOT_BYTES: usize = 16_384;
const MAX_ADDRESSES: usize = 16;
const MAX_RESOLVED_RESULTS: usize = 64;

pub trait RuntimeIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> RuntimeIo for T {}
pub type BoxRuntimeIo = Box<dyn RuntimeIo>;

#[derive(Clone, Copy)]
pub struct ConnectionInput<'a> {
    /// Exact imported/provisioned object reference, not a SIP URI or locator host.
    pub server_reference: &'a str,
    pub authority: &'a str,
    pub root_der_base64: &'a str,
    pub enable_sni: bool,
    /// Original Common reference context, never a Client-property/login key.
    pub reference_decode_key: Option<&'a ConfigDecodeKey>,
}
impl fmt::Debug for ConnectionInput<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ConnectionInput(<redacted>)") }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectError { Scope, InvalidConfiguration, UnsupportedProfile, ResourceLimit, Timeout, Io }
impl ConnectError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Scope => "jegotrip_connector_scope_invalid",
            Self::InvalidConfiguration => "jegotrip_connector_configuration_invalid",
            Self::UnsupportedProfile => "jegotrip_connector_profile_unsupported",
            Self::ResourceLimit => "jegotrip_connector_resource_limit",
            Self::Timeout => "jegotrip_connector_timeout",
            Self::Io => "jegotrip_connector_io_failed",
        }
    }
}

/// Owned IO bound BEFORE return to the exact original request and physical
/// scope. It cannot be cloned or relabelled. The TLS engine still verifies the
/// certificate; a trusted connector implementation is not an authentication API.
pub struct ConnectedIo {
    owner: AttemptOwner,
    scope: RpcConnectionScope,
    reference: Zeroizing<String>,
    authority: Zeroizing<String>,
    root: Zeroizing<String>,
    enable_sni: bool,
    io: BoxRuntimeIo,
}
impl fmt::Debug for ConnectedIo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ConnectedIo(<redacted>)") }
}
impl ConnectedIo {
    /// For trusted connector adapters (including isolated Actions duplex peers).
    /// No caller-facing credential/API endpoint may construct this capability.
    pub(super) fn original<I: RuntimeIo + 'static>(owner: &AttemptOwner, scope: &RpcConnectionScope,
        input: ConnectionInput<'_>, io: I) -> Result<Self, ConnectError> {
        validate_input(input)?;
        if !scope.is_open() { return Err(ConnectError::Scope); }
        Ok(Self { owner: owner.clone(), scope: scope.clone(),
            reference: Zeroizing::new(input.server_reference.to_owned()),
            authority: Zeroizing::new(input.authority.to_owned()), root: Zeroizing::new(input.root_der_base64.to_owned()),
            enable_sni: input.enable_sni, io: Box::new(io) })
    }
    pub fn into_original_io(self, owner: &AttemptOwner, scope: &RpcConnectionScope,
        input: ConnectionInput<'_>) -> Result<BoxRuntimeIo, ConnectError> {
        if &self.owner != owner || !self.scope.matches(scope) || !self.scope.is_open()
            || self.reference.as_str() != input.server_reference || self.authority.as_str() != input.authority
            || self.root.as_str() != input.root_der_base64 || self.enable_sni != input.enable_sni {
            return Err(ConnectError::Scope);
        }
        Ok(self.io)
    }
}

pub type ConnectFuture<'a> = Pin<Box<dyn Future<Output = Result<ConnectedIo, ConnectError>> + Send + 'a>>;
pub trait RuntimeConnector: Send + Sync {
    fn connect<'a>(&'a self, owner: &'a AttemptOwner, scope: &'a RpcConnectionScope,
        input: ConnectionInput<'a>) -> ConnectFuture<'a>;
}

fn validate_input(input: ConnectionInput<'_>) -> Result<(), ConnectError> {
    if !valid_authority(input.authority) || input.server_reference.is_empty()
        || input.server_reference.len() > MAX_TARGET_BYTES || input.server_reference.chars().any(char::is_control)
        || input.root_der_base64.is_empty() || input.root_der_base64.len() > (MAX_ROOT_BYTES + 2) / 3 * 4 {
        return Err(ConnectError::InvalidConfiguration);
    }
    let root = STANDARD.decode(input.root_der_base64).map_err(|_| ConnectError::InvalidConfiguration)?;
    if root.is_empty() || root.len() > MAX_ROOT_BYTES { return Err(ConnectError::InvalidConfiguration); }
    let mut store = tokio_rustls::rustls::RootCertStore::empty();
    store.add(tokio_rustls::rustls::pki_types::CertificateDer::from(root))
        .map_err(|_| ConnectError::InvalidConfiguration)?;
    Ok(())
}

pub(super) fn same_authority(host: &str, authority: &str) -> bool {
    match (host.parse::<IpAddr>(), authority.parse::<IpAddr>()) {
        (Ok(a), Ok(b)) => a == b,
        (Err(_), Err(_)) => host.eq_ignore_ascii_case(authority),
        _ => false,
    }
}

struct Target { host: Zeroizing<String>, port: u16 }
fn targets(input: ConnectionInput<'_>) -> Result<Vec<Target>, ConnectError> {
    validate_input(input)?;
    let decoded = decode_configuration(input.server_reference.as_bytes(), input.reference_decode_key)
        .map_err(|_| ConnectError::InvalidConfiguration)?;
    let reference = parse_object_reference(decoded.expose_to_backend()).map_err(|_| ConnectError::UnsupportedProfile)?;
    // direct_raw_tcp rejects the ENTIRE reference if any endpoint needs another
    // transport. Never silently select a tcp alternative to required tls/udp/etc.
    let endpoints = reference.direct_raw_tcp().map_err(|_| ConnectError::UnsupportedProfile)?;
    if endpoints.len() > MAX_ADDRESSES { return Err(ConnectError::ResourceLimit); }
    let mut targets = Vec::new();
    for endpoint in endpoints {
        let host = std::str::from_utf8(endpoint.host()).map_err(|_| ConnectError::InvalidConfiguration)?;
        if !same_authority(host, input.authority) { return Err(ConnectError::UnsupportedProfile); }
        if !targets.iter().any(|prior: &Target| prior.host.as_str() == host && prior.port == endpoint.port()) {
            targets.push(Target { host: Zeroizing::new(host.to_owned()), port: endpoint.port() });
        }
    }
    if targets.is_empty() { return Err(ConnectError::UnsupportedProfile); }
    Ok(targets)
}

/// Serialize every NONBLOCKING future poll with original retirement, including
/// the first poll that may schedule DNS or issue a TCP connect. Resolver system
/// configuration is prepared outside the gate; only nonblocking network work
/// is polled within it. Accepted OS/resolver work cannot be recalled, but its
/// result cannot authorize a later scope. No await/blocking reads in the gate.
async fn poll_scoped<F: Future>(scope: &RpcConnectionScope, future: F) -> Result<F::Output, ConnectError> {
    tokio::pin!(future);
    poll_fn(|context| match scope.with_io_gate(|| future.as_mut().poll(context)) {
        Some(result) => result.map(Ok), None => Poll::Ready(Err(ConnectError::Scope)),
    }).await
}

/// Explicit production connector. Construction does not resolve or connect.
/// The whole DNS/address attempt group shares ONE local deadline and original
/// scope. No target, wildcard, proxy, alias, root or active IO is synthesized.
pub struct TcpRuntimeConnector { timeout: Duration }
impl fmt::Debug for TcpRuntimeConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("TcpRuntimeConnector(<redacted>)") }
}
impl TcpRuntimeConnector {
    pub fn new(timeout: Duration) -> Result<Self, ConnectError> {
        if timeout.is_zero() || timeout > Duration::from_secs(10) { return Err(ConnectError::InvalidConfiguration); }
        Ok(Self { timeout })
    }
}
impl RuntimeConnector for TcpRuntimeConnector {
    fn connect<'a>(&'a self, owner: &'a AttemptOwner, scope: &'a RpcConnectionScope,
        input: ConnectionInput<'a>) -> ConnectFuture<'a> {
        Box::pin(async move {
            if !scope.is_open() { return Err(ConnectError::Scope); }
            let targets = targets(input)?;
            if !scope.is_open() { return Err(ConnectError::Scope); }
            let group = async {
                let mut attempted = Vec::<SocketAddr>::new(); let mut resolved_results = 0usize;
                for target in targets {
                    let lookup = prepare_socket_addrs(target.host.as_str(), target.port).map_err(|_| ConnectError::Io)?;
                    let addresses = poll_scoped(scope, lookup.resolve()).await?.map_err(|_| ConnectError::Io)?;
                    for address in addresses {
                        resolved_results += 1;
                        if resolved_results > MAX_RESOLVED_RESULTS { return Err(ConnectError::ResourceLimit); }
                        if attempted.contains(&address) { continue; }
                        if attempted.len() == MAX_ADDRESSES { return Err(ConnectError::ResourceLimit); }
                        attempted.push(address);
                        if !scope.is_open() { return Err(ConnectError::Scope); }
                        if let Ok(io) = poll_scoped(scope, TcpStream::connect(address)).await? {
                            io.set_nodelay(true).map_err(|_| ConnectError::Io)?;
                            return ConnectedIo::original(owner, scope, input, io);
                        }
                    }
                }
                Err(ConnectError::Io)
            };
            tokio::select! { biased;
                _ = scope.closed() => Err(ConnectError::Scope),
                result = tokio::time::timeout(self.timeout, group) => result.unwrap_or(Err(ConnectError::Timeout)),
            }
        })
    }
}

#[cfg(test)]
#[path = "runtime_connector_tests.rs"]
mod tests;
