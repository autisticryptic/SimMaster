//! Recovered swapCalls.AccountEntry.Account argument and result streams.
//!
//! The six counter/ack words stay opaque and in native order. This codec does not
//! allocate/reset batch epochs or infer token lifetime or nested request success.
//! No TLS/network frame is produced and no provider becomes available.
use super::rpc_stream::{read_footer, CodecError, Reader, RpcBytes, RpcFooter, RpcProperty, Writer, MAX_MAP_ENTRIES};
use std::{collections::BTreeMap, fmt};

pub const SWAP_CALLS_METHOD: &str = "swapCalls.AccountEntry.Account";

pub struct BatchRequest<'a> {
    pub method: &'a [u8],
    pub arguments: &'a [u8],
    pub metadata: Vec<RpcProperty<'a>>,
}

impl fmt::Debug for BatchRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchRequest").field("metadata_entries", &self.metadata.len())
            .field("data", &"<redacted>").finish()
    }
}

pub struct BatchResult<'a> {
    pub result_code: u32,
    pub result_stream: &'a [u8],
}

impl fmt::Debug for BatchResult<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchResult").field("result_code", &self.result_code)
            .field("data", &"<redacted>").finish()
    }
}

pub struct BatchPayload<'a> {
    pub fields: [u32; 6],
    pub requests: BTreeMap<i32, BatchRequest<'a>>,
    pub results: BTreeMap<i32, BatchResult<'a>>,
}

impl fmt::Debug for BatchPayload<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchPayload").field("request_count", &self.requests.len())
            .field("result_count", &self.results.len()).field("data", &"<redacted>").finish()
    }
}

pub struct BatchReply<'a> {
    pub success: bool,
    pub payload: BatchPayload<'a>,
    pub properties: Vec<RpcProperty<'a>>,
    pub footer: RpcFooter<'a>,
}

impl fmt::Debug for BatchReply<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchReply").field("success", &self.success)
            .field("payload", &self.payload).finish_non_exhaustive()
    }
}

/// `user_id` comes from a successful loginSession reply. The second argument is
/// the originally selected SDK session id (Mtc_CliApplySessId), NOT the returned
/// ARC auth token. The returned token belongs in the Common RPC metadata `token`.
/// The byte layout was correct before this distinction was recovered, but naming
/// the second string `session_token` would cause a real backend to send the wrong
/// value. No default deployment/session id is invented here.
pub fn encode_swap_calls(user_id: &[u8], selected_session_id: &[u8], payload: &BatchPayload<'_>)
    -> Result<RpcBytes, CodecError> {
    if user_id.is_empty() || selected_session_id.is_empty() { return Err(CodecError::InvalidField); }
    if payload.requests.len() > MAX_MAP_ENTRIES || payload.results.len() > MAX_MAP_ENTRIES {
        return Err(CodecError::TooLarge);
    }
    let mut writer = Writer::versioned()?;
    writer.bytes(user_id)?;
    writer.bytes(selected_session_id)?;
    for field in payload.fields { writer.u32(field)?; }
    writer.u32(payload.requests.len() as u32)?;
    // The native request/result map comparator is signed int32, not u32.
    for (&sequence, request) in &payload.requests {
        if request.method.is_empty() { return Err(CodecError::InvalidField); }
        writer.i32(sequence)?;
        writer.bytes(request.method)?;
        writer.bytes(request.arguments)?;
        writer.properties(&request.metadata)?;
    }
    writer.u32(payload.results.len() as u32)?;
    for (&sequence, result) in &payload.results {
        writer.i32(sequence)?;
        writer.u32(result.result_code)?;
        writer.bytes(result.result_stream)?;
    }
    Ok(writer.finish())
}

pub(super) fn read_payload<'a>(reader: &mut Reader<'a>) -> Result<BatchPayload<'a>, CodecError> {
    let mut fields = [0; 6];
    for field in &mut fields { *field = reader.u32()?; }
    let request_count = reader.count(16)?;
    let mut requests = BTreeMap::new();
    for _ in 0..request_count {
        let sequence = reader.i32()?;
        let method = reader.bytes()?;
        if method.is_empty() { return Err(CodecError::InvalidField); }
        let arguments = reader.bytes()?;
        let metadata = reader.properties()?;
        if requests.insert(sequence, BatchRequest { method, arguments, metadata }).is_some() {
            return Err(CodecError::DuplicateKey);
        }
    }
    let result_count = reader.count(12)?;
    let mut results = BTreeMap::new();
    for _ in 0..result_count {
        let sequence = reader.i32()?;
        let result_code = reader.u32()?;
        let result_stream = reader.bytes()?;
        if results.insert(sequence, BatchResult { result_code, result_stream }).is_some() {
            return Err(CodecError::DuplicateKey);
        }
    }
    Ok(BatchPayload { fields, requests, results })
}

/// `result_code` is the already-demultiplexed Common RPC result. A false outer
/// bool is preserved and must block all nested delivery; no automatic retry.
pub fn decode_swap_calls_reply(result_code: u32, bytes: &[u8]) -> Result<BatchReply<'_>, CodecError> {
    if result_code != 0 { return Err(CodecError::RemoteFailure); }
    let mut reader = Reader::new(bytes)?;
    let success = reader.boolean()?;
    let payload = read_payload(&mut reader)?;
    let properties = reader.properties()?;
    let footer = read_footer(&mut reader)?;
    reader.finish()?;
    if !footer.reason.is_empty() { return Err(CodecError::RemoteFailure); }
    Ok(BatchReply { success, payload, properties, footer })
}

#[cfg(test)]
#[path = "rpc_batch_tests.rs"]
mod tests;
