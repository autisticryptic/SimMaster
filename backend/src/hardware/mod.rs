//! Hardware domain: everything that talks directly to physical devices or their
//! firmware.
//!
//!   - [`cellular`] — the cellular modem: QMI/mmcli/AT integration for data,
//!     calls, SMS, band/cell lock, and operator registration.
//!   - [`sim`] — eUICC/eSIM profile management (via `lpac`) on the physical SIM
//!     card inserted in the device.

pub mod cellular;
pub mod sim;
