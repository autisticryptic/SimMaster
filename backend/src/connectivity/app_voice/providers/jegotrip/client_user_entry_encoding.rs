//! Measured, no-growth encoders for the new Client READ path only. The original
//! Writer, canonical encoders, version list, signed maps and refusals remain.
//! Reservations precede these constructors; no new budget or transport grant.
use super::{rpc_batch::{BatchPayload, SWAP_CALLS_METHOD},
    rpc_stream::{CodecError, RpcBytes, RpcProperty, Writer, MAX_FIELD_BYTES, MAX_MAP_ENTRIES},
    user_entry_data::{Aid, MAX_AIDS}};

pub(super) fn aid_arguments_bytes(aid: Aid<'_>) -> Result<usize, CodecError> {
    Aid::original(aid.kind(), aid.value())?;
    // type0 + one version-list entry + count + two STRING length words.
    17usize.checked_add(aid.kind().bytes().len()).and_then(|n| n.checked_add(aid.value().len())).ok_or(CodecError::TooLarge)
}
pub(super) fn encode_aid_arguments(aid: Aid<'_>) -> Result<RpcBytes, CodecError> {
    let mut writer = Writer::versioned_measured(aid_arguments_bytes(aid)?)?;
    writer.u32(1)?; writer.bytes(aid.kind().bytes())?; writer.bytes(aid.value())?;
    Ok(writer.finish())
}
pub(super) fn property_bytes(fields: &[RpcProperty<'_>]) -> Result<usize, CodecError> {
    if fields.len() > MAX_MAP_ENTRIES { return Err(CodecError::TooLarge); }
    fields.iter().try_fold(0usize, |total, field| {
        if field.key.len() > MAX_FIELD_BYTES || field.value.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
        total.checked_add(8).and_then(|n| n.checked_add(field.key.len())).and_then(|n| n.checked_add(field.value.len())).ok_or(CodecError::TooLarge)
    })
}
pub(super) fn request_bytes(fields: &[RpcProperty<'_>], method: &[u8], arguments: &[u8]) -> Result<usize, CodecError> {
    if method.is_empty() { return Err(CodecError::InvalidField); }
    if method.len() > MAX_FIELD_BYTES || arguments.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
    // type0 + callid + map count + method STRING + arguments STRING.
    17usize.checked_add(property_bytes(fields)?).and_then(|n| n.checked_add(method.len())).and_then(|n| n.checked_add(arguments.len())).ok_or(CodecError::TooLarge)
}
pub(super) fn encode_request(call_id: i32, fields: &[RpcProperty<'_>], method: &[u8], arguments: &[u8]) -> Result<RpcBytes, CodecError> {
    if call_id <= 0 { return Err(CodecError::InvalidCallId); }
    let mut writer = Writer::measured(request_bytes(fields, method, arguments)?)?;
    writer.i32(call_id)?; writer.properties(fields)?; writer.bytes(method)?; writer.bytes(arguments)?;
    Ok(writer.finish())
}
pub(super) fn swap_arguments_bytes(uid: &[u8], selector: &[u8], payload: &BatchPayload<'_>) -> Result<usize, CodecError> {
    if uid.is_empty() || selector.is_empty() { return Err(CodecError::InvalidField); }
    if uid.len() > MAX_FIELD_BYTES || selector.len() > MAX_FIELD_BYTES || payload.requests.len() > MAX_AIDS || payload.results.len() > MAX_MAP_ENTRIES { return Err(CodecError::TooLarge); }
    let mut bytes = 45usize.checked_add(uid.len()).and_then(|n| n.checked_add(selector.len())).ok_or(CodecError::TooLarge)?;
    for request in payload.requests.values() {
        if request.method.is_empty() { return Err(CodecError::InvalidField); }
        if request.method.len() > MAX_FIELD_BYTES || request.arguments.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
        bytes = bytes.checked_add(16).and_then(|n| n.checked_add(request.method.len())).and_then(|n| n.checked_add(request.arguments.len())).and_then(|n| n.checked_add(property_bytes(&request.metadata).ok()?)).ok_or(CodecError::TooLarge)?;
    }
    for result in payload.results.values() {
        if result.result_stream.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
        bytes = bytes.checked_add(12).and_then(|n| n.checked_add(result.result_stream.len())).ok_or(CodecError::TooLarge)?;
    }
    Ok(bytes)
}
pub(super) fn encode_swap_calls(uid: &[u8], selector: &[u8], payload: &BatchPayload<'_>) -> Result<RpcBytes, CodecError> {
    let mut writer = Writer::versioned_measured(swap_arguments_bytes(uid, selector, payload)?)?;
    writer.bytes(uid)?; writer.bytes(selector)?;
    for word in payload.fields { writer.u32(word)?; }
    writer.u32(payload.requests.len() as u32)?;
    for (&sequence, request) in &payload.requests {
        writer.i32(sequence)?; writer.bytes(request.method)?; writer.bytes(request.arguments)?; writer.properties(&request.metadata)?;
    }
    writer.u32(payload.results.len() as u32)?;
    for (&sequence, result) in &payload.results {
        writer.i32(sequence)?; writer.u32(result.result_code)?; writer.bytes(result.result_stream)?;
    }
    Ok(writer.finish())
}
pub(super) fn swap_method() -> &'static [u8] { SWAP_CALLS_METHOD.as_bytes() }
