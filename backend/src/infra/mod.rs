//! Infrastructure domain: cross-cutting foundations used by every other module.
//!
//!   - `config`: persisted application configuration + the `ConfigManager`
//!   - `db`: SQLite persistence layer (SMS, runtime state, events)
//!   - `utils`: system/network/disk/CPU sampling helpers

pub mod config;
pub mod db;
pub mod utils;
