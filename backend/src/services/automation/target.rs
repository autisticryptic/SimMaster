use crate::platform::config::AutomationTarget;
use crate::state::AppState;
use anyhow::{anyhow, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAutomationModem {
    pub line_id: String,
    pub modem_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAutomationLine {
    pub line_id: String,
    pub line_kind: String,
    pub modem_path: Option<String>,
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

pub async fn resolve_line_target(
    app: &AppState,
    params: &serde_json::Value,
) -> Result<ResolvedAutomationLine> {
    let target = params
        .get("target")
        .filter(|value| !value.is_null())
        .map(|value| serde_json::from_value::<AutomationTarget>(value.clone()))
        .transpose()?;
    let requested_line_id = match target {
        Some(AutomationTarget::ModemLine { line_id }) => line_id,
        Some(AutomationTarget::StandaloneSimSlot { slot_id }) => {
            let slot = app
                .config_manager
                .get_standalone_sim_slots()
                .into_iter()
                .find(|slot| slot.id == slot_id)
                .ok_or_else(|| anyhow!("automation_target_reader_slot_not_found"))?;
            crate::hardware::cellular::modem_manager::reader_line_id(
                slot.reader_path
                    .trim()
                    .strip_prefix("pcsc://")
                    .unwrap_or(slot.id.as_str()),
                if slot.reader_path.trim().starts_with("pcsc://") {
                    1
                } else {
                    slot.uim_slot
                },
            )
        }
        None => return Err(anyhow!("automation_target_line_required")),
    };
    app.line_registry
        .refresh(app.dbus_conn.as_ref())
        .await
        .map_err(|error| anyhow!("automation_target_refresh_failed: {error}"))?;
    let line = app
        .line_registry
        .get(&requested_line_id)
        .await
        .ok_or_else(|| anyhow!("automation_target_line_not_found"))?;
    let binding = line.binding();
    if !binding.present {
        return Err(anyhow!("automation_target_line_not_present"));
    }
    if !app
        .config_manager
        .get_line_profile(&binding.line_id)
        .enabled
    {
        return Err(anyhow!("automation_target_line_disabled"));
    }
    Ok(ResolvedAutomationLine {
        line_id: binding.line_id,
        line_kind: binding.line_kind,
        modem_path: (!binding.modem_path.trim().is_empty()).then_some(binding.modem_path),
    })
}

/// Resolve the persistent automation target to a live ModemManager object.
/// Reader reservations are intentionally rejected until a real PC/SC/QMI AKA
/// adapter is connected to the runtime.
pub async fn resolve_modem_target(
    app: &AppState,
    params: &serde_json::Value,
) -> Result<ResolvedAutomationModem> {
    let target = resolve_line_target(app, params).await?;
    let modem_path = target
        .modem_path
        .filter(|_| target.line_kind.is_empty() || target.line_kind == "baseband")
        .ok_or_else(|| anyhow!("automation_target_line_has_no_baseband"))?;
    Ok(ResolvedAutomationModem {
        line_id: target.line_id,
        modem_path,
    })
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
