//! Cellular / modem domain: everything that talks to the modem hardware.
//!
//!   - `modem_manager`: ModemManager (D-Bus) + qmicli/mmcli/AT integration —
//!     data connection, calls, SMS, band/cell lock, operator registration
//!   - `qmi_wds`: QMI WDS sessions — a client id held across the several
//!     `qmicli` calls an IMS bearer needs, plus the endpoint capabilities that
//!     decide where such a flow can run at all
//!   - `cell_lock_store`: in-memory cell-lock UI state
//!   - `serial`: low-level serial/AT port helpers
//!
//! Device-specific logic (spare-channel discovery/binding, DATA6 runtime) lives
//! under [`crate::hardware::devices`], keyed by device name.

pub mod cell_lock_store;
pub mod cgcontrdp;
pub mod data_proxy;
pub mod modem_manager;
pub mod qmi_netdev;
pub mod qmi_wds;
pub mod serial;
