//! Matched bootstrap configuration and decoded account-reference preparation.
//!
//! A legacy plaintext/config value is NOT authenticated by successful unwrap.
//! This module accepts a privately matched original ARC bootstrap, preserves
//! opaque properties, and never changes roots, SNI, routes, endpoints or ready.
//! Client-property and reference decode keys must be supplied independently.
use super::{
    arc_session::OwnedBootstrapReply,
    legacy_config::{decode_configuration, ConfigDecodeKey, ConfigError, ConfigurationBytes},
    object_reference::{parse_object_reference, ObjectReference, ReferenceEndpoint, ReferenceError},
    rpc_pending::RpcConnectionScope,
    session::AttemptOwner,
};
use std::fmt;
use zeroize::Zeroize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentError { Scope, Configuration(ConfigError), Reference(ReferenceError) }
impl From<ConfigError> for DeploymentError { fn from(error: ConfigError) -> Self { Self::Configuration(error) } }
impl From<ReferenceError> for DeploymentError { fn from(error: ReferenceError) -> Self { Self::Reference(error) } }
impl DeploymentError {
    pub fn code(self) -> &'static str {
        match self { Self::Scope => "jegotrip_deployment_scope_invalid", Self::Configuration(error) => error.code(), Self::Reference(error) => error.code() }
    }
}

pub struct BootstrapDeployment {
    owner: AttemptOwner,
    scope: RpcConnectionScope,
    call_id: i32,
    properties: Vec<(Vec<u8>, Vec<u8>)>,
    account_reference: ObjectReference,
    account_alive: Vec<u8>,
    user_entry: Vec<u8>,
}
impl Drop for BootstrapDeployment {
    fn drop(&mut self) {
        for (key, value) in &mut self.properties { key.zeroize(); value.zeroize(); }
        self.properties.clear(); self.account_alive.zeroize(); self.user_entry.zeroize();
        // ObjectReference and its target/endpoints wipe their own owned fields.
    }
}
impl fmt::Debug for BootstrapDeployment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("BootstrapDeployment(<redacted>)") }
}

impl BootstrapDeployment {
    /// Capture only a matched successful original bootstrap. Wrong scope is
    /// refused before legacy decryption/parse, not rebound to a new connection.
    /// `reference_key` is the Common object-reference context, not the Client
    /// property context or any password/token used for SDK/gateway login.
    pub fn from_reply(reply: &OwnedBootstrapReply, owner: &AttemptOwner, scope: &RpcConnectionScope,
        reference_key: Option<&ConfigDecodeKey>) -> Result<Self, DeploymentError> {
        let material = reply.material_for(owner, scope).ok_or(DeploymentError::Scope)?;
        let reference = decode_configuration(material.account_entry, reference_key)?;
        let account_reference = parse_object_reference(reference.expose_to_backend())?;
        Ok(Self { owner: reply.owner().clone(), scope: scope.clone(), call_id: reply.call_id(),
            properties: material.properties.iter().map(|p| (p.key.to_vec(), p.value.to_vec())).collect(), account_reference,
            account_alive: material.account_alive.to_vec(), user_entry: material.user_entry.to_vec() })
    }
    fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && self.scope.is_open()
    }
    pub fn call_id(&self) -> i32 { self.call_id }
    pub fn account_reference_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Option<&ObjectReference> {
        if self.matches(owner, scope) { Some(&self.account_reference) } else { None }
    }
    /// These endpoints are parsed data only. The future connector still needs
    /// an explicit original endpoint/trust/authority binding and authorization;
    /// parsing does not dial, resolve, choose a root or replace a live IO.
    pub fn raw_tcp_candidates_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope)
        -> Result<&[ReferenceEndpoint], DeploymentError> {
        if !self.matches(owner, scope) { return Err(DeploymentError::Scope); }
        Ok(self.account_reference.direct_raw_tcp()?)
    }
    /// Decode a property only when requested under the ORIGINAL Client-key
    /// context. Unknown/missing values remain unknown; do not fill deployment
    /// defaults or recursively decode every opaque metadata/footer value.
    pub fn property_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, name: &[u8],
        client_key: Option<&ConfigDecodeKey>) -> Result<Option<ConfigurationBytes>, DeploymentError> {
        if !self.matches(owner, scope) { return Err(DeploymentError::Scope); }
        self.properties.iter().find(|(key, _)| key == name).map(|(_, value)| decode_configuration(value, client_key))
            .transpose().map_err(DeploymentError::from)
    }
    /// AccountAlive/UserEntry references are separate optional channels, not
    /// SIP gateways or proof that inbox/call/renewal delivery is available.
    pub fn auxiliary_references_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Option<(&[u8], &[u8])> {
        if self.matches(owner, scope) { Some((&self.account_alive, &self.user_entry)) } else { None }
    }
}

#[cfg(test)]
#[path = "deployment_tests.rs"]
mod tests;
