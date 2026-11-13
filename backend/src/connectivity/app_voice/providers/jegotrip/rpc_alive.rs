//! Passive AccountAlive RPC wire codec, not a connection or registration grant.
//!
//! 593000/5B7EE0: type0, method-version list [0], original UID and selector,
//! wall-clock timestamp64, interval32. The two strings are the same ClientI
//! +2328/+2152 pair passed to swapCalls at590C08; they are not an App UUID or
//! a replacement phone/ARC token. 5B5580 consumes bool + Common footer.
//! No endpoint, transport, timer, retry, current clock or checkAlive state is
//! installed here. A decoded positive bit remains uncorrelated peer DATA.
use super::rpc_stream::{read_footer, CodecError, Reader, RpcBytes, RpcFooter, Writer, MAX_FIELD_BYTES, MAX_STREAM_BYTES};
use std::fmt;

pub(crate) const REGISTER_ALIVE_METHOD: &str = "registerAlive.AccountAlive.Account";
const REQUEST_OVERHEAD: usize = 1 + 4 + 4 + 4 + 8 + 4;

/// Inputs must later be captured from the original authenticated session by
/// its owning caller. This borrowed DATA type does not prove that provenance.
pub(crate) struct RegisterAliveArguments<'a> {
    pub user_id: &'a [u8],
    pub selected_session_id: &'a [u8],
    pub request_timestamp_ms: u64,
    pub interval_seconds: u32,
}
impl fmt::Debug for RegisterAliveArguments<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RegisterAliveArguments(<redacted>)") }
}
fn validate(arguments: &RegisterAliveArguments<'_>) -> Result<(), CodecError> {
    if arguments.user_id.is_empty() || arguments.selected_session_id.is_empty() { return Err(CodecError::InvalidField); }
    if [arguments.user_id, arguments.selected_session_id].iter().any(|value| value.len() > MAX_FIELD_BYTES) { return Err(CodecError::TooLarge); }
    let total = REQUEST_OVERHEAD.checked_add(arguments.user_id.len()).and_then(|size| size.checked_add(arguments.selected_session_id.len())).ok_or(CodecError::TooLarge)?;
    if total > MAX_STREAM_BYTES { return Err(CodecError::TooLarge); }
    Ok(())
}

/// Preserve every numeric bit; zero/max are representable wire DATA, not a
/// default interval, a stop command, a clock reading or permission to renew.
pub(crate) fn encode_register_alive(arguments: &RegisterAliveArguments<'_>) -> Result<RpcBytes, CodecError> {
    validate(arguments)?;
    let mut writer = Writer::versioned()?;
    writer.bytes(arguments.user_id)?;
    writer.bytes(arguments.selected_session_id)?;
    writer.u64(arguments.request_timestamp_ms)?;
    writer.u32(arguments.interval_seconds)?;
    Ok(writer.finish())
}
pub(crate) fn decode_register_alive_arguments(bytes: &[u8]) -> Result<RegisterAliveArguments<'_>, CodecError> {
    let mut reader = Reader::versioned(bytes)?;
    let arguments = RegisterAliveArguments { user_id: reader.bytes()?, selected_session_id: reader.bytes()?, request_timestamp_ms: reader.u64()?, interval_seconds: reader.u32()? };
    reader.finish()?;
    validate(&arguments)?;
    Ok(arguments)
}

/// This result has no owner, connection, call correlation or capability brand.
/// In particular, returned_acceptance() is NOT "registered" or "alive".
pub(crate) struct RawRegisterAliveResult<'a> { accepted: bool, footer: RpcFooter<'a> }
impl fmt::Debug for RawRegisterAliveResult<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RawRegisterAliveResult(<redacted>)") }
}
impl RawRegisterAliveResult<'_> {
    pub(crate) fn returned_acceptance(&self) -> bool { self.accepted }
    pub(crate) fn footer(&self) -> &RpcFooter<'_> { &self.footer }
}
pub(crate) fn decode_register_alive_result(code: u32, bytes: &[u8]) -> Result<RawRegisterAliveResult<'_>, CodecError> {
    // Negotiation/error flags are not successful version0 results. This codec
    // does not perform the generated native agent's version retries.
    if code != 0 { return Err(CodecError::RemoteFailure); }
    let mut reader = Reader::new(bytes)?;
    let accepted = reader.boolean()?;
    let footer = read_footer(&mut reader)?;
    reader.finish()?;
    if !footer.reason.is_empty() { return Err(CodecError::RemoteFailure); }
    Ok(RawRegisterAliveResult { accepted, footer })
}

#[cfg(test)]
#[path = "rpc_alive_tests.rs"]
mod tests;
