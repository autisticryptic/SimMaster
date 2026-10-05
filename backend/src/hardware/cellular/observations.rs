//! Backend-neutral observations consumed by the line registry.
//!
//! This interface exposes no radio-enable, bearer-connect or SIM-selection
//! commands. It does not define daemon startup policy. The currently shipped
//! implementation wraps the existing MM queries; native
//! implementations can be added without passing D-Bus connections through the
//! registry/API refresh path.

use std::{error::Error, fmt};

use crate::{
    connectivity::core::access_network::ServingAccessSnapshot,
    hardware::devices::transport::TransportFuture,
};

use super::bindings::ModemBinding;

/// Display evidence only, never an operational binding or a control selector.
/// SIM presence is MM's cached observation, not an active card probe.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct PassiveModemInventory {
    pub line_id: String,
    pub manufacturer: String,
    pub model: String,
    pub slot_source: String,
    pub slot_stable: bool,
    pub uim_slot: u8,
    pub present: bool,
    /// None means MM did not provide an authoritative Sim property.
    pub sim_missing: Option<bool>,
    pub observation_source: &'static str,
}

/// A failed observation is not necessarily authoritative loss of service.
///
/// Providers classify their own protocol errors. For serving-cell observations,
/// registry consumers must not parse MM, QMI or MBIM error strings to decide
/// whether to keep a last-known snapshot until its existing TTL expires.
/// Whole-inventory discovery failure policy is separately owned by the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationError {
    Unavailable(String),
    Transient(String),
}

impl ObservationError {
    pub fn reason(&self) -> &str {
        match self {
            Self::Unavailable(reason) | Self::Transient(reason) => reason,
        }
    }

    pub fn invalidates_context(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

impl fmt::Display for ObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.reason())
    }
}

impl Error for ObservationError {}

/// Injected once when constructing the registry, not hot-swapped during IMS.
///
/// `ModemBinding` retains legacy serialized selector fields for compatibility;
/// only a concrete provider may interpret them as backend object paths.
pub trait ModemObservationProvider: Send + Sync {
    fn name(&self) -> &'static str;

    fn discover(&self) -> TransportFuture<'_, Result<Vec<ModemBinding>, ObservationError>>;

    /// Strictly passive display inventory. No protocol discovery, SIM/APDU/UIM
    /// access, transports, config migration, or runtime construction is allowed.
    /// In particular, never delegate to discover(): MM's ordinary discovery can
    /// read USIM identity through a logical channel. Providers must opt in.
    fn discover_passive(
        &self,
    ) -> TransportFuture<'_, Result<Vec<PassiveModemInventory>, ObservationError>> {
        Box::pin(async {
            Err(ObservationError::Unavailable(
                "passive_inventory_unsupported".into(),
            ))
        })
    }

    /// Fresh, positive evidence for registered non-roaming cellular voice.
    /// Unknown/unsupported providers fail closed; never infer home from an APN
    /// or from a default `roaming=false` on a missing registration property.
    fn registered_home_voice<'a>(
        &'a self,
        _binding: &'a ModemBinding,
    ) -> TransportFuture<'a, Result<bool, ObservationError>> {
        Box::pin(async {
            Err(ObservationError::Unavailable(
                "voice_home_observation_unsupported".into(),
            ))
        })
    }

    fn serving_access<'a>(
        &'a self,
        binding: &'a ModemBinding,
    ) -> TransportFuture<'a, Result<ServingAccessSnapshot, ObservationError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_is_typed_not_inferred_from_message_text() {
        let definitive = ObservationError::Unavailable("native_no_service".to_string());
        assert!(definitive.invalidates_context());
        assert_eq!(definitive.to_string(), "native_no_service");

        let transient =
            ObservationError::Transient("access_network_not_registered:transport_payload".into());
        assert!(!transient.invalidates_context());
        assert_eq!(
            transient.reason(),
            "access_network_not_registered:transport_payload"
        );
    }
}
