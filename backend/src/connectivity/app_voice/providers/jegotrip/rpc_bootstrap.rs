//! Recovered getAppConfigs bootstrap arguments/results, not endpoint discovery.
//!
//! Returned object references and deployment properties remain private bytes.
//! Decoding them neither resolves a host nor changes TLS trust or readiness.
use super::rpc_stream::{read_footer, CodecError, Reader, RpcBytes, RpcFooter, RpcProperty, Writer};
use std::fmt;

pub const GET_APP_CONFIGS_METHOD: &str = "getAppConfigs.AccountEntry.Account";

pub struct AppConfigsResult<'a> {
    pub properties: Vec<RpcProperty<'a>>,
    pub account_entry: &'a [u8],
    pub account_alive: &'a [u8],
    pub user_entry: &'a [u8],
    pub footer: RpcFooter<'a>,
}

impl fmt::Debug for AppConfigsResult<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AppConfigsResult(<redacted>)")
    }
}

pub fn encode_get_app_configs(application_key: &[u8]) -> Result<RpcBytes, CodecError> {
    if application_key.is_empty() { return Err(CodecError::InvalidField); }
    let mut writer = Writer::versioned()?;
    writer.bytes(application_key)?;
    Ok(writer.finish())
}

/// Native order: bool, property map, AccountEntry, AccountAlive, UserEntry,
/// mandatory terminal reason, optional footer properties. Only AccountEntry
/// must be nonempty at this stage; the other channels may still be unavailable.
pub fn decode_get_app_configs_result(result_code: u32, bytes: &[u8])
    -> Result<AppConfigsResult<'_>, CodecError> {
    if result_code != 0 { return Err(CodecError::RemoteFailure); }
    let mut reader = Reader::new(bytes)?;
    let success = reader.boolean()?;
    let properties = reader.properties()?;
    let account_entry = reader.bytes()?;
    let account_alive = reader.bytes()?;
    let user_entry = reader.bytes()?;
    let footer = read_footer(&mut reader)?;
    reader.finish()?;
    if !success || !footer.reason.is_empty() { return Err(CodecError::RemoteFailure); }
    if account_entry.is_empty() { return Err(CodecError::InvalidField); }
    Ok(AppConfigsResult { properties, account_entry, account_alive, user_entry, footer })
}

#[cfg(test)]
#[path = "rpc_bootstrap_tests.rs"]
mod tests;
