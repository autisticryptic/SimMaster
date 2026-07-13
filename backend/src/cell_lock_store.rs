//! In-memory 小区锁定状态（无底层锁网能力时仅会话内展示）。
use crate::models::{CellLockRatStatus, CellLockRequest, CellLockStatusResponse};

#[derive(Debug, Default, Clone)]
pub struct CellLockStore {
    pub lte: Option<(u32, u16)>,
    pub nr: Option<(u32, u16)>,
}

impl CellLockStore {
    pub fn status(&self) -> CellLockStatusResponse {
        let (lte_en, lte_arfcn, lte_pci) = match self.lte {
            Some((arfcn, pci)) => (true, Some(arfcn), Some(pci)),
            None => (false, None, None),
        };
        let (nr_en, nr_arfcn, nr_pci) = match self.nr {
            Some((arfcn, pci)) => (true, Some(arfcn), Some(pci)),
            None => (false, None, None),
        };
        let rat_status = vec![
            CellLockRatStatus {
                rat: 12,
                rat_name: "LTE".to_string(),
                enabled: lte_en,
                lock_type: 0,
                pci: lte_pci,
                arfcn: lte_arfcn,
            },
            CellLockRatStatus {
                rat: 16,
                rat_name: "NR".to_string(),
                enabled: nr_en,
                lock_type: 0,
                pci: nr_pci,
                arfcn: nr_arfcn,
            },
        ];
        CellLockStatusResponse {
            any_locked: self.lte.is_some() || self.nr.is_some(),
            rat_status,
        }
    }

    pub fn apply(&mut self, req: &CellLockRequest) -> Result<(), String> {
        if !req.enable {
            if req.rat == 12 {
                self.lte = None;
            } else if req.rat == 16 {
                self.nr = None;
            }
            return Ok(());
        }
        let arfcn = req.arfcn.ok_or_else(|| "缺少 ARFCN".to_string())?;
        let pci = req.pci.ok_or_else(|| "缺少 PCI".to_string())?;
        if req.rat == 12 {
            self.lte = Some((arfcn, pci));
        } else if req.rat == 16 {
            self.nr = Some((arfcn, pci));
        } else {
            return Err("不支持的 RAT".to_string());
        }
        Ok(())
    }

    pub fn unlock_all(&mut self) {
        self.lte = None;
        self.nr = None;
    }
}
