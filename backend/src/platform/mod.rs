//! Infrastructure domain: cross-cutting foundations used by every other module.
//!
//!   - `config`: persisted application configuration + the `ConfigManager`
//!   - `config_file`: the hand-editable text file holding main program settings
//!   - `config_store`: per-line/per-slot and event-bound settings in `data.db`
//!   - `db`: SQLite persistence layer (SMS, runtime state, events)
//!   - `utils`: system/network/disk/CPU sampling helpers
//!
//! Configuration deliberately lives in two places. Settings an operator may
//! want to read or edit by hand are in the text file; anything the program
//! rewrites on its own — per-UE line profiles, modem/reader slot maps,
//! notification and automation records — is in `data.db`, because hardware
//! discovery rewrites those on every hotplug and would churn the text file.

pub mod config;
pub mod config_file;
pub mod config_maintenance;
pub mod config_store;
pub mod db;
pub mod netns;
pub mod network_routing;
pub mod shutdown;
pub mod utils;
