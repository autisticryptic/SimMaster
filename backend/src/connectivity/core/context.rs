//! Transport-neutral IMS addressing and request parameters.
//!
//! These types deliberately contain no VoWiFi profile or VoLTE bearer types.
//! Each access leg maps its discovered/configured values into this context
//! before invoking the shared SIP builders and REGISTER transaction.

use std::net::SocketAddr;

/// SIP transport advertised in Via and Contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SipTransport {
    Udp,
    Tcp,
}

impl SipTransport {
    pub const fn as_via(self) -> &'static str {
        match self {
            Self::Udp => "SIP/2.0/UDP",
            Self::Tcp => "SIP/2.0/TCP",
        }
    }

    pub const fn as_param(self) -> &'static str {
        match self {
            Self::Udp => "udp",
            Self::Tcp => "tcp",
        }
    }
}

/// IMS identity used by REGISTER, MESSAGE and dialog requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImsIdentity {
    /// IMS private identity (IMPI), without a `sip:` prefix.
    pub private_user: String,
    /// IMS public identity (IMPU), normally a complete `sip:` URI.
    pub public_uri: String,
    /// Contact URI user part.
    pub contact_user: String,
    /// Home IMS domain used as the default registrar/request URI.
    pub home_domain: String,
    /// Whether Contact should carry `;user=phone`.
    pub contact_user_phone: bool,
}

/// Local and P-CSCF addressing for one protected SIP channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImsRoute {
    pub local_addr: SocketAddr,
    pub pcscf_addr: SocketAddr,
    pub transport: SipTransport,
}

/// Neutral REGISTER policy distilled from a carrier profile or VoLTE bearer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImsRegisterParams {
    pub realm: String,
    pub domain: String,
    pub registrar: Option<String>,
    pub supported_header: String,
    pub require_sec_agree: bool,
    pub user_agent: String,
    pub pani: Option<String>,
    pub visited_network: Option<String>,
    pub allow_header: String,
    pub expires: u32,
}

impl ImsRegisterParams {
    pub fn request_uri(&self) -> String {
        format!(
            "sip:{}",
            self.registrar.as_deref().unwrap_or(self.domain.as_str())
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registrar_overrides_home_domain() {
        let mut params = ImsRegisterParams {
            realm: "ims.example".into(),
            domain: "ims.example".into(),
            registrar: None,
            supported_header: "path,gruu".into(),
            require_sec_agree: true,
            user_agent: "SimAdmin".into(),
            pani: None,
            visited_network: None,
            allow_header: "MESSAGE".into(),
            expires: 3600,
        };
        assert_eq!(params.request_uri(), "sip:ims.example");
        params.registrar = Some("reg.ims.example".into());
        assert_eq!(params.request_uri(), "sip:reg.ims.example");
    }
}
