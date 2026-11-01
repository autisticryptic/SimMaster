//! Recovered AccountEntry loginSession argument/result boundary, not login auth.
//!
//! The second input string is the originally selected SDK session id, not the
//! password or returned ARC auth token. The phone-login token is carried by the
//! per-call `pwd` metadata. TLS/endpoint/provisioning discovery is still separate.
use super::rpc_stream::{read_footer, CodecError, Reader, RpcBytes, RpcFooter, RpcProperty, Writer, MAX_FIELD_BYTES};
use std::fmt;

pub const LOGIN_SESSION_METHOD: &str = "loginSession.AccountEntry.Account";

pub struct LoginSessionResult<'a> {
    pub user_id: &'a [u8],
    pub session_token: &'a [u8],
    pub properties: Vec<RpcProperty<'a>>,
    /// Native `__adjustOutParams` reads these from the SAME reply: the
    /// `mainAccountServer` names the loginSessionAgent the reply belongs to and
    /// `mainAccount` is stored on the original account object. They are data
    /// only, never a new agent, URI, trust anchor or account grant.
    pub main_account_server: Option<&'a [u8]>,
    pub main_account: Option<&'a [u8]>,
    pub footer: RpcFooter<'a>,
}

fn optional_property<'a>(properties: &[RpcProperty<'a>], name: &[u8]) -> Result<Option<&'a [u8]>, CodecError> {
    let Some(value) = properties.iter().find(|p| p.key == name).map(|p| p.value) else { return Ok(None); };
    if value.is_empty() || value.len() > MAX_FIELD_BYTES || value.iter().any(u8::is_ascii_control)
        || value.windows(4).any(|bytes| bytes == b"ENC(") {
        return Err(CodecError::InvalidField);
    }
    Ok(Some(value))
}

impl fmt::Debug for LoginSessionResult<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LoginSessionResult(<redacted>)")
    }
}

pub struct NormalizedIdentity(Vec<u8>);

impl NormalizedIdentity {
    pub fn expose_to_backend(&self) -> &[u8] { &self.0 }
}

impl fmt::Debug for NormalizedIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("NormalizedIdentity(<redacted>)") }
}

/// Matches native identity normalization without choosing an identity type or
/// inventing a deployment domain. `typed_identity` must be captured from the
/// native identity stage. Already-normalized colon-containing inputs follow the
/// same native colon-first branch; they are not silently stripped or rewritten.
pub fn normalize_session_identity(typed_identity: &[u8], domain_name: &[u8])
    -> Result<NormalizedIdentity, CodecError> {
    if typed_identity.is_empty() || typed_identity.len() > MAX_FIELD_BYTES
        || typed_identity.iter().any(|byte| byte.is_ascii_control())
        || typed_identity.windows(4).any(|bytes| bytes == b"ENC(") {
        return Err(CodecError::InvalidField);
    }
    let typed = typed_identity.iter().position(|&byte| byte == b':').is_some_and(|position| position > 0);
    if !typed && typed_identity[0] == b'[' { return Ok(NormalizedIdentity(typed_identity.to_vec())); }
    if domain_name.is_empty() || domain_name.len() > MAX_FIELD_BYTES
        || domain_name.iter().any(|byte| byte.is_ascii_control())
        || domain_name.windows(4).any(|bytes| bytes == b"ENC(") {
        return Err(CodecError::InvalidField);
    }
    let size = typed_identity.len().checked_add(domain_name.len()).and_then(|size| size.checked_add(if typed { 3 } else { 7 }))
        .ok_or(CodecError::TooLarge)?;
    if size > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
    let mut identity = Vec::with_capacity(size);
    identity.extend_from_slice(if typed { b"[".as_slice() } else { b"[uid:".as_slice() });
    identity.extend_from_slice(typed_identity);
    identity.push(b'@');
    identity.extend_from_slice(domain_name);
    identity.push(b']');
    Ok(NormalizedIdentity(identity))
}

pub fn encode_login_session(normalized_identity: &[u8], selected_session_id: &[u8],
    deployment_properties: &[RpcProperty<'_>]) -> Result<RpcBytes, CodecError> {
    if normalized_identity.is_empty() || selected_session_id.is_empty() {
        return Err(CodecError::InvalidField);
    }
    let mut writer = Writer::versioned()?;
    writer.bytes(normalized_identity)?;
    writer.bytes(selected_session_id)?;
    writer.properties(deployment_properties)?;
    Ok(writer.finish())
}

/// Native login success requires RPC result zero, a true boolean and both
/// nonempty returned values. Server properties are opaque, not verified expiry
/// or refresh semantics for the user's imported phone token.
pub fn decode_login_session_result(result_code: u32, bytes: &[u8])
    -> Result<LoginSessionResult<'_>, CodecError> {
    if result_code != 0 { return Err(CodecError::RemoteFailure); }
    let mut reader = Reader::new(bytes)?;
    let success = reader.boolean()?;
    let user_id = reader.bytes()?;
    let session_token = reader.bytes()?;
    let properties = reader.properties()?;
    let footer = read_footer(&mut reader)?;
    reader.finish()?;
    if !success || !footer.reason.is_empty() { return Err(CodecError::RemoteFailure); }
    if user_id.is_empty() || session_token.is_empty() { return Err(CodecError::InvalidField); }
    let main_account_server = optional_property(&properties, b"mainAccountServer")?;
    let main_account = optional_property(&properties, b"mainAccount")?;
    Ok(LoginSessionResult { user_id, session_token, properties, main_account_server, main_account, footer })
}

#[cfg(test)]
#[path = "rpc_login_tests.rs"]
mod tests;
