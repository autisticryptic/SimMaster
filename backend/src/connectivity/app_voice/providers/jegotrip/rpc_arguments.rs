//! RPC argument format verified from the selected native SDK in read-only IDA.
//!
//! This is only the versioned argument payload passed to ObjectAgent, not a
//! network frame or SIP message. ARC session/batch framing remains unimplemented.
use super::session::{GatewayRegistration, STATUS_REGISTER, STATUS_UNREGISTER};
use std::fmt;

pub const UPDATE_STATUS_METHOD: &str = "updateStatus.SipGatewayPub.SipGateway";
pub const DEFAULT_GATEWAY_OBJECT: &str = "#SipGateway";
const MAX_ARGUMENT_BYTES: usize = 128 * 1024;

/// A payload contains credentials: no Serialize/Display or byte-dumping Debug.
pub struct RpcArguments(Vec<u8>);

impl RpcArguments {
    pub fn expose_to_transport(&self) -> &[u8] { &self.0 }
}

impl fmt::Debug for RpcArguments {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcArguments").field("byte_length", &self.0.len())
            .field("data", &"<redacted>").finish()
    }
}

fn put_string(out: &mut Vec<u8>, value: &str) -> Result<(), &'static str> {
    let bytes = value.as_bytes();
    if bytes.contains(&0) { return Err("jegotrip_rpc_string_invalid"); }
    let length = u32::try_from(bytes.len()).map_err(|_| "jegotrip_rpc_arguments_too_large")?;
    let next = out.len().checked_add(4).and_then(|size| size.checked_add(bytes.len()))
        .ok_or("jegotrip_rpc_arguments_too_large")?;
    if next > MAX_ARGUMENT_BYTES { return Err("jegotrip_rpc_arguments_too_large"); }
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(())
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
    let mut fields = [
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
    fields.sort_by(|(left, _), (right, _)| left.len().cmp(&right.len())
        .then_with(|| left.as_bytes().cmp(right.as_bytes())));
    // Bounds/NUL rejection are local adapter policies, not inferred remote
    // protocol restrictions. Never serialize an unvalidated empty credential.
    let mut out = Vec::new();
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    put_string(&mut out, current_user_uri)?;
    out.extend_from_slice(&(fields.len() as u32).to_be_bytes());
    for (key, value) in fields {
        put_string(&mut out, key)?;
        put_string(&mut out, value)?;
    }
    Ok(RpcArguments(out))
}

#[cfg(test)]
#[path = "rpc_arguments_tests.rs"]
mod tests;
