//! ModemManager adapter for the registry's discovery/serving observation seam.
//!
//! Construction only retains the caller's existing connection. It does not
//! open another bus, enable a modem, create an NM profile or change boot/radio
//! policy. The delegated queries deliberately preserve the 1.1.4 behavior.

use std::{collections::HashMap, sync::Arc, time::Duration};

use zbus::{proxy::CacheProperties, zvariant::OwnedValue, Connection};

use crate::{
    connectivity::{core::access_network::ServingAccessSnapshot, modems::ims::access_network},
    hardware::devices::transport::TransportFuture,
};

use super::{
    bindings::ModemBinding,
    modem_manager,
    observations::{ModemObservationProvider, ObservationError, PassiveModemInventory},
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

const MM_SERVICE: &str = "org.freedesktop.ModemManager1";
const MM_MODEM: &str = "org.freedesktop.ModemManager1.Modem";
type Properties = HashMap<String, OwnedValue>;

fn home_voice_state(modem_state: Option<i32>, registration: Option<u32>) -> bool {
    // MM home SMS-only (6) is not evidence that cellular voice is usable.
    matches!(modem_state, Some(8 | 10 | 11)) && matches!(registration, Some(1 | 9))
}

async fn read_properties(
    connection: &Connection,
    owner: &str,
    path: &str,
    interface: &str,
) -> zbus::Result<Properties> {
    zbus::proxy::Builder::<zbus::Proxy<'_>>::new(connection)
        .destination(owner)?
        .path(path)?
        .interface("org.freedesktop.DBus.Properties")?
        .cache_properties(CacheProperties::No)
        .build()
        .await?
        .call("GetAll", &(interface,))
        .await
}

fn matches_voice_binding(modem: &Properties, binding: &ModemBinding) -> bool {
    let sim = modem
        .get("Sim")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| zbus::zvariant::OwnedObjectPath::try_from(v).ok());
    let port = modem
        .get("PrimaryPort")
        .and_then(|v| <&str>::try_from(v).ok());
    let slot = match modem.get("PrimarySimSlot") {
        None => Some(1),
        Some(value) => u32::try_from(value).ok().and_then(|n| match n {
            0 => Some(1),
            1..=255 => Some(n),
            _ => None,
        }),
    };
    sim.is_some_and(|sim| sim.as_str() != "/" && binding.sim_path.as_deref() == Some(sim.as_str()))
        && port == Some(binding.primary_port.as_str())
        && slot == Some(u32::from(binding.uim_slot))
}

async fn home_voice_sample(
    connection: &Connection,
    owner: &str,
    binding: &ModemBinding,
) -> zbus::Result<bool> {
    let modem = read_properties(connection, owner, &binding.modem_path, MM_MODEM).await?;
    if !matches_voice_binding(&modem, binding) {
        return Ok(false);
    }
    let sim = binding
        .sim_path
        .as_deref()
        .expect("binding matched non-root SIM");
    let gpp = read_properties(
        connection,
        owner,
        &binding.modem_path,
        "org.freedesktop.ModemManager1.Modem.Modem3gpp",
    )
    .await?;
    // Bracket the registration read with endpoint/SIM identity checks. A SIM
    // change during GetAll must not authorize the old call using the new PLMN.
    let after = read_properties(connection, owner, &binding.modem_path, MM_MODEM).await?;
    if !matches_voice_binding(&after, binding) {
        return Ok(false);
    }
    let identity =
        read_properties(connection, owner, sim, "org.freedesktop.ModemManager1.Sim").await?;
    let id = identity
        .get("SimIdentifier")
        .and_then(|v| <&str>::try_from(v).ok())
        .unwrap_or("");
    let registration = gpp
        .get("RegistrationState")
        .and_then(|v| u32::try_from(v).ok());
    Ok(crate::platform::utils::normalize_iccid(id)
        == crate::platform::utils::normalize_iccid(&binding.sim_iccid)
        && home_voice_state(
            modem.get("State").and_then(|v| i32::try_from(v).ok()),
            registration,
        )
        && home_voice_state(
            after.get("State").and_then(|v| i32::try_from(v).ok()),
            registration,
        ))
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

    fn discover_passive(
        &self,
    ) -> TransportFuture<'_, Result<Vec<PassiveModemInventory>, ObservationError>> {
        Box::pin(async move {
            modem_manager::discover_passive_modems(self.connection.as_ref())
                .await
                .map_err(|error| ObservationError::Transient(format!(
                    "passive_inventory_unavailable: {error}"
                )))
        })
    }

    fn registered_home_voice<'a>(
        &'a self,
        binding: &'a ModemBinding,
    ) -> TransportFuture<'a, Result<bool, ObservationError>> {
        Box::pin(async move {
            if !binding.present
                || binding.slot_conflict
                || binding.primary_port.is_empty()
                || crate::platform::utils::normalize_iccid(&binding.sim_iccid).is_empty()
                || !binding
                    .modem_path
                    .starts_with("/org/freedesktop/ModemManager1/Modem/")
            {
                return Ok(false);
            }
            tokio::time::timeout(Duration::from_millis(750), async {
                let manager = zbus::fdo::DBusProxy::new(&self.connection).await?;
                let name = MM_SERVICE.try_into().expect("constant MM service");
                let owner = manager.get_name_owner(name).await?;
                let home = home_voice_sample(&self.connection, owner.as_str(), binding).await?
                    && home_voice_sample(&self.connection, owner.as_str(), binding).await?;
                let current = manager
                    .get_name_owner(MM_SERVICE.try_into().expect("constant MM service"))
                    .await?;
                Ok::<_, zbus::Error>(home && owner == current)
            })
            .await
            .map_err(|_| ObservationError::Transient("voice_home_observation_timeout".into()))?
            .map_err(|_| ObservationError::Transient("voice_home_observation_unavailable".into()))
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
#[path = "mm_voice_observation_tests.rs"]
mod dbus_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cellular_voice_requires_registered_home_not_missing_or_sms_only() {
        for state in [8, 10, 11] {
            assert!(home_voice_state(Some(state), Some(1)));
            assert!(home_voice_state(Some(state), Some(9)));
            for registration in [
                None,
                Some(0),
                Some(2),
                Some(5),
                Some(6),
                Some(7),
                Some(8),
                Some(10),
            ] {
                assert!(!home_voice_state(Some(state), registration));
            }
        }
        for state in [None, Some(0), Some(3), Some(7), Some(9)] {
            assert!(!home_voice_state(state, Some(1)));
        }
    }

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
