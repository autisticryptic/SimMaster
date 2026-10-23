//! Native AccountEntry.logoutSession method boundary. Logout submits original
//! returned user ID and ORIGINAL selected session ID; not token/SipPwd/phonepwd.
use super::rpc_stream::{read_footer, CodecError, Reader, RpcBytes, RpcFooter, Writer, MAX_FIELD_BYTES};
use std::fmt;

pub const LOGOUT_SESSION_METHOD: &str = "logoutSession.AccountEntry.Account";
pub struct LogoutResult<'a> { pub accepted: bool, pub footer: RpcFooter<'a> }
impl fmt::Debug for LogoutResult<'_> { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("LogoutResult(<redacted>)") } }

pub fn encode_logout_session(user_id: &[u8], selected_session_id: &[u8]) -> Result<RpcBytes, CodecError> {
    if user_id.is_empty() || selected_session_id.is_empty() || selected_session_id.len() > 127
        || [user_id,selected_session_id].iter().any(|v| v.len() > MAX_FIELD_BYTES || v.iter().any(u8::is_ascii_control)) {
        return Err(CodecError::InvalidField);
    }
    let mut writer = Writer::versioned()?; writer.bytes(user_id)?; writer.bytes(selected_session_id)?; Ok(writer.finish())
}
pub fn decode_logout_result(code: u32, bytes: &[u8]) -> Result<LogoutResult<'_>, CodecError> {
    if code != 0 { return Err(CodecError::RemoteFailure); }
    let mut reader = Reader::new(bytes)?; let accepted = reader.boolean()?;
    let footer = read_footer(&mut reader)?; reader.finish()?;
    if !footer.reason.is_empty() { return Err(CodecError::RemoteFailure); }
    Ok(LogoutResult { accepted, footer })
}

#[cfg(test)]
#[path = "rpc_logout_tests.rs"]
mod tests;
