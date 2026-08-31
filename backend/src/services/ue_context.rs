//! Per-UE context model for the multi-SIM isolation architecture.
//!
//! A UE Context is the stable identity shared by every access leg of one
//! physical line: the SIM slot (or PC/SC reader), the VoLTE bearer, the
//! VoWiFi tunnel, the data proxy and the SIP trunk. Each UE owns its own
//! Linux network namespace, so two SIMs can be
//! handed identical IPv4/IPv6 addresses, P-CSCF addresses, XFRM state or
//! netfilter rules without ever observing each other.

use std::time::Instant;

use serde::Serialize;

use crate::{
    hardware::cellular::modem_manager::ModemBinding,
    platform::netns::{self, NetnsError, NetnsName},
};

/// What kind of physical hardware anchors this UE.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UeKind {
    /// A ModemManager modem/SIM slot that can register on a cell and run
    /// VoLTE/VoWiFi.
    Modem,
    /// A standalone PC/SC card reader (user-space VoWiFi + eSIM management
    /// only).
    PcscReader,
    /// A legacy non-PC/SC UIM adapter anchored by a `/dev/` path.
    LegacyUimAdapter,
}

impl UeKind {
    fn from_binding(binding: &ModemBinding) -> Self {
        if binding.line_kind == "reader" && binding.model.starts_with("pcsc://") {
            Self::PcscReader
        } else if binding.line_kind == "reader" {
            Self::LegacyUimAdapter
        } else {
            Self::Modem
        }
    }
}

/// The runtime identity of one physical line.
///
/// Only the identity (id, slot, namespace) is stored here; the actual data
/// planes live in the per-line runtimes owned by [`LineRuntime`], which all
/// resolve through this context so they can never bind another UE's device.
#[derive(Debug, Clone, Serialize)]
pub struct UeContext {
    /// Stable UE identity; equal to the owning line id.
    pub ue_id: String,
    pub kind: UeKind,
    /// Physical slot selector (never exposed by the HTTP inventory endpoint).
    #[serde(skip)]
    pub hardware_key: String,
    /// Zero-based card slot inside the anchor (modem or reader).
    pub uim_slot: u8,
    /// Linux network namespace owned by this UE. Stable across restarts.
    pub namespace: NetnsName,
    /// True after the namespace was created and loopback brought up in this
    /// process run.
    pub netns_ready: bool,
    #[serde(skip)]
    pub created_at: Instant,
}

impl UeContext {
    /// Build the UE Context for a discovered modem/reader binding.
    pub fn for_binding(binding: &ModemBinding) -> Self {
        let namespace = NetnsName::for_line(netns::DEFAULT_NAMESPACE_PREFIX, &binding.line_id);
        Self {
            ue_id: binding.line_id.clone(),
            kind: UeKind::from_binding(binding),
            hardware_key: binding.hardware_key.clone(),
            uim_slot: binding.uim_slot,
            namespace,
            netns_ready: false,
            created_at: Instant::now(),
        }
    }

    /// Create the UE namespace and bring loopback up.
    pub async fn ensure_netns(&mut self) -> Result<(), NetnsError> {
        // Never carry a previous successful generation's readiness across a
        // failed ensure attempt.  A stale `true` here would make the line
        // registry publish a worker/socket context for a namespace that was
        // removed or could not be recreated, allowing a later caller to bind
        // the wrong network path.
        self.netns_ready = false;
        netns::ensure(&self.namespace).await?;
        self.netns_ready = true;
        Ok(())
    }

    /// Delete the UE namespace. Missing namespaces are not an error.
    pub async fn teardown_netns(&self) -> Result<(), NetnsError> {
        netns::remove(&self.namespace).await
    }

    /// Refresh physical slot identity after a re-discovery. The stable line id
    /// and namespace never change, so hotplug cannot silently redirect this
    /// UE's state to another card.
    pub fn update_binding(&mut self, binding: &ModemBinding) {
        self.kind = UeKind::from_binding(binding);
        self.hardware_key = binding.hardware_key.clone();
        self.uim_slot = binding.uim_slot;
    }

    /// Suggested host-side veth link name for this UE's egress pair.
    pub fn host_veth_name(&self) -> String {
        self.namespace
            .host_veth_name(netns::DEFAULT_HOST_VETH_PREFIX)
    }

    /// Suggested UE-side veth link name for this UE's egress pair.
    pub fn ue_veth_name(&self) -> String {
        self.namespace.ue_veth_name(netns::DEFAULT_UE_VETH_PREFIX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(line_id: &str, line_kind: &str, model: &str) -> ModemBinding {
        ModemBinding {
            line_id: line_id.to_string(),
            display_order: 1,
            slot_label: "基带 1".to_string(),
            slot_source: "test".to_string(),
            slot_stable: true,
            slot_conflict: false,
            modem_id: "0".to_string(),
            modem_path: "/org/freedesktop/ModemManager1/Modem/0".to_string(),
            manufacturer: "test".to_string(),
            model: model.to_string(),
            device_family: "test".to_string(),
            control_transport: "test".to_string(),
            primary_port: String::new(),
            qmi_device: None,
            uim_slot: 1,
            sim_path: None,
            sim_iccid: String::new(),
            operator_id: String::new(),
            state: "registered".to_string(),
            present: true,
            sim_type: "physical".to_string(),
            esim_status: "none".to_string(),
            line_kind: line_kind.to_string(),
            hardware_key: "hw-1".to_string(),
            equipment_identifier: "imei".to_string(),
            legacy_hardware_keys: Vec::new(),
            legacy_line_ids: Vec::new(),
        }
    }

    #[test]
    fn context_tracks_stable_namespace_and_kind() {
        let modem = binding("line-1", "baseband", "qcm410");
        let ue = UeContext::for_binding(&modem);
        assert_eq!(ue.kind, UeKind::Modem);
        assert_eq!(ue.ue_id, "line-1");
        assert!(ue.namespace.as_str().starts_with("sa-ue"));
        assert!(!ue.netns_ready);

        let reader = binding("line-2", "reader", "pcsc://foo");
        let ue = UeContext::for_binding(&reader);
        assert_eq!(ue.kind, UeKind::PcscReader);

        let legacy = binding("line-3", "reader", "at_reader");
        let ue = UeContext::for_binding(&legacy);
        assert_eq!(ue.kind, UeKind::LegacyUimAdapter);
    }

    #[tokio::test]
    #[cfg(not(target_os = "linux"))]
    async fn mandatory_namespace_reports_unsupported_off_linux() {
        let modem = binding("line-1", "baseband", "qcm410");
        let mut ue = UeContext::for_binding(&modem);
        ue.netns_ready = true;
        assert!(matches!(
            ue.ensure_netns().await,
            Err(NetnsError {
                kind: netns::NetnsErrorKind::Unsupported,
                ..
            })
        ));
        assert!(!ue.netns_ready);
    }

    #[test]
    fn veth_names_derive_from_the_ue_namespace() {
        let modem = binding("line-1", "baseband", "qcm410");
        let ue = UeContext::for_binding(&modem);
        let host = ue.host_veth_name();
        let ue_if = ue.ue_veth_name();
        assert!(host.len() < 16 && ue_if.len() < 16);
        assert!(host.starts_with(netns::DEFAULT_HOST_VETH_PREFIX));
        assert!(ue_if.starts_with(netns::DEFAULT_UE_VETH_PREFIX));
    }
}
