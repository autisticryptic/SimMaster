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
