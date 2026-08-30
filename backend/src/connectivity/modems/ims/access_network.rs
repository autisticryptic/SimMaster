//! Runtime serving-cell snapshots used by IMS REGISTER construction.
//!
//! This adapter is deliberately fail-closed: a carrier may request CNI/PANI,
//! but a missing or ambiguous ModemManager snapshot results in header omission
//! instead of a fabricated cell identity.

use crate::{
    connectivity::core::access_network::{AccessNetworkSource, ServingAccessSnapshot},
    hardware::cellular::modem_manager::{get_cells_data_for_modem, get_network_info_for_modem},
};
use zbus::Connection;

const MODEM_PATH_PREFIX: &str = "/org/freedesktop/ModemManager1/Modem/";

pub fn normalize_modem_path(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value
            .chars()
            .any(|ch| ch == '\r' || ch == '\n' || ch == '\0' || ch.is_control())
    {
        return None;
    }
    if let Some(index) = value.strip_prefix(MODEM_PATH_PREFIX) {
        return (!index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| value.to_string());
    }
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| format!("{MODEM_PATH_PREFIX}{value}"))
}

/// Read one complete, profile-independent serving snapshot using the caller's
/// existing D-Bus connection. This is intended for `LineRuntimeRegistry` refresh
/// passes; SIP transactions must not open their own system-bus connection.
pub async fn serving_access_snapshot(
    conn: &Connection,
    modem_path: &str,
) -> Result<ServingAccessSnapshot, String> {
    let modem_path = normalize_modem_path(modem_path)
        .ok_or_else(|| "access_network_modem_path_invalid".to_string())?;
    let (network, cells) = tokio::join!(
        get_network_info_for_modem(conn, &modem_path),
        get_cells_data_for_modem(conn, &modem_path),
    );
    let network =
        network.map_err(|error| format!("access_network_network_query_failed:{error}"))?;
    if !matches!(
        network.registration_status.as_str(),
        "registered" | "roaming" | "attached"
    ) {
        return Err(format!(
            "access_network_not_registered:{}",
            network.registration_status
        ));
    }
    let cells = cells.map_err(|error| format!("access_network_cell_query_failed:{error}"))?;
    let serving = cells.cells.iter().find(|cell| cell.is_serving);
    let technology = if cells.serving_cell.tech.trim().is_empty() {
        serving.map(|cell| cell.tech.as_str()).unwrap_or_default()
    } else {
        cells.serving_cell.tech.as_str()
    };
    let cell_id = if cells.serving_cell.cell_id == 0 {
        serving.map(|cell| cell.cell_id).unwrap_or_default()
    } else {
        cells.serving_cell.cell_id
    };
    let serving_band = serving
        .map(|cell| cell.band.trim())
        .filter(|band| !band.is_empty())
        .map(str::to_string);

    ServingAccessSnapshot::new(
        network.mcc.as_deref().unwrap_or_default(),
        network.mnc.as_deref().unwrap_or_default(),
        technology,
        cell_id,
        cells.serving_cell.tac,
        serving_band,
        AccessNetworkSource::ModemManager,
    )
    .ok_or_else(|| {
        format!(
            "access_network_snapshot_incomplete:tech={};plmn={};cell={};tac={}",
            technology,
            if network.mcc.is_some() && network.mnc.is_some() {
                "present"
            } else {
                "missing"
            },
            if cell_id == 0 { "missing" } else { "present" },
            if cells.serving_cell.tac == 0 {
                "missing"
            } else {
                "present"
            }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modem_path_accepts_full_paths_and_numeric_ids_only() {
        assert_eq!(
            normalize_modem_path("/org/freedesktop/ModemManager1/Modem/7"),
            Some("/org/freedesktop/ModemManager1/Modem/7".to_string())
        );
        assert_eq!(
            normalize_modem_path("7"),
            Some("/org/freedesktop/ModemManager1/Modem/7".to_string())
        );
        assert_eq!(normalize_modem_path("reader:7"), None);
        assert_eq!(normalize_modem_path("7\r\n/evil"), None);
    }
}
