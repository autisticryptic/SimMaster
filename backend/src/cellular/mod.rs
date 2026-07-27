//! Cellular / modem domain: everything that talks to the modem hardware.
//!
//!   - `modem_manager`: ModemManager (D-Bus) + qmicli/mmcli/AT integration —
//!     data connection, calls, SMS, band/cell lock, operator registration
//!   - `qmi_wds`: QMI WDS sessions — a client id held across the several
//!     `qmicli` calls an IMS bearer needs, plus the endpoint capabilities that
//!     decide where such a flow can run at all
//!   - `secondary_qmi`: discovery/binding of a baseband's spare QMI endpoint,
//!     which carries a plain data session so the primary port is free for IMS
//!   - `cell_lock_store`: in-memory cell-lock UI state
//!   - `serial`: low-level serial/AT port helpers

pub mod cell_lock_store;
pub mod data_proxy;
pub mod modem_manager;
pub mod qmi_netdev;
pub mod qmi_wds;
pub mod secondary_qmi;
pub mod serial;
