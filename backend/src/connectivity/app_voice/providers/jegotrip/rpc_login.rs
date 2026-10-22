//! Recovered AccountEntry loginSession argument/result boundary, not login auth.
//!
//! The two input strings and deployment map must come from the verified ARC
//! identity/auth/provisioning path. Never guess them from an imported token or
//! substitute the hosting SipPwd. TLS/endpoint/auth discovery is not implemented.
use super::rpc_stream::{read_footer, CodecError, Reader, RpcBytes, RpcFooter, RpcProperty, Writer};
use std::fmt;

pub const LOGIN_SESSION_METHOD: &str = "loginSession.AccountEntry.Account";

pub struct LoginSessionResult<'a> {
    pub user_id: &'a [u8],
    pub session_token: &'a [u8],
    pub properties: Vec<RpcProperty<'a>>,
    pub footer: RpcFooter<'a>,
}

impl fmt::Debug for LoginSessionResult<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LoginSessionResult(<redacted>)")
    }
}

pub fn encode_login_session(normalized_identity: &[u8], native_session_argument: &[u8],
    deployment_properties: &[RpcProperty<'_>]) -> Result<RpcBytes, CodecError> {
    if normalized_identity.is_empty() || native_session_argument.is_empty() {
        return Err(CodecError::InvalidField);
    }
    let mut writer = Writer::versioned()?;
    writer.bytes(normalized_identity)?;
    writer.bytes(native_session_argument)?;
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
    Ok(LoginSessionResult { user_id, session_token, properties, footer })
}

#[cfg(test)]
#[path = "rpc_login_tests.rs"]
mod tests;
