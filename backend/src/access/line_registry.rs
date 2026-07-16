//! Per-modem/SIM runtime registry.
//!
//! Legacy SimAdmin selected the first ModemManager object and stored one global
//! VoLTE runtime. This registry keeps one independent runtime per stable
//! hardware+SIM line while retaining a seed runtime for backwards-compatible
//! single-line API handlers.

use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
};

use serde::Serialize;
use tokio::sync::{Mutex, RwLock as AsyncRwLock};
use zbus::Connection;

use crate::{
    access::volte::{live::VolteLiveHandle, VolteRuntime, VolteRuntimeStatus},
    cellular::modem_manager::{discover_modem_bindings, ModemBinding},
    infra::config::ConfigManager,
    trunk::runtime::{TrunkRuntime, TrunkRuntimeStatus},
};

pub struct LineRuntime {
    binding: RwLock<ModemBinding>,
    pub volte: Arc<VolteRuntime>,
    pub volte_live: VolteLiveHandle,
    pub volte_connect_lock: Mutex<()>,
    pub trunk: Arc<TrunkRuntime>,
}

impl LineRuntime {
    fn new(binding: ModemBinding, volte: Arc<VolteRuntime>, volte_live: VolteLiveHandle) -> Self {
        Self {
            binding: RwLock::new(binding),
            volte,
            volte_live,
            volte_connect_lock: Mutex::new(()),
            trunk: Arc::new(TrunkRuntime::new()),
        }
    }

    pub fn binding(&self) -> ModemBinding {
        self.binding
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn replace_binding(&self, binding: ModemBinding) {
        *self
            .binding
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = binding;
    }

    fn mark_absent(&self) {
        self.binding
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .present = false;
    }

    pub async fn status(&self) -> LineRuntimeStatus {
        LineRuntimeStatus {
            modem: self.binding(),
            volte: self.volte.status().await,
            trunk: self.trunk.status().await,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LineRuntimeStatus {
    pub modem: ModemBinding,
    pub volte: VolteRuntimeStatus,
    pub trunk: TrunkRuntimeStatus,
}

#[derive(Default)]
pub struct LineRuntimeRegistry {
    lines: AsyncRwLock<BTreeMap<String, Arc<LineRuntime>>>,
    seed_runtime: Arc<VolteRuntime>,
    seed_claimed: AtomicBool,
}

impl LineRuntimeRegistry {
    pub fn new(seed_runtime: Arc<VolteRuntime>) -> Self {
        Self {
            lines: AsyncRwLock::new(BTreeMap::new()),
            seed_runtime,
            seed_claimed: AtomicBool::new(false),
        }
    }

    /// Refresh presence and descriptors without discarding per-line runtime
    /// state. Missing lines remain addressable as offline entries so callers
    /// can tear them down and the same SIM can safely reappear after hotplug.
    pub async fn refresh(&self, conn: &Connection) -> zbus::Result<usize> {
        let discovered = discover_modem_bindings(conn).await?;
        let mut lines = self.lines.write().await;
        for line in lines.values() {
            line.mark_absent();
        }
        for binding in discovered {
            if let Some(line) = lines.get(&binding.line_id) {
                line.replace_binding(binding);
                continue;
            }
            let is_seed = !self.seed_claimed.swap(true, Ordering::SeqCst);
            let runtime = if is_seed {
                Arc::clone(&self.seed_runtime)
            } else {
                Arc::new(VolteRuntime::new())
            };
            let live = if is_seed {
                VolteLiveHandle::legacy_shared()
            } else {
                VolteLiveHandle::new()
            };
            lines.insert(
                binding.line_id.clone(),
                Arc::new(LineRuntime::new(binding, runtime, live)),
            );
        }
        Ok(lines.values().filter(|line| line.binding().present).count())
    }

    pub async fn get(&self, line_id: &str) -> Option<Arc<LineRuntime>> {
        self.lines.read().await.get(line_id).cloned()
    }

    pub async fn all(&self) -> Vec<Arc<LineRuntime>> {
        self.lines.read().await.values().cloned().collect()
    }

    pub async fn primary(&self) -> Option<Arc<LineRuntime>> {
        self.lines
            .read()
            .await
            .values()
            .find(|line| line.binding().present)
            .cloned()
    }

    pub async fn for_modem_path(&self, modem_path: &str) -> Option<Arc<LineRuntime>> {
        self.lines
            .read()
            .await
            .values()
            .find(|line| {
                let binding = line.binding();
                binding.present && binding.modem_path == modem_path
            })
            .cloned()
    }

    pub async fn statuses(&self) -> Vec<LineRuntimeStatus> {
        let lines = self
            .lines
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut statuses = Vec::with_capacity(lines.len());
        for line in lines {
            statuses.push(line.status().await);
        }
        statuses
    }

    pub async fn sync_trunk_profiles(&self, config_manager: &ConfigManager) {
        for line in self.all().await {
            let profile = config_manager.get_line_profile(&line.binding().line_id);
            line.trunk.apply_profile(&profile.trunk).await;
        }
    }

    pub async fn present_count(&self) -> usize {
        self.lines
            .read()
            .await
            .values()
            .filter(|line| line.binding().present)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(line_id: &str, present: bool) -> ModemBinding {
        ModemBinding {
            line_id: line_id.to_string(),
            modem_id: "0".to_string(),
            modem_path: "/org/freedesktop/ModemManager1/Modem/0".to_string(),
            manufacturer: "test".to_string(),
            model: "test".to_string(),
            primary_port: "wwan0mbim0".to_string(),
            qmi_device: Some("/dev/wwan0qmi0".to_string()),
            uim_slot: 1,
            sim_path: Some("/org/freedesktop/ModemManager1/SIM/0".to_string()),
            sim_iccid: "8986000000000000000".to_string(),
            operator_id: "46000".to_string(),
            state: "registered".to_string(),
            present,
            hardware_key: "test-hardware".to_string(),
        }
    }

    #[tokio::test]
    async fn line_status_keeps_runtime_and_binding_together() {
        let runtime = Arc::new(VolteRuntime::new());
        let line = LineRuntime::new(binding("line-a", true), runtime, VolteLiveHandle::new());
        let status = line.status().await;
        assert_eq!(status.modem.line_id, "line-a");
        assert_eq!(status.volte.phase, "disabled");
        assert_eq!(status.trunk.phase, "disabled");
    }

    #[test]
    fn absent_transition_does_not_change_stable_identity() {
        let runtime = Arc::new(VolteRuntime::new());
        let line = LineRuntime::new(binding("line-a", true), runtime, VolteLiveHandle::new());
        line.mark_absent();
        assert_eq!(line.binding().line_id, "line-a");
        assert!(!line.binding().present);
    }
}
