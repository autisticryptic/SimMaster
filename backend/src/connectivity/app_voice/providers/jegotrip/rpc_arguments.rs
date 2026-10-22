//! RPC argument format verified from the selected native SDK in read-only IDA.
//!
//! This is only the versioned argument payload passed to ObjectAgent, not a
//! network frame or SIP message. ARC connection/TLS framing remains unimplemented.
use super::{rpc_stream::{read_footer, CodecError, Reader, RpcFooter, RpcProperty, Writer},
    session::{GatewayRegistration, STATUS_REGISTER, STATUS_UNREGISTER}};

pub const UPDATE_STATUS_METHOD: &str = "updateStatus.SipGatewayPub.SipGateway";
pub const DEFAULT_GATEWAY_OBJECT: &str = "#SipGateway";
pub use super::rpc_stream::{RpcBytes as RpcArguments, MAX_STREAM_BYTES as MAX_ARGUMENT_BYTES};

fn validate_string(value: &str) -> Result<(), &'static str> {
    if value.as_bytes().contains(&0) { Err("jegotrip_rpc_string_invalid") } else { Ok(()) }
}

/// `current_user_uri` is the SDK's actual Mtc_UeGetUri, not a guessed sip: URI.
/// String map order is length first, then byte order for equal lengths, matching
/// Common::String::operator< in the reviewed library (not Rust lexical BTreeMap).
pub fn encode_update_status(current_user_uri: &str, status: i32,
    credentials: &GatewayRegistration<'_>) -> Result<RpcArguments, &'static str> {
    if current_user_uri.is_empty() { return Err("jegotrip_sdk_uri_required"); }
    if !matches!(status, STATUS_REGISTER | STATUS_UNREGISTER) {
        return Err("jegotrip_hosting_status_unsupported");
    }
    let status = status.to_string();
    let fields = [
        ("SipFlag", status.as_str()),
        ("AuthnamePrefix", credentials.authname_prefix),
        ("ChannelId", credentials.channel_id),
        ("DeleteIntCode", credentials.delete_int_code),
        ("SipNumber", credentials.sip_number),
        ("SipPwd", credentials.sip_password),
    ];
    if fields.iter().any(|(_, value)| value.is_empty()) {
        return Err("jegotrip_rpc_credential_missing");
    }
    validate_string(current_user_uri)?;
    for (_, value) in &fields { validate_string(value)?; }
    let fields: Vec<_> = fields.iter().map(|(key, value)| RpcProperty {
        key: key.as_bytes(), value: value.as_bytes(),
    }).collect();
    let mut writer = Writer::versioned().map_err(CodecError::code)?;
    writer.bytes(current_user_uri.as_bytes()).map_err(CodecError::code)?;
    writer.properties(&fields).map_err(CodecError::code)?;
    Ok(writer.finish())
}

/// The result is already correlated to updateStatus, not merely a matching
/// notification name. Native success requires RPC result zero AND bool true.
/// It is still only a gateway acknowledgement, not channel readiness.
pub fn decode_update_status_result(result_code: u32, bytes: &[u8])
    -> Result<(bool, RpcFooter<'_>), CodecError> {
    if result_code != 0 { return Err(CodecError::RemoteFailure); }
    let mut reader = Reader::new(bytes)?;
    let success = reader.boolean()?;
    let footer = read_footer(&mut reader)?;
    reader.finish()?;
    if !footer.reason.is_empty() { return Err(CodecError::RemoteFailure); }
    Ok((success, footer))
}

#[cfg(test)]
#[path = "rpc_arguments_tests.rs"]
mod tests;
