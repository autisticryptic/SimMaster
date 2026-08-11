//! Hardware domain: everything that talks directly to physical devices or their
//! firmware.
//!
//!   - [`cellular`] — the cellular modem: QMI/mmcli/AT integration for data,
//!     calls, SMS, band/cell lock, and operator registration.
//!   - [`devices`] — device-specific drivers (e.g. Qualcomm 410), implementing
//!     the [`devices::transport`] traits that upper layers depend on.
//!   - [`sim`] — eUICC/eSIM profile management (via `lpac`) on the physical SIM
//!     card inserted in the device.

pub mod cellular;
pub mod devices;
pub mod sim;
