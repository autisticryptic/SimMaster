//! E911 provider registry.
//!
//! A provider is derived exclusively from the sealed carrier catalog's
//! `/services/e911` policy (`provider`, `entitlement_url`, `websheet_host_policy`)
//! — never from MCC/MNC guessing. Unknown operators resolve to
//! [`ProviderKind::MetadataOnly`], which performs no network requests.

use crate::connectivity::core::entitlement::ProviderKind;
use crate::connectivity::modems::ims::vowifi::profiles::CarrierProfile;

/// A resolved E911 provider for one carrier profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E911Provider {
    pub profile_id: String,
    pub kind: ProviderKind,
    /// Entitlement query endpoint (only set when `kind.may_query()`).
    pub entitlement_url: Option<String>,
    /// Hosts permitted to be contacted for this provider. The entitlement host
    /// is always included; websheet redirect targets must be a subset.
    pub host_allow_list: Vec<String>,
    /// Policy name for websheet hosts (e.g. `public_https`). `None` means no
    /// websheet is expected.
    pub websheet_host_policy: Option<String>,
}

/// Normalize a policy value to a [`ProviderKind`]. Returns `None` when the
/// carrier profile gives no usable evidence, which forces `MetadataOnly`.
pub fn kind_for_provider(provider: &str) -> Option<ProviderKind> {
    match provider.trim() {
        "" => None,
        "ts43" => Some(ProviderKind::Ts43),
        "external_portal" => Some(ProviderKind::ExternalPortal),
        "native_verified" => Some(ProviderKind::NativeVerified),
        other => {
            // Catalog may use branded names; only the three known kinds can
            // ever trigger requests. Anything unknown stays metadata-only.
            let _ = other;
            None
        }
    }
}

/// Validate an entitlement URL for basic sanity (scheme + present host). Deep
/// SSRF checks (DNS/IP/redirect) happen in the SSRF client at request time.
pub fn validate_entitlement_url(url: &str) -> Option<&str> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    parsed
        .host_str()
        .filter(|host| !host.is_empty())
        .map(|_| url)
}

/// Host of an https URL, lowercased, or `None` when the URL is unusable.
pub fn host_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url.trim()).ok()?;
    parsed.host_str().map(|host| host.to_ascii_lowercase())
}

impl E911Provider {
    /// Whether this provider is allowed to perform automated entitlement
    /// network requests.
    pub fn may_query(&self) -> bool {
        self.kind.may_query()
    }
}

/// Build the provider for a carrier profile. Evidence must come from the
/// sealed catalog record, never derived from the PLMN.
pub fn provider_from_profile(profile: &CarrierProfile) -> E911Provider {
    let e911 = &profile.e911;
    let provider = e911.provider.unwrap_or("");
    let mut kind = kind_for_provider(provider).unwrap_or(ProviderKind::MetadataOnly);

    let mut host_allow_list = Vec::new();
    let entitlement_url = if kind.may_query() {
        e911.entitlement_url
            .filter(|url| validate_entitlement_url(url).is_some())
            .map(|url| url.to_string())
    } else {
        None
    };
    if let Some(url) = &entitlement_url {
        if let Some(host) = host_of(url) {
            host_allow_list.push(host);
        }
    }
    // A query-capable provider with no usable HTTPS endpoint can never run a
    // request, so downgrade to metadata-only. Unknown URLs never auto-hit other
    // endpoints.
    if kind.may_query() && entitlement_url.is_none() {
        kind = ProviderKind::MetadataOnly;
    }

    let websheet_host_policy = e911.websheet_host_policy.map(str::to_string);
    // A websheet policy without a matching allow-list host would block every
    // redirect, which is safe-by-default: the SSRF client re-checks against
    // the allow list and will refuse if nothing matches.

    E911Provider {
        profile_id: profile.meta.profile_id.to_string(),
        kind,
        entitlement_url,
        host_allow_list,
        websheet_host_policy,
    }
}

/// A registry over the carriers the catalog currently exposes. Kept as a
/// snapshot so callers never re-open the catalog per request.
#[derive(Debug, Clone, Default)]
pub struct E911ProviderRegistry {
    providers: Vec<E911Provider>,
}

impl E911ProviderRegistry {
    pub fn new(providers: Vec<E911Provider>) -> Self {
        Self { providers }
    }

    pub fn provider_for(&self, profile_id: &str) -> Option<&E911Provider> {
        self.providers
            .iter()
            .find(|provider| provider.profile_id == profile_id)
    }

    /// Fallback that never triggers requests: used when a profile is not in the
    /// registry so unknown operators stay metadata-only.
    pub fn metadata_only_for(&self, profile_id: &str) -> E911Provider {
        E911Provider {
            profile_id: profile_id.to_string(),
            kind: ProviderKind::MetadataOnly,
            entitlement_url: None,
            host_allow_list: Vec::new(),
            websheet_host_policy: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::core::entitlement::ProviderKind;
    use crate::connectivity::modems::ims::vowifi::profile_record::CarrierProfileRecord;
    use crate::connectivity::modems::ims::vowifi::profiles::generate_standard_3gpp_profile;
    use crate::connectivity::modems::ims::vowifi::profiles::CarrierProfile;

    fn with_e911_profile(kind: &str, url: Option<&str>, policy: Option<&str>) -> CarrierProfile {
        let profile = generate_standard_3gpp_profile("234", "33", 2);
        let mut record = CarrierProfileRecord::from_profile(profile);
        record.e911.provider = Some(kind.to_string());
        record.e911.entitlement_url = url.map(str::to_string);
        record.e911.websheet_host_policy = policy.map(str::to_string);
        *record.intern()
    }

    #[test]
    fn metadata_only_when_no_evidence() {
        let provider = provider_from_profile(&with_e911_profile("", None, None));
        assert_eq!(provider.kind, ProviderKind::MetadataOnly);
        assert!(provider.entitlement_url.is_none());
        assert!(provider.host_allow_list.is_empty());
    }

    #[test]
    fn unknown_provider_never_queries() {
        let provider = provider_from_profile(&with_e911_profile("mystery_brand", None, None));
        assert_eq!(provider.kind, ProviderKind::MetadataOnly);
        assert!(provider.entitlement_url.is_none());
    }

    #[test]
    fn ts43_provider_keeps_https_url_and_host() {
        let provider = provider_from_profile(&with_e911_profile(
            "ts43",
            Some("https://entitlement.example.net/query"),
            Some("public_https"),
        ));
        assert_eq!(provider.kind, ProviderKind::Ts43);
        assert!(provider.may_query());
        assert_eq!(
            provider.entitlement_url.as_deref(),
            Some("https://entitlement.example.net/query")
        );
        assert!(provider
            .host_allow_list
            .contains(&"entitlement.example.net".to_string()));
        assert_eq!(
            provider.websheet_host_policy.as_deref(),
            Some("public_https")
        );
    }

    #[test]
    fn non_https_or_empty_url_is_dropped_silently() {
        let provider = provider_from_profile(&with_e911_profile(
            "ts43",
            Some("http://entitlement.example.net/query"),
            None,
        ));
        assert_eq!(provider.kind, ProviderKind::MetadataOnly);
        assert!(provider.entitlement_url.is_none());

        let provider = provider_from_profile(&with_e911_profile("native_verified", Some(""), None));
        assert_eq!(provider.kind, ProviderKind::MetadataOnly);
        assert!(provider.entitlement_url.is_none());
    }

    #[test]
    fn registry_fallback_is_metadata_only() {
        let registry = E911ProviderRegistry::default();
        let provider = registry.metadata_only_for("profile-x");
        assert_eq!(provider.kind, ProviderKind::MetadataOnly);
        assert!(!provider.may_query());
    }

    #[test]
    fn validate_entitlement_url_rejects_bad_input() {
        assert!(validate_entitlement_url("https://ok.example").is_some());
        assert!(validate_entitlement_url("http://bad.example").is_none());
        assert!(validate_entitlement_url("not a url").is_none());
        assert!(validate_entitlement_url("  ").is_none());
        assert_eq!(
            host_of("https://HOST.example/path"),
            Some("host.example".to_string())
        );
        assert_eq!(host_of("not a url"), None);
    }
}
