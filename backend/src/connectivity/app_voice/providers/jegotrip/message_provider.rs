//! Explicit installation of the implemented inbound Smgw-text capability.
//! Available is per current original application-account run, not a SIM/IMS
//! grant or permission to send a paid message. Incoming voice is not advertised
//! until its media/runtime integration is implemented and independently verified.
use super::{account_runtime::AccountManager, LABEL, PROVIDER_ID};
use crate::connectivity::app_voice::registry::{Non3gppVoiceProvider, ProviderAvailability, ProviderDescriptor, VoiceCapability};
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
            capabilities: vec![VoiceCapability::Messaging] }, accounts: Arc::downgrade(accounts) }
    }
}
impl Non3gppVoiceProvider for MessagingProvider {
    fn descriptor(&self) -> &ProviderDescriptor { &self.descriptor }
    fn availability(&self, line_id: &str) -> ProviderAvailability {
        self.accounts.upgrade().map_or(ProviderAvailability::NotConfigured, |accounts| accounts.messaging_availability(line_id))
    }
}
#[cfg(test)]
#[path = "message_provider_tests.rs"]
mod tests;
