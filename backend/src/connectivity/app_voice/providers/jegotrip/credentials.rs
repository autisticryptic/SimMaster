//! User-supplied credentials; account-login UI and server authorization differ.
use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroize;

/// Never formatted as cleartext, even in validation diagnostics.
#[derive(Clone, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Internal material obtained through the owned, authenticated App flow.
    /// This constructor grants no SDK/session/readiness authority.
    pub(super) fn from_owned(value: String) -> Self { Self(value) }
    pub fn expose_to_backend(&self) -> &str { &self.0 }
}

impl Drop for Secret { fn drop(&mut self) { self.0.zeroize(); } }

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("<redacted>") }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkCredentials {
    pub account_identity: Secret,
    /// The app passes getPhoneToken()/CallUserInfo.cstToken, NOT SipPwd here.
    pub phone_login_token: Secret,
    pub application_key: Secret,
    pub server_address: Secret,
    /// Preserve the configured device identity; do not invent a new one.
    pub device_id: Option<Secret>,
    /// Originally selected Mtc_CliApplySessId input, not an auth token. The
    /// credential import stays backward-compatible; explicit native runtime
    /// validation requires this selector and never invents a default.
    pub selected_session_id: Option<Secret>,
    pub root_der_base64: Secret,
    /// Legacy configuration unwrap only. Client-property and reference keys
    /// are independent optional imports, never pwd/token/SipPwd substitutes.
    pub client_config_decode_key: Option<Secret>,
    pub reference_decode_key: Option<Secret>,
    pub tls_only: bool,
    /// Unknown expiry stays unknown; it must not be reported as permanent.
    pub expires_at_unix_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostingCredentials {
    pub sip_number: Secret,
    /// Decoded gateway password, not the cached encrypted sipPassword string.
    pub sip_password: Secret,
    pub authname_prefix: Secret,
    pub channel_id: Secret,
    /// The inspected app supplies this policy as a string, not a JSON boolean.
    pub delete_int_code: String,
    pub enrollment_confirmed: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedCredentials {
    pub sdk: SdkCredentials,
    pub hosting: HostingCredentials,
}

impl ImportedCredentials {
    /// Errors are stable codes only; input values are never put into errors.
    pub fn validate(&self, now_unix_seconds: u64) -> Result<(), &'static str> {
        for secret in [
            &self.sdk.account_identity, &self.sdk.phone_login_token,
            &self.sdk.application_key, &self.sdk.server_address,
            &self.sdk.root_der_base64, &self.hosting.sip_number,
            &self.hosting.sip_password, &self.hosting.authname_prefix,
            &self.hosting.channel_id,
        ] {
            let value = secret.expose_to_backend();
            if value.is_empty() || value.len() > 16_384 || value.chars().any(char::is_control) {
                return Err("jegotrip_credential_invalid");
            }
        }
        if self.sdk.device_id.as_ref().is_some_and(|id| {
            id.0.is_empty() || id.0.len() > 1024 || id.0.chars().any(char::is_control)
        }) { return Err("jegotrip_device_identity_invalid"); }
        if self.sdk.selected_session_id.as_ref().is_some_and(|id| {
            id.0.is_empty() || id.0.len() > 127 || id.0.chars().any(char::is_control)
        }) { return Err("jegotrip_session_identity_invalid"); }
        if self.hosting.delete_int_code.is_empty()
            || self.hosting.delete_int_code.len() > 32
            || self.hosting.delete_int_code.chars().any(char::is_control)
        { return Err("jegotrip_gateway_policy_invalid"); }
        for key in [&self.sdk.client_config_decode_key, &self.sdk.reference_decode_key].into_iter().flatten() {
            let value = key.expose_to_backend();
            if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
                return Err("jegotrip_config_decode_key_invalid");
            }
        }
        if !self.sdk.tls_only { return Err("jegotrip_tls_required"); }
        if !self.hosting.enrollment_confirmed { return Err("jegotrip_hosting_enrollment_required"); }
        if self.sdk.expires_at_unix_seconds.is_some_and(|at| at <= now_unix_seconds) {
            return Err("jegotrip_credential_expired");
        }
        Ok(())
    }
}

/// Static implementation contract, never current account readiness or consent.
#[derive(Debug, Serialize)]
pub struct CredentialContract {
    pub provider_id: &'static str,
    pub account_login_ui_required: bool,
    pub service_credentials_required: bool,
    pub transport_implemented: bool,
    pub basic_hosting: BasicHostingContract,
    pub direct_sip_interoperability: &'static str,
    pub renewal: &'static str,
    pub verified_sdk_call_path: &'static str,
    pub required_parameters: Vec<ParameterContract>,
}

#[derive(Debug, Serialize)]
pub struct BasicHostingContract {
    pub account_line_kind: &'static str,
    pub access: &'static str,
    pub billing_account: &'static str,
    pub explicit_profile_required: bool,
    pub manual_start_required: bool,
    pub automatic_cleanup_supported: bool,
    pub automatic_reconnect_implemented: bool,
    pub implemented_operations: Vec<&'static str>,
    pub media_transport: &'static str,
    pub media_profile: &'static str,
    pub plain_media_opt_in_required: bool,
    pub outbound_call_implemented: bool,
    pub outbound_message_implemented: bool,
    pub default_sdk_client_implemented: bool,
    pub readiness_source: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ParameterContract {
    pub field: &'static str,
    pub source: &'static str,
    pub purpose: &'static str,
    pub secret: bool,
}

pub fn contract() -> CredentialContract {
    CredentialContract {
        provider_id: super::PROVIDER_ID,
        account_login_ui_required: false,
        service_credentials_required: true,
        transport_implemented: true,
        basic_hosting: BasicHostingContract {
            account_line_kind: "app_account", access: "application_data", billing_account: "jegotrip_account",
            explicit_profile_required: true, manual_start_required: true,
            automatic_cleanup_supported: true, automatic_reconnect_implemented: true,
            implemented_operations: super::BASIC_INBOUND_OPERATIONS.iter().map(|operation| operation.as_str()).collect(),
            media_transport: "endpoint_tcp_mpath",
            media_profile: "raw_or_zlib_base64_rse_sdp_rtp_avp_rtcp_mux_passthrough",
            plain_media_opt_in_required: true,
            outbound_call_implemented: false, outbound_message_implemented: false,
            default_sdk_client_implemented: false,
            readiness_source: "current_original_run_and_per_capability_proof",
        },
        direct_sip_interoperability: "unverified",
        renewal: "manual_reimport_until_renewal_verified",
        verified_sdk_call_path: "JCClient.login -> JCAccount.updateStatus(4)",
        required_parameters: vec![
            ParameterContract { field: "sdk.account_identity", source: "getAreaCodeMobileNum", purpose: "SDK session identity", secret: true },
            ParameterContract { field: "sdk.phone_login_token", source: "wyx_phone_login_token / wyx_phone_login_token_key; CallUserInfo.cstToken", purpose: "JCClient.login password argument", secret: true },
            ParameterContract { field: "sdk.application_key", source: "JCClient.create configuration", purpose: "SDK application", secret: true },
            ParameterContract { field: "sdk.server_address", source: "config_key_server_address", purpose: "SDK server, not an inferred SIP SBC", secret: true },
            ParameterContract { field: "sdk.device_id (optional)", source: "config_key_device_id", purpose: "Preserve the original device binding if configured", secret: true },
            ParameterContract { field: "sdk.selected_session_id (optional)", source: "Mtc_CliApplySessId / verified SDK session configuration", purpose: "Login/swap method session selector, distinct from phone token and returned ARC metadata token; no invented default", secret: true },
            ParameterContract { field: "sdk.expires_at_unix_seconds (optional)", source: "Verified token expiry, if known", purpose: "Unknown expiry must remain unknown", secret: false },
            ParameterContract { field: "sdk.root_der_base64", source: "config_key_root_der", purpose: "Deployment TLS trust", secret: true },
            ParameterContract { field: "sdk.client_config_decode_key (optional)", source: "Original Client PBE configuration context", purpose: "Unwrap Client property ENC values only; no built-in default, not account authentication", secret: true },
            ParameterContract { field: "sdk.reference_decode_key (optional)", source: "Original Common reference PBE context", purpose: "Unwrap reference ENC values only; distinct from Client key/pwd/token/SipPwd and no trust proof", secret: true },
            ParameterContract { field: "sdk.tls_only", source: "config_key_use_tls_only", purpose: "TLS required", secret: false },
            ParameterContract { field: "hosting.sip_number", source: "SipNumber / getAreaCodeMobileNum", purpose: "Hosted number", secret: true },
            ParameterContract { field: "hosting.sip_password", source: "voip_status.sipPassword after decryptThreeDESECB; legacy callAccountSipPassword requires format check", purpose: "SipPwd gateway credential, distinct from SDK token", secret: true },
            ParameterContract { field: "hosting.authname_prefix", source: "AuthnamePrefix", purpose: "Gateway deployment policy", secret: true },
            ParameterContract { field: "hosting.channel_id", source: "ChannelId", purpose: "Gateway deployment channel", secret: true },
            ParameterContract { field: "hosting.delete_int_code", source: "DeleteIntCode", purpose: "Preserve supplied string policy", secret: false },
            ParameterContract { field: "hosting.enrollment_confirmed", source: "Existing account hosting enrollment", purpose: "Import does not enable server entitlement", secret: false },
            ParameterContract { field: "config.profile", source: "Original explicit native connection, routing and receiver bindings", purpose: "Required by the native account runtime; no guessed endpoint, authority, receiver or current readiness", secret: true },
            ParameterContract { field: "config.reconnect (optional)", source: "Explicit local policy {max_attempts 1..16, initial_delay_ms 1000..600000, max_delay_ms<=3600000}; requires cleanup_on_disconnect", purpose: "Bounded fresh restart of the same stored revision only after this manager's retired run fully cleaned up; disconnect/timeout/unavailable only, never auth/hosting rejection/expiry, token renewal or session resume", secret: false },
            ParameterContract { field: "config.cleanup_on_disconnect (optional)", source: "Explicit local account lifecycle policy; default false", purpose: "Bounded cleanup of this manager's retired original run only; no automatic restart, token renewal or prior-process takeover", secret: false },
            ParameterContract { field: "config.call_media (optional)", source: "Original Endpoint authority/root/routing and explicit local audio/Trunk settings", purpose: "Required for incoming voice; plain-media opt-in, current original call binding and supported SDP, never a SIM cost grant", secret: true },
        ],
    }
}
