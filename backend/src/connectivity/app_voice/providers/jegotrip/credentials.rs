//! User-supplied credentials; account-login UI and server authorization differ.
use serde::{Deserialize, Serialize};
use std::fmt;

/// Never formatted as cleartext, even in validation diagnostics.
#[derive(Clone, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn expose_to_backend(&self) -> &str { &self.0 }
}

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
    pub root_der_base64: Secret,
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
        if self.hosting.delete_int_code.is_empty()
            || self.hosting.delete_int_code.len() > 32
            || self.hosting.delete_int_code.chars().any(char::is_control)
        { return Err("jegotrip_gateway_policy_invalid"); }
        if !self.sdk.tls_only { return Err("jegotrip_tls_required"); }
        if !self.hosting.enrollment_confirmed { return Err("jegotrip_hosting_enrollment_required"); }
        if self.sdk.expires_at_unix_seconds.is_some_and(|at| at <= now_unix_seconds) {
            return Err("jegotrip_credential_expired");
        }
        Ok(())
    }
}

/// Static contract, not a claim that the transport can connect yet.
#[derive(Debug, Serialize)]
pub struct CredentialContract {
    pub provider_id: &'static str,
    pub account_login_ui_required: bool,
    pub service_credentials_required: bool,
    pub transport_implemented: bool,
    pub direct_sip_interoperability: &'static str,
    pub renewal: &'static str,
    pub verified_sdk_call_path: &'static str,
    pub required_parameters: Vec<ParameterContract>,
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
        transport_implemented: false,
        direct_sip_interoperability: "unverified",
        renewal: "manual_reimport_until_renewal_verified",
        verified_sdk_call_path: "JCClient.login -> JCAccount.updateStatus(4)",
        required_parameters: vec![
            ParameterContract { field: "sdk.account_identity", source: "getAreaCodeMobileNum", purpose: "SDK session identity", secret: true },
            ParameterContract { field: "sdk.phone_login_token", source: "wyx_phone_login_token / wyx_phone_login_token_key; CallUserInfo.cstToken", purpose: "JCClient.login password argument", secret: true },
            ParameterContract { field: "sdk.application_key", source: "JCClient.create configuration", purpose: "SDK application", secret: true },
            ParameterContract { field: "sdk.server_address", source: "config_key_server_address", purpose: "SDK server, not an inferred SIP SBC", secret: true },
            ParameterContract { field: "sdk.device_id (optional)", source: "config_key_device_id", purpose: "Preserve the original device binding if configured", secret: true },
            ParameterContract { field: "sdk.expires_at_unix_seconds (optional)", source: "Verified token expiry, if known", purpose: "Unknown expiry must remain unknown", secret: false },
            ParameterContract { field: "sdk.root_der_base64", source: "config_key_root_der", purpose: "Deployment TLS trust", secret: true },
            ParameterContract { field: "sdk.tls_only", source: "config_key_use_tls_only", purpose: "TLS required", secret: false },
            ParameterContract { field: "hosting.sip_number", source: "SipNumber / getAreaCodeMobileNum", purpose: "Hosted number", secret: true },
            ParameterContract { field: "hosting.sip_password", source: "voip_status.sipPassword after decryptThreeDESECB; legacy callAccountSipPassword requires format check", purpose: "SipPwd gateway credential, distinct from SDK token", secret: true },
            ParameterContract { field: "hosting.authname_prefix", source: "AuthnamePrefix", purpose: "Gateway deployment policy", secret: true },
            ParameterContract { field: "hosting.channel_id", source: "ChannelId", purpose: "Gateway deployment channel", secret: true },
            ParameterContract { field: "hosting.delete_int_code", source: "DeleteIntCode", purpose: "Preserve supplied string policy", secret: false },
            ParameterContract { field: "hosting.enrollment_confirmed", source: "Existing account hosting enrollment", purpose: "Import does not enable server entitlement", secret: false },
        ],
    }
}
