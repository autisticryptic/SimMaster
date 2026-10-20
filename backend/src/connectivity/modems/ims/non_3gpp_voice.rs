//! Reserved interface for non-3GPP, application-based voice/messaging.
//!
//! Scope note: this module defines *contracts only*. No provider is implemented
//! here, and none is registered by default. Services such as carrier apps,
//! iMessage/FaceTime or third-party voice apps authenticate against the
//! vendor's own servers; integrating one requires that vendor's authorization
//! and its published/consented API. Re-implementing a vendor's private
//! authentication is explicitly out of scope for this project.
//!
//! A future provider must:
//! - run on a no-modem WLAN line (see the virtual-SIM line work), never on a
//!   modem line's UICC path;
//! - declare its capabilities instead of assuming IMS equivalence;
//! - keep its credentials out of logs, diagnostics and the public API.
use std::collections::BTreeMap;
use std::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VoiceCapability {
    /// Real-time voice over an application data channel.
    Voice,
    /// Text messaging through the provider.
    Messaging,
    /// Video calling.
    Video,
    /// Group or conference calling.
    Conference,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDescriptor {
    /// Stable identifier, e.g. `"vendor-app-voice"`.
    pub id: &'static str,
    /// Human-readable label for the UI.
    pub label: &'static str,
    pub capabilities: Vec<VoiceCapability>,
}

/// Contract for a provider. Implementations are intentionally absent: adding
/// one requires vendor authorization and a separate review.
pub trait Non3gppVoiceProvider: Send + Sync {
    fn descriptor(&self) -> &ProviderDescriptor;

    /// Whether this provider can currently be used on the given line. A
    /// provider must not report availability based on a cached "last success".
    fn availability(&self, line_id: &str) -> ProviderAvailability;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAvailability {
    Available,
    /// Not configured/registered yet (no credentials, no consent).
    NotConfigured,
    /// Explicitly unsupported on this line (e.g. modem line).
    Unsupported,
    Unknown,
}

/// Runtime registry. Empty unless a provider is explicitly installed at
/// startup; there is deliberately no auto-discovery.
#[derive(Default)]
pub struct ProviderRegistry {
    providers: RwLock<BTreeMap<&'static str, Box<dyn Non3gppVoiceProvider>>>,
}

impl std::fmt::Debug for ProviderRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderRegistry")
            .field("installed", &self.providers.read().map(|map| map.len()).unwrap_or(0))
            .finish()
    }
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Install one provider. Duplicate ids are rejected so two integrations
    /// cannot silently shadow each other.
    pub fn install(&self, provider: Box<dyn Non3gppVoiceProvider>) -> Result<(), &'static str> {
        let id = provider.descriptor().id;
        if id.is_empty() {
            return Err("ims_non_3gpp_provider_invalid");
        }
        let mut providers = self.providers.write().map_err(|_| "ims_non_3gpp_provider_invalid")?;
        if providers.contains_key(id) {
            return Err("ims_non_3gpp_provider_duplicate");
        }
        providers.insert(id, provider);
        Ok(())
    }

    pub fn descriptors(&self) -> Vec<ProviderDescriptor> {
        self.providers.read().map(|map| map.values().map(|p| p.descriptor().clone()).collect())
            .unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.read().map(|map| map.is_empty()).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stub(ProviderDescriptor);

    impl Non3gppVoiceProvider for Stub {
        fn descriptor(&self) -> &ProviderDescriptor { &self.0 }
        fn availability(&self, line_id: &str) -> ProviderAvailability {
            if line_id.starts_with("modem-") { ProviderAvailability::Unsupported }
            else { ProviderAvailability::NotConfigured }
        }
    }

    fn descriptor() -> ProviderDescriptor {
        ProviderDescriptor { id: "vendor-app-voice", label: "Vendor app voice",
            capabilities: vec![VoiceCapability::Voice, VoiceCapability::Messaging] }
    }

    #[test]
    fn nothing_is_registered_by_default() {
        let registry = ProviderRegistry::new();
        assert!(registry.is_empty());
        assert!(registry.descriptors().is_empty());
        assert!(format!("{registry:?}").contains("installed: 0"));
    }

    #[test]
    fn duplicate_ids_and_empty_ids_are_rejected() {
        let registry = ProviderRegistry::new();
        registry.install(Box::new(Stub(descriptor()))).unwrap();
        assert_eq!(registry.install(Box::new(Stub(descriptor()))).err(),
            Some("ims_non_3gpp_provider_duplicate"));
        let empty = ProviderDescriptor { id: "", label: "x", capabilities: vec![] };
        assert_eq!(registry.install(Box::new(Stub(empty))).err(), Some("ims_non_3gpp_provider_invalid"));
        assert_eq!(registry.descriptors(), vec![descriptor()]);
    }

    #[test]
    fn availability_is_per_line_and_not_a_cached_success() {
        let provider = Stub(descriptor());
        assert_eq!(provider.availability("modem-line-1"), ProviderAvailability::Unsupported);
        assert_eq!(provider.availability("wlan-line-1"), ProviderAvailability::NotConfigured);
    }
}
