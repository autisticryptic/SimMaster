use crate::hardware::cellular::modem_manager::find_modem_path;
use crate::platform::config::AutomationTarget;
use crate::state::AppState;
use anyhow::{anyhow, Result};

/// Resolve the persistent automation target to a live ModemManager object.
/// Reader reservations are intentionally rejected until a real PC/SC/QMI AKA
/// adapter is connected to the runtime.
pub async fn resolve_modem_path(app: &AppState, params: &serde_json::Value) -> Result<String> {
    let target = params
        .get("target")
        .filter(|value| !value.is_null())
        .map(|value| serde_json::from_value::<AutomationTarget>(value.clone()))
        .transpose()?;

    match target {
        Some(AutomationTarget::ModemLine { line_id }) => {
            app.line_registry
                .refresh(app.dbus_conn.as_ref())
                .await
                .map_err(|error| anyhow!("automation_target_refresh_failed: {error}"))?;
            let line = app
                .line_registry
                .get(&line_id)
                .await
                .ok_or_else(|| anyhow!("automation_target_line_not_found"))?;
            let binding = line.binding();
            if !binding.present {
                return Err(anyhow!("automation_target_line_not_present"));
            }
            Ok(binding.modem_path)
        }
        Some(AutomationTarget::StandaloneSimSlot { slot_id }) => {
            let slot = app
                .config_manager
                .get_standalone_sim_slots()
                .into_iter()
                .find(|slot| slot.id == slot_id)
                .ok_or_else(|| anyhow!("automation_target_reader_slot_not_found"))?;
            if !slot.enabled {
                return Err(anyhow!("automation_target_reader_slot_disabled"));
            }
            Err(anyhow!("automation_target_reader_runtime_unavailable"))
        }
        None => find_modem_path(&app.dbus_conn)
            .await
            .map_err(|error| anyhow!("automation_primary_modem_unavailable: {error}")),
    }
}
