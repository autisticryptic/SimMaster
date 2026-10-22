//! Recovered Common RPC envelopes, after authenticated transport processing.
//!
//! These are not TCP/TLS frames. Object routing, __magic deduplication, pending
//! operation ownership and authorization belong to the backend, not the codec.
use super::rpc_stream::{CodecError, Reader, RpcBytes, RpcProperty, Writer};
use std::fmt;

pub struct RpcRequest<'a> {
    pub call_id: i32,
    pub metadata: Vec<RpcProperty<'a>>,
    pub method: &'a [u8],
    pub arguments: &'a [u8],
}

impl fmt::Debug for RpcRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcRequest").field("call_id", &self.call_id)
            .field("metadata_entries", &self.metadata.len()).field("data", &"<redacted>").finish()
    }
}

pub struct RpcReply<'a> {
    pub call_id: i32,
    pub result_code: u32,
    pub result_stream: &'a [u8],
}

impl fmt::Debug for RpcReply<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcReply").field("call_id", &self.call_id)
            .field("result_code", &self.result_code).field("data", &"<redacted>").finish()
    }
}

fn require_call_id(id: i32) -> Result<(), CodecError> {
    if id <= 0 { Err(CodecError::InvalidCallId) } else { Ok(()) }
}

pub fn encode_request(call_id: i32, metadata: &[RpcProperty<'_>], method: &[u8],
    arguments: &[u8]) -> Result<RpcBytes, CodecError> {
    require_call_id(call_id)?;
    if method.is_empty() { return Err(CodecError::InvalidField); }
    let mut writer = Writer::new();
    writer.i32(call_id)?;
    writer.properties(metadata)?;
    writer.bytes(method)?;
    writer.bytes(arguments)?;
    Ok(writer.finish())
}

pub fn decode_request(bytes: &[u8]) -> Result<RpcRequest<'_>, CodecError> {
    let mut reader = Reader::new(bytes)?;
    let call_id = reader.i32()?;
    require_call_id(call_id)?;
    let metadata = reader.properties()?;
    let method = reader.bytes()?;
    if method.is_empty() { return Err(CodecError::InvalidField); }
    let arguments = reader.bytes()?;
    reader.finish()?;
    Ok(RpcRequest { call_id, metadata, method, arguments })
}

/// Nonzero result codes are preserved; callers must never treat them as success.
pub fn decode_reply(bytes: &[u8]) -> Result<RpcReply<'_>, CodecError> {
    let mut reader = Reader::new(bytes)?;
    let call_id = reader.i32()?;
    require_call_id(call_id)?;
    let result_code = reader.u32()?;
    let result_stream = reader.bytes()?;
    reader.finish()?;
    Ok(RpcReply { call_id, result_code, result_stream })
}

#[cfg(test)]
#[path = "rpc_envelope_tests.rs"]
mod tests;
