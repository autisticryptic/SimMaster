//! Cellular / modem domain: everything that talks to the modem hardware.
//!
//!   - `modem_manager`: ModemManager (D-Bus) + qmicli/mmcli/AT integration —
//!     data connection, calls, SMS, band/cell lock, operator registration
//!   - `cell_lock_store`: in-memory cell-lock UI state
//!   - `serial`: low-level serial/AT port helpers

pub mod cell_lock_store;
pub mod data_proxy;
pub mod modem_manager;
pub mod serial;
