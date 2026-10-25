//! Recovered CallEx argument/result DATA, not a call runtime or media grant.
//!
//! Native call identifiers are full BE64 values, distinct from the enclosing
//! ARC/direct/batch i32 correlation ID. Properties/peer data remain opaque.
//! These codecs cannot establish original receiver/agent ownership, authorize
//! answering, infer SDP/crypto/endpoints, send a command or mark media ready.
use super::rpc_stream::{read_footer, CodecError, Reader, RpcBytes, RpcFooter, RpcProperty, Writer};
use std::fmt;

pub const ACCEPT_METHOD: &str = "accept.CallServerEx.CallEx";
pub const INFORM_METHOD: &str = "inform.CallServerEx.CallEx";
pub const RELEASE_METHOD: &str = "release.CallServerEx.CallEx";
pub const INVITED_METHOD: &str = "onInvited.CallClientEx.CallEx";
pub const RELEASED_METHOD: &str = "onReleased.CallClientEx.CallEx";
pub const ACCEPTED_METHOD: &str = "onAccepted.CallClientEx.CallEx";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallOperation { Inform, Accept, Release }
impl CallOperation {
    pub fn method(self) -> &'static str { match self { Self::Inform=>INFORM_METHOD, Self::Accept=>ACCEPT_METHOD, Self::Release=>RELEASE_METHOD } }
}

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

pub fn notice_kind(method:&[u8])->Option<CallNoticeKind>{
    if method==INVITED_METHOD.as_bytes(){Some(CallNoticeKind::Invited)}else if method==RELEASED_METHOD.as_bytes(){Some(CallNoticeKind::Released)}else if method==ACCEPTED_METHOD.as_bytes(){Some(CallNoticeKind::Accepted)}else{None}
}

/// Exact recovered receiver schemas. Unknown methods are refused BEFORE
/// parsing; a surrounding original-scope dispatcher must validate object/domain
/// and capture its original reply ticket before using any resulting data.
pub fn decode_notice<'a>(method: &[u8], arguments: &'a [u8]) -> Result<Option<CallNotice<'a>>, CodecError> {
    let Some(kind)=notice_kind(method)else{return Ok(None);};
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
    encode_operation(CallOperation::Accept,call_id,properties)
}
pub fn encode_operation(_operation:CallOperation,call_id:NativeCallId,properties:&[RpcProperty<'_>])->Result<RpcBytes,CodecError>{
    let mut writer=Writer::versioned()?;writer.u64(call_id.0)?;writer.properties(properties)?;Ok(writer.finish())
}
/// Invited/Released native callbacks return bool+Common footer. Accepted has
/// one additional result property map; these formats must not be conflated.
pub fn encode_notice_result(kind:CallNoticeKind,accepted:bool,properties:&[RpcProperty<'_>])->Result<RpcBytes,CodecError>{
    if kind!=CallNoticeKind::Accepted && !properties.is_empty(){return Err(CodecError::InvalidField);}
    let mut writer=Writer::new();writer.boolean(accepted)?;
    if kind==CallNoticeKind::Accepted{writer.properties(properties)?;}
    writer.bytes(b"")?;Ok(writer.finish())
}

pub struct OperationResult<'a>{pub accepted:bool,pub properties:Vec<RpcProperty<'a>>,pub footer:RpcFooter<'a>}
impl fmt::Debug for OperationResult<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OperationResult(<redacted>)")}}
/// Semantic rejection is distinct from malformed transport and SDK login.
/// The original shared ARC pending allocator must establish provenance first.
pub fn decode_operation_result(operation:CallOperation,result_code:u32,bytes:&[u8])->Result<OperationResult<'_>,CodecError>{
    if result_code!=0{return Err(CodecError::RemoteFailure);}
    let mut reader=Reader::new(bytes)?;let value=reader.boolean()?;
    let properties=if operation==CallOperation::Accept{reader.properties()?}else{Vec::new()};
    let footer=read_footer(&mut reader)?;reader.finish()?;
    Ok(OperationResult{accepted:value&&footer.reason.is_empty(),properties,footer})
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
