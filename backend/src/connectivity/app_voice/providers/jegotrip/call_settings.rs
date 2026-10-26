//! Pure, explicitly imported basic inbound-call/media settings. No construction
//! or configuration persistence starts a SDK/media/trunk socket or grants ready.
use super::{media_endpoint::{MediaEndpointProfile, MediaEndpointProfileInputs}, runtime_connector::{RuntimeConnector, TcpRuntimeConnector},
    runtime_provision::{AccountTrust, AccountTrustInputs}, legacy_config::ConfigDecodeKey, rpc_stream::RpcProperty};
use crate::platform::config::{TrunkIncomingMode, TrunkProfileConfig};
use std::{fmt, net::IpAddr, sync::Arc, time::Duration};
pub(super) struct CallSettings {
    pub profile: Arc<MediaEndpointProfile>, pub connector: Arc<dyn RuntimeConnector>, pub bind_ip: IpAddr, pub trunk: Option<TrunkProfileConfig>,
}
impl fmt::Debug for CallSettings { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("CallSettings(<redacted>)") } }
impl Drop for CallSettings { fn drop(&mut self) { use zeroize::Zeroize; if let Some(trunk) = &mut self.trunk { trunk.secret.zeroize(); } } }
pub(super) struct Inputs<'a> {
    pub authority: &'a str, pub root: &'a str, pub enable_sni: bool, pub routing: &'a [RpcProperty<'a>],
    pub features: &'a [&'a [u8]], pub properties: &'a [RpcProperty<'a>], pub key: Option<ConfigDecodeKey>,
    pub connect_timeout: Duration, pub handshake_timeout: Duration, pub partial_timeout: Duration, pub io_timeout: Duration, pub rpc_timeout: Duration,
    pub bind_ip: IpAddr, pub allow_plain_media: bool, pub trunk: Option<&'a TrunkProfileConfig>,
}
impl CallSettings {
    pub(super) fn supplied(input: Inputs<'_>) -> Result<Self, &'static str> {
        const INVALID: &str = "app_account_call_configuration_invalid";
        if !input.allow_plain_media || input.bind_ip.is_unspecified() || input.bind_ip.is_multicast() { return Err(INVALID); }
        if let Some(trunk) = input.trunk {
            if trunk.vowifi_only || trunk.allow_home_cellular_calls || trunk.incoming_mode == TrunkIncomingMode::BoundImmediate {
                return Err(INVALID); // not a SIM/IMS access/cost or early-answer gate
            }
            if trunk.enabled {
                crate::services::trunk::driver::validate_profile(trunk).map_err(|_| INVALID)?;
                for value in [&trunk.asterisk_host, &trunk.username, &trunk.incoming_binding, &trunk.outgoing_binding] {
                    if value.len() > 1024 || value.chars().any(char::is_control) { return Err(INVALID); }
                }
                if trunk.incoming_mode == TrunkIncomingMode::BoundPending && trunk.incoming_binding.trim().is_empty() { return Err(INVALID); }
            }
        }
        let trust = Arc::new(AccountTrust::supplied(AccountTrustInputs { authority: input.authority, root_der_base64: input.root, enable_sni: input.enable_sni }).map_err(|_| INVALID)?);
        let profile = Arc::new(MediaEndpointProfile::supplied(MediaEndpointProfileInputs { trust, routing: input.routing, features: input.features,
            properties: input.properties, reference_decode_key: input.key, handshake_timeout: input.handshake_timeout, partial_timeout: input.partial_timeout,
            io_timeout: input.io_timeout, rpc_timeout: input.rpc_timeout }).map_err(|_| INVALID)?);
        let connector = Arc::new(TcpRuntimeConnector::new(input.connect_timeout).map_err(|_| INVALID)?);
        Ok(Self { profile, connector, bind_ip: input.bind_ip, trunk: input.trunk.cloned() })
    }
}
