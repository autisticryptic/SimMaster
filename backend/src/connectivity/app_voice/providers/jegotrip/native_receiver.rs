//! Original local client-adapter namespace, separate from account/HTTP domains.
//!
//! The reviewed client registers MessageReceiver and CallClientEx on one local
//! adapter. DIRECT receives use that connection's domain filter; authenticated
//! swapCalls deliveries reach the adapter table without repeating that filter.
//! This bounded mode supports only the unqualified registered identities. It
//! does not resolve facets, cloud IDs, remote adapters or arbitrary endpoints.
use super::{rpc_stream::{CodecError, RpcProperty, MAX_FIELD_BYTES, MAX_MAP_ENTRIES, MAX_STREAM_BYTES},
    session::AttemptOwner, rpc_pending::RpcConnectionScope};
use serde::Deserialize;
use std::{fmt, sync::Arc};
use zeroize::Zeroize;

pub(super) const MESSAGE_OBJECT: &[u8] = b"MessageReceiver";
pub(super) const CALL_OBJECT: &[u8] = b"CallClientEx";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeReceiverConfig {
    /// None is the original connection default: an empty deny-list. This is
    /// local reception policy, not a token, service grant or TLS trust anchor.
    #[serde(default)]
    pub direct_domains: Option<DirectDomainConfig>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectDomainConfig { pub mode: DomainMode, pub values: Vec<Vec<u8>> }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainMode { DenyList, AllowList }
impl Drop for DirectDomainConfig { fn drop(&mut self) { self.values.zeroize(); } }
impl fmt::Debug for NativeReceiverConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("NativeReceiverConfig(<redacted>)") }
}
impl fmt::Debug for DirectDomainConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("DirectDomainConfig(<redacted>)") }
}

pub(super) struct NativeReceiverPolicy { mode: DomainMode, domains: Vec<Vec<u8>>, retained: usize }
impl Drop for NativeReceiverPolicy { fn drop(&mut self) { self.domains.zeroize(); } }
impl fmt::Debug for NativeReceiverPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("NativeReceiverPolicy(<redacted>)") }
}
impl NativeReceiverPolicy {
    pub(super) fn capture(config: &NativeReceiverConfig) -> Result<Arc<Self>, CodecError> {
        let mode = config.direct_domains.as_ref().map_or(DomainMode::DenyList, |filter| filter.mode);
        let values = config.direct_domains.as_ref().map_or(&[][..], |filter| filter.values.as_slice());
        if values.len() > MAX_MAP_ENTRIES { return Err(CodecError::TooLarge); }
        let mut retained = 0usize;
        for (index, value) in values.iter().enumerate() {
            if value.is_empty() || value.iter().any(u8::is_ascii_control) || value.windows(4).any(|part| part == b"ENC(") {
                return Err(CodecError::InvalidField);
            }
            if value.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
            retained = retained.checked_add(value.len()).ok_or(CodecError::TooLarge)?;
            if retained > MAX_STREAM_BYTES { return Err(CodecError::TooLarge); }
            if values[..index].contains(value) { return Err(CodecError::DuplicateKey); }
        }
        Ok(Arc::new(Self { mode, domains: values.to_vec(), retained }))
    }
    pub(super) fn retained_bytes(&self) -> usize { self.retained }
    /// The caller must first check its private original-scope request brand.
    /// No raw HTTP/notification payload or bare decoder can mint that brand.
    pub(super) fn validate(&self, object: ReceiverObject, metadata: &[RpcProperty<'_>], transport: ReceiverTransport) -> Result<(), CodecError> {
        validate_metadata(metadata)?;
        let get = |key: &[u8]| metadata.iter().find(|property| property.key == key).map(|property| property.value);
        if get(b"id") != Some(object.identity()) { return Err(CodecError::InvalidField); }
        if transport == ReceiverTransport::Direct {
            let domain = get(b"domain").filter(|value| !value.is_empty());
            let listed = domain.is_some_and(|value| self.domains.iter().any(|expected| expected.as_slice() == value));
            let allowed = match self.mode { DomainMode::DenyList => !listed, DomainMode::AllowList => listed };
            if !allowed { return Err(CodecError::InvalidField); }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReceiverTransport { Direct, Batch }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReceiverObject { Message, Call }
impl ReceiverObject {
    pub(super) fn identity(self) -> &'static [u8] { match self { Self::Message => MESSAGE_OBJECT, Self::Call => CALL_OBJECT } }
}

/// Bounded DATA selected before login; retained by the original actor, never
/// populated from an uncorrelated notification or used as an auth grant.
pub(super) struct NativeReceiverSetup {
    pub(super) policy: Arc<NativeReceiverPolicy>,
    pub(super) client_ex: bool,
    control: Option<Vec<(Vec<u8>, Vec<u8>)>>,
    bytes: usize,
}
impl Drop for NativeReceiverSetup { fn drop(&mut self) { if let Some(control) = &mut self.control { for (key, value) in control { key.zeroize(); value.zeroize(); } } } }
impl fmt::Debug for NativeReceiverSetup { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("NativeReceiverSetup(<redacted>)") } }
impl NativeReceiverSetup {
    pub(super) fn capture(policy: Arc<NativeReceiverPolicy>, client_ex: bool, routing: Option<&[RpcProperty<'_>]>) -> Result<Arc<Self>, CodecError> {
        let mut bytes = policy.retained_bytes();
        if let Some(routing) = routing {
            super::call_control::validate_routing(routing)?;
            for p in routing { bytes = bytes.checked_add(p.key.len()).and_then(|n| n.checked_add(p.value.len())).ok_or(CodecError::TooLarge)?; }
        }
        if bytes > MAX_STREAM_BYTES { return Err(CodecError::TooLarge); }
        Ok(Arc::new(Self { policy, client_ex, control: routing.map(|routing| routing.iter().map(|p| (p.key.to_vec(), p.value.to_vec())).collect()), bytes }))
    }
    pub(super) fn retained_bytes(&self) -> usize { self.bytes }
    pub(super) fn control(&self) -> Option<Vec<RpcProperty<'_>>> {
        self.control.as_ref().map(|routing| routing.iter().map(|(key, value)| RpcProperty { key, value }).collect())
    }
}

/// Private local dispatch setup. Captured before IO and activated only by a
/// matched login on that original Common scope, never by the configuration flag.
#[derive(Clone)]
pub(super) struct NativeReceiverBinding { owner: AttemptOwner, scope: RpcConnectionScope, policy: Arc<NativeReceiverPolicy> }
impl fmt::Debug for NativeReceiverBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("NativeReceiverBinding(<redacted>)") }
}
impl NativeReceiverBinding {
    pub(super) fn original(owner: AttemptOwner, scope: RpcConnectionScope, policy: Arc<NativeReceiverPolicy>) -> Result<Self, CodecError> {
        if !scope.is_open() || scope.session_revoked() { return Err(CodecError::InvalidField); }
        Ok(Self { owner, scope, policy })
    }
    pub(super) fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && scope.is_open() && !scope.session_revoked()
    }
    pub(super) fn policy(&self) -> &Arc<NativeReceiverPolicy> { &self.policy }
}
fn validate_metadata(properties: &[RpcProperty<'_>]) -> Result<(), CodecError> {
    if properties.len() > MAX_MAP_ENTRIES { return Err(CodecError::TooLarge); }
    let mut size = 0usize;
    for (index, property) in properties.iter().enumerate() {
        if property.key.len() > MAX_FIELD_BYTES || property.value.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
        size = size.checked_add(property.key.len()).and_then(|size| size.checked_add(property.value.len())).ok_or(CodecError::TooLarge)?;
        if size > MAX_STREAM_BYTES { return Err(CodecError::TooLarge); }
        if properties[..index].iter().any(|prior| prior.key == property.key) { return Err(CodecError::DuplicateKey); }
    }
    Ok(())
}

#[cfg(test)]
#[path = "native_receiver_tests.rs"]
mod tests;
