//! Explicit installation of implemented basic inbound voice/text capability.
//! Availability is per current original application-account run, never a SIM/IMS
//! grant or permission to send a paid message/place an outbound call. Voice
//! requires its explicitly configured original media/call supervisor.
use super::{account_runtime::AccountManager, LABEL, PROVIDER_ID};
use crate::connectivity::app_voice::registry::{Non3gppVoiceProvider, ProviderAvailability, ProviderDescriptor, ProviderOperation, VoiceCapability};
use std::{fmt, sync::{Arc, Weak}};

pub(crate) struct MessagingProvider {
    descriptor: ProviderDescriptor,
    accounts: Weak<AccountManager>,
}
impl fmt::Debug for MessagingProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("MessagingProvider(<redacted>)") }
}
impl MessagingProvider {
    /// Construction does not load credentials, create a run or resolve/dial.
    /// Main explicitly installs this only after constructing the owning manager.
    pub(crate) fn for_accounts(accounts: &Arc<AccountManager>) -> Self {
        Self { descriptor: ProviderDescriptor { id: PROVIDER_ID, label: LABEL,
            capabilities: vec![VoiceCapability::Voice, VoiceCapability::Messaging] }, accounts: Arc::downgrade(accounts) }
    }
}
impl Non3gppVoiceProvider for MessagingProvider {
    fn descriptor(&self) -> &ProviderDescriptor { &self.descriptor }
    fn availability(&self, line_id: &str) -> ProviderAvailability {
        self.accounts.upgrade().map_or(ProviderAvailability::NotConfigured, |accounts| accounts.inbound_availability(line_id))
    }
    fn capability_availability(&self, line_id: &str, capability: VoiceCapability) -> ProviderAvailability {
        if !matches!(capability, VoiceCapability::Voice | VoiceCapability::Messaging) { return ProviderAvailability::Unsupported; }
        self.accounts.upgrade().map_or(ProviderAvailability::NotConfigured, |accounts| match capability {
            VoiceCapability::Voice => accounts.call_availability(line_id),
            VoiceCapability::Messaging => accounts.messaging_availability(line_id),
            _ => ProviderAvailability::Unsupported,
        })
    }
    fn implemented_operations(&self) -> &'static [ProviderOperation] { super::BASIC_INBOUND_OPERATIONS }
}
#[cfg(test)]
#[path = "message_provider_tests.rs"]
mod tests;
