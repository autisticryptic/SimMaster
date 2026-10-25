//! Recovered connect.Endpoint.Endpoint argument/result DATA only.
//!
//! Content must be the original Accept result's `content` property, supplied by
//! the caller under its original pending-call scope. This opaque, potentially
//! sensitive media value is not the phone login credential or an assumed URI. Features and properties remain opaque binary data:
//! neither encoding nor decoding grants socket access, TLS trust or media-ready
//! state, including when returned properties name Client.Host or ArcPort.
use super::rpc_stream::{
    compare_keys, read_footer, CodecError, Reader, RpcBytes, RpcFooter, RpcProperty,
    Writer, MAX_FIELD_BYTES, MAX_MAP_ENTRIES,
};
use std::fmt;

pub const CONNECT_ENDPOINT_METHOD: &str = "connect.Endpoint.Endpoint";

pub struct EndpointConnectInputs<'a> {
    pub content: &'a [u8],
    pub features: &'a [&'a [u8]],
    pub properties: &'a [RpcProperty<'a>],
}

impl fmt::Debug for EndpointConnectInputs<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EndpointConnectInputs(<redacted>)")
    }
}

/// Native request: StreamData0, BE16 version-count=1/version=0, content
/// string, BE32-counted set<string>, property map. The set uses Common String's
/// length-first comparator, not lexical-only ordering or a configuration map.
/// Nonempty strings and duplicate rejection are local policies; no features
/// are invented and no roots, authentication material or endpoints extracted.
/// Owned output uses the shared Writer/RpcBytes lifetime conventions.
pub fn encode_endpoint_connect(inputs: EndpointConnectInputs<'_>) -> Result<RpcBytes, CodecError> {
    if inputs.content.is_empty() { return Err(CodecError::InvalidField); }
    if inputs.content.len() > MAX_FIELD_BYTES || inputs.features.len() > MAX_MAP_ENTRIES {
        return Err(CodecError::TooLarge);
    }
    for feature in inputs.features {
        if feature.is_empty() { return Err(CodecError::InvalidField); }
        if feature.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
    }
    let mut features = inputs.features.to_vec();
    features.sort_by(|left, right| compare_keys(left, right));
    if features.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(CodecError::DuplicateKey);
    }

    let mut writer = Writer::versioned()?;
    writer.bytes(inputs.content)?;
    writer.u32(features.len() as u32)?;
    for feature in features { writer.bytes(feature)?; }
    writer.properties(inputs.properties)?;
    Ok(writer.finish())
}

pub struct EndpointConnectResult<'a> {
    pub features: Vec<&'a [u8]>,
    pub properties: Vec<RpcProperty<'a>>,
    pub footer: RpcFooter<'a>,
}

impl fmt::Debug for EndpointConnectResult<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EndpointConnectResult(<redacted>)")
    }
}

/// Native result has no request-version list: bool, set<string>, property map,
/// mandatory Common reason string and optional footer map. External RPC code
/// belongs to the envelope and must already be correlated by the caller.
/// Native set insertion would collapse duplicates; this codec deliberately
/// rejects duplicate/colliding values rather than simplifying malformed data.
/// Success here is only parsed RPC DATA, not an authorized media endpoint.
pub fn decode_endpoint_connect_result(result_code: u32, bytes: &[u8])
    -> Result<EndpointConnectResult<'_>, CodecError> {
    if result_code != 0 { return Err(CodecError::RemoteFailure); }
    let mut reader = Reader::new(bytes)?;
    let success = reader.boolean()?;
    let count = reader.count(4)?;
    let mut features: Vec<&[u8]> = Vec::with_capacity(count);
    for _ in 0..count {
        let feature = reader.bytes()?;
        if feature.is_empty() { return Err(CodecError::InvalidField); }
        if features.iter().any(|previous| *previous == feature) {
            return Err(CodecError::DuplicateKey);
        }
        features.push(feature);
    }
    let properties = reader.properties()?;
    let footer = read_footer(&mut reader)?;
    reader.finish()?;
    if !success || !footer.reason.is_empty() { return Err(CodecError::RemoteFailure); }
    Ok(EndpointConnectResult { features, properties, footer })
}

#[cfg(test)]
#[path = "rpc_endpoint_tests.rs"]
mod tests;
