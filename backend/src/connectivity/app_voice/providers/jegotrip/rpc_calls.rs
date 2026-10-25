//! Recovered CallEx argument/result DATA, not a call runtime or media grant.
//!
//! Native call identifiers are full BE64 values, distinct from the enclosing
//! ARC/direct/batch i32 correlation ID. Properties/peer data remain opaque.
//! These codecs cannot establish original receiver/agent ownership, authorize
//! answering, infer SDP/crypto/endpoints, send a command or mark media ready.
use super::rpc_stream::{read_footer, CodecError, Reader, RpcBytes, RpcFooter, RpcProperty, Writer};
use std::fmt;

pub const ACCEPT_METHOD: &str = "accept.CallServerEx.CallEx";
pub const INVITED_METHOD: &str = "onInvited.CallClientEx.CallEx";
pub const RELEASED_METHOD: &str = "onReleased.CallClientEx.CallEx";
pub const ACCEPTED_METHOD: &str = "onAccepted.CallClientEx.CallEx";

/// A value, not an original-call capability or a remote delivery identifier.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct NativeCallId(u64);
impl NativeCallId {
    pub fn from_bits(value: u64) -> Self { Self(value) }
    pub fn expose_to_backend(self) -> u64 { self.0 }
}
impl fmt::Debug for NativeCallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("NativeCallId(<redacted>)") }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallNoticeKind { Invited, Released, Accepted }
pub struct CallNotice<'a> {
    pub kind: CallNoticeKind,
    pub call_id: NativeCallId,
    pub peer: Option<&'a [u8]>,
    pub properties: Vec<RpcProperty<'a>>,
}
impl fmt::Debug for CallNotice<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("CallNotice(<redacted>)") }
}

/// Exact recovered receiver schemas. Unknown methods are refused BEFORE
/// parsing; a surrounding original-scope dispatcher must validate object/domain
/// and capture its original reply ticket before using any resulting data.
pub fn decode_notice<'a>(method: &[u8], arguments: &'a [u8]) -> Result<Option<CallNotice<'a>>, CodecError> {
    let kind = if method == INVITED_METHOD.as_bytes() { CallNoticeKind::Invited }
        else if method == RELEASED_METHOD.as_bytes() { CallNoticeKind::Released }
        else if method == ACCEPTED_METHOD.as_bytes() { CallNoticeKind::Accepted }
        else { return Ok(None); };
    let mut reader = Reader::versioned(arguments)?;
    let call_id = NativeCallId(reader.u64()?);
    let peer = if kind == CallNoticeKind::Invited { Some(reader.bytes()?) } else { None };
    let properties = reader.properties()?;
    reader.finish()?;
    Ok(Some(CallNotice { kind, call_id, peer, properties }))
}

/// Encoding alone must never bypass an original invitation/agent, explicit
/// action policy or supported-media validation. This module performs no IO.
pub fn encode_accept(call_id: NativeCallId, properties: &[RpcProperty<'_>]) -> Result<RpcBytes, CodecError> {
    let mut writer = Writer::versioned()?;
    writer.u64(call_id.0)?;
    writer.properties(properties)?;
    Ok(writer.finish())
}

pub struct AcceptResult<'a> {
    pub properties: Vec<RpcProperty<'a>>,
    pub footer: RpcFooter<'a>,
}
impl fmt::Debug for AcceptResult<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AcceptResult(<redacted>)") }
}
/// This result is not SDP, a channel acknowledgement or an active-media event.
/// Caller must match its ORIGINAL pending RPC owner/scope/sequence first. The
/// native version-retry callback is deliberately not copied into an IO retry.
pub fn decode_accept_result(result_code: u32, bytes: &[u8]) -> Result<AcceptResult<'_>, CodecError> {
    if result_code != 0 { return Err(CodecError::RemoteFailure); }
    let mut reader = Reader::new(bytes)?;
    let accepted = reader.boolean()?;
    let properties = reader.properties()?;
    let footer = read_footer(&mut reader)?;
    reader.finish()?;
    if !accepted || !footer.reason.is_empty() { return Err(CodecError::RemoteFailure); }
    Ok(AcceptResult { properties, footer })
}

#[cfg(test)]
#[path = "rpc_calls_tests.rs"]
mod tests;
