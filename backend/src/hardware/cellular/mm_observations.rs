//! ModemManager adapter for the registry's discovery/serving observation seam.
//!
//! Construction only retains the caller's existing connection. It does not
//! open another bus, enable a modem, create an NM profile or change boot/radio
//! policy. The delegated queries deliberately preserve the 1.1.4 behavior.

use std::sync::Arc;

use zbus::Connection;

use crate::{
    connectivity::{core::access_network::ServingAccessSnapshot, modems::ims::access_network},
    hardware::devices::transport::TransportFuture,
};

use super::{
    bindings::ModemBinding,
    modem_manager,
    observations::{ModemObservationProvider, ObservationError},
};

pub struct ModemManagerObservations {
    connection: Arc<Connection>,
}

impl ModemManagerObservations {
    pub fn new(connection: Arc<Connection>) -> Self {
        Self { connection }
    }
}

fn classify_serving_failure(reason: String) -> ObservationError {
    if reason.starts_with("access_network_not_registered:")
        || reason.starts_with("access_network_snapshot_incomplete:")
        || reason == "access_network_unavailable_for_line_kind"
    {
        ObservationError::Unavailable(reason)
    } else {
        ObservationError::Transient(reason)
    }
}

impl ModemObservationProvider for ModemManagerObservations {
    fn name(&self) -> &'static str {
        "modemmanager"
    }

    fn discover(&self) -> TransportFuture<'_, Result<Vec<ModemBinding>, ObservationError>> {
        Box::pin(async move {
            modem_manager::discover_modem_bindings(self.connection.as_ref())
                .await
                .map_err(|error| ObservationError::Transient(error.to_string()))
        })
    }

    fn serving_access<'a>(
        &'a self,
        binding: &'a ModemBinding,
    ) -> TransportFuture<'a, Result<ServingAccessSnapshot, ObservationError>> {
        Box::pin(async move {
            // This is an MM adapter precondition, not a requirement that native
            // providers invent a ModemManager object path.
            if binding.modem_path.trim().is_empty() {
                return Err(ObservationError::Unavailable(
                    "access_network_unavailable_for_line_kind".to_string(),
                ));
            }
            access_network::serving_access_snapshot(self.connection.as_ref(), &binding.modem_path)
                .await
                .map_err(classify_serving_failure)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mm_definitive_failures_preserve_the_existing_clear_policy() {
        for reason in [
            "access_network_not_registered:searching",
            "access_network_snapshot_incomplete:tech=lte;plmn=missing",
            "access_network_unavailable_for_line_kind",
        ] {
            let error = classify_serving_failure(reason.to_string());
            assert!(error.invalidates_context(), "{reason}");
            assert_eq!(error.reason(), reason);
        }
    }

    #[test]
    fn mm_query_failures_preserve_the_existing_ttl_policy() {
        for reason in [
            "access_network_network_query_failed:timeout",
            "access_network_cell_query_failed:connection_lost",
            "access_network_modem_path_invalid",
        ] {
            let error = classify_serving_failure(reason.to_string());
            assert!(!error.invalidates_context(), "{reason}");
            assert_eq!(error.to_string(), reason);
        }
    }
}
