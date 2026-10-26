//! Contracts and explicit installation for application-based voice/messaging.
//!
//! The registry itself starts empty; main explicitly installs implemented
//! integrations. Descriptors/operations describe software, not a live account
//! grant. Aggregate availability does not imply readiness of every capability.
//! Query the requested capability separately before interpreting its readiness.
//!
//! A provider must:
//! - own an independent no-modem app-account line over ordinary application
//!   data, never a fake virtual SIM or a modem line's UICC/IMS path;
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

/// Implemented operation metadata. Listing an operation is never permission
/// to execute it or evidence that any particular account is currently ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderOperation {
    IncomingCall,
    AnswerCall,
    HangupCall,
    BidirectionalAudio,
    IncomingMessage,
    MessageHistory,
    MessageNotification,
    IncomingTrunk,
}
impl ProviderOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IncomingCall => "incoming_call", Self::AnswerCall => "answer_call",
            Self::HangupCall => "hangup_call", Self::BidirectionalAudio => "bidirectional_audio",
            Self::IncomingMessage => "incoming_message", Self::MessageHistory => "message_history",
            Self::MessageNotification => "message_notification", Self::IncomingTrunk => "incoming_trunk",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDescriptor {
    /// Stable identifier, e.g. `"app-voice"`.
    pub id: &'static str,
    /// Human-readable label for the UI.
    pub label: &'static str,
    pub capabilities: Vec<VoiceCapability>,
}

/// Contract for a provider. Implementations live in their own modules and are
/// installed explicitly at startup.
pub trait Non3gppVoiceProvider: Send + Sync {
    fn descriptor(&self) -> &ProviderDescriptor;

    /// Aggregate observation: at least one implemented capability is ready.
    /// This does not authorize an action or imply that Voice and Messaging are
    /// interchangeable. Cached last-success observations are never sufficient.
    fn availability(&self, line_id: &str) -> ProviderAvailability;

    /// Only an implementation's current original-owner proof may return
    /// Available for a particular capability. Legacy implementations without
    /// that proof remain Unknown, even if aggregate availability is Available.
    fn capability_availability(&self, _line_id: &str, capability: VoiceCapability) -> ProviderAvailability {
        if self.descriptor().capabilities.contains(&capability) { ProviderAvailability::Unknown }
        else { ProviderAvailability::Unsupported }
    }

    fn implemented_operations(&self) -> &'static [ProviderOperation] { &[] }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAvailability {
    Available,
    /// Not configured/registered yet (no credentials).
    NotConfigured,
    /// Explicitly unsupported on this line (e.g. modem line).
    Unsupported,
    Unknown,
}

/// Runtime registry. Empty unless a provider is explicitly installed at
/// startup; there is deliberately no auto-discovery.
/// Providers named in the product plan that have no implementation yet.
///
/// They are listed here so the ids and labels are reserved: a future
/// implementation keeps the same id, the UI keeps showing the same entry, and
/// nothing has to guess a name. Listing them is not registration — they are
/// reported as `Unsupported` and can never be selected for a call.
pub const PLANNED_PROVIDERS: &[(&str, &str)] = super::providers::RESERVED;

static REGISTRY: std::sync::OnceLock<ProviderRegistry> = std::sync::OnceLock::new();

/// Process-wide registry used by the API. Empty until a provider is installed.
pub fn registry() -> &'static ProviderRegistry {
    REGISTRY.get_or_init(ProviderRegistry::new)
}

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

    pub fn availability(&self, provider_id: &str, line_id: &str) -> ProviderAvailability {
        let Ok(providers) = self.providers.read() else { return ProviderAvailability::Unknown; };
        providers.get(provider_id).map_or(ProviderAvailability::Unsupported, |provider| provider.availability(line_id))
    }

    pub fn capability_availability(&self, provider_id: &str, line_id: &str, capability: VoiceCapability) -> ProviderAvailability {
        let Ok(providers) = self.providers.read() else { return ProviderAvailability::Unknown; };
        providers.get(provider_id).map_or(ProviderAvailability::Unsupported, |provider| {
            if provider.descriptor().capabilities.contains(&capability) { provider.capability_availability(line_id, capability) }
            else { ProviderAvailability::Unsupported }
        })
    }

    pub fn implemented_operations(&self, provider_id: &str) -> Vec<ProviderOperation> {
        self.providers.read().ok().and_then(|providers| providers.get(provider_id)
            .map(|provider| provider.implemented_operations().to_vec())).unwrap_or_default()
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
        ProviderDescriptor { id: "app-voice", label: "App voice",
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
    fn reserved_ids_are_unique_and_never_install_a_provider() {
        let mut ids = std::collections::BTreeSet::new();
        for &(id, label) in PLANNED_PROVIDERS {
            assert!(!id.is_empty() && !label.is_empty());
            assert!(ids.insert(id), "duplicate reserved provider id");
        }
        assert_eq!(ids.len(), 4);
        assert!(ProviderRegistry::new().is_empty());
    }

    #[test]
    fn imessage_is_not_advertised_as_voice() {
        let capabilities = super::super::providers::reserved_capabilities("imessage");
        assert_eq!(capabilities, vec![VoiceCapability::Messaging]);
        assert!(!capabilities.contains(&VoiceCapability::Voice));
    }

    #[test]
    fn unknown_provider_has_no_invented_capabilities() {
        assert!(super::super::providers::reserved_capabilities("not-installed").is_empty());
    }

    #[test]
    fn aggregate_availability_cannot_invent_capability_proof_or_implemented_operations() {
        struct AggregateOnly(ProviderDescriptor);
        impl Non3gppVoiceProvider for AggregateOnly {
            fn descriptor(&self) -> &ProviderDescriptor { &self.0 }
            fn availability(&self, _: &str) -> ProviderAvailability { ProviderAvailability::Available }
        }
        let registry = ProviderRegistry::new();
        registry.install(Box::new(AggregateOnly(descriptor()))).unwrap();
        assert_eq!(registry.availability("app-voice", "app-account-fixture"), ProviderAvailability::Available);
        for capability in [VoiceCapability::Voice, VoiceCapability::Messaging] {
            assert_eq!(registry.capability_availability("app-voice", "app-account-fixture", capability), ProviderAvailability::Unknown);
        }
        assert_eq!(registry.capability_availability("app-voice", "app-account-fixture", VoiceCapability::Video), ProviderAvailability::Unsupported);
        assert_eq!(registry.capability_availability("not-installed", "app-account-fixture", VoiceCapability::Voice), ProviderAvailability::Unsupported);
        assert!(registry.implemented_operations("app-voice").is_empty());
        assert!(registry.implemented_operations("not-installed").is_empty());
    }

    #[test]
    fn availability_is_per_line_and_not_a_cached_success() {
        let provider = Stub(descriptor());
        assert_eq!(provider.availability("modem-line-1"), ProviderAvailability::Unsupported);
        assert_eq!(provider.availability("wlan-line-1"), ProviderAvailability::NotConfigured);
    }
}
