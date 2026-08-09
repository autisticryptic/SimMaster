use crate::platform::config::AutomationTarget;
use crate::state::AppState;
use anyhow::{anyhow, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAutomationModem {
    pub line_id: String,
    pub modem_path: String,
}

pub fn target_line_id(target: Option<&AutomationTarget>) -> Option<&str> {
    match target {
        Some(AutomationTarget::ModemLine { line_id }) => {
            let line_id = line_id.trim();
            (!line_id.is_empty()).then_some(line_id)
        }
        _ => None,
    }
}

/// Resolve the persistent automation target to a live ModemManager object.
/// Reader reservations are intentionally rejected until a real PC/SC/QMI AKA
/// adapter is connected to the runtime.
pub async fn resolve_modem_target(
    app: &AppState,
    params: &serde_json::Value,
) -> Result<ResolvedAutomationModem> {
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
            if !app.config_manager.get_line_profile(&line_id).enabled {
                return Err(anyhow!("automation_target_line_disabled"));
            }
            if binding.modem_path.trim().is_empty()
                || (!binding.line_kind.is_empty() && binding.line_kind != "baseband")
            {
                return Err(anyhow!("automation_target_line_has_no_baseband"));
            }
            Ok(ResolvedAutomationModem {
                line_id: binding.line_id,
                modem_path: binding.modem_path,
            })
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
        None => Err(anyhow!("automation_target_line_required")),
    }
}

#[cfg(test)]
mod tests {
    use crate::platform::config::AutomationTarget;

    #[test]
    fn modem_target_json_keeps_the_explicit_line() {
        let target = serde_json::to_value(AutomationTarget::ModemLine {
            line_id: "line-b".to_string(),
        })
        .unwrap();
        assert_eq!(target["kind"], "modem_line");
        assert_eq!(target["line_id"], "line-b");
    }
}
