//! System domain: OS-level status, events, OTA updates, and device health.
//!
//!   - `ota`: over-the-air firmware/app update download + apply
//!   - `system_event`: structured system-event emitter + severity/status codes
//!   - `system_event_monitor`: background watcher that raises system events
//!   - `device_status`: aggregated device/runtime status reporting

pub mod device_status;
pub mod ota;
pub mod system_event;
pub mod system_event_monitor;
