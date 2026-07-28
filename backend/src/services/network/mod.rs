//! Network domain: dynamic DNS management and firewall (iptables) control.
//!
//!   - `device_network`: DDNS provider integration + device network config
//!   - `iptables`: firewall rule management
pub mod device_network;
pub mod iptables;
