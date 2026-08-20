//! Per-UE network configuration planner (isolation architecture, Option B).
//!
//! The parent process owns bindings, bearer state and the ModemManager
//! session; the worker owns the UE network namespace. This module is the
//! single place that decides *what* the worker should configure there. It is
//! deliberately pure (no I/O) so the generated operation batches are exact and
//! unit-testable without touching a live namespace.

use std::net::{IpAddr, Ipv4Addr};

use crate::{
    platform::{config::UeIsolationConfig, netns::NetnsName},
    services::ue_worker::NetConfigOp,
};

/// Deterministic /30 pair for a UE veth egress, derived from the stable
/// namespace suffix: `10.200.<a>.<b&0xFC>` (host) and `+1` (UE peer).
/// The /16 gives up to 16k independent UE egress pairs before collision.
pub fn veth_addrs_for(namespace: &NetnsName) -> (Ipv4Addr, Ipv4Addr) {
    let suffix = namespace.suffix_hex();
    let mut bytes = [0u8; 2];
    for (index, chunk) in suffix.as_bytes().chunks(2).take(2).enumerate() {
        if let Ok(hex) = std::str::from_utf8(chunk) {
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                bytes[index] = value;
            }
        }
    }
    let host = Ipv4Addr::new(10, 200, bytes[0], bytes[1] & 0xFC);
    let mut ue_octets = host.octets();
    ue_octets[3] = ue_octets[3].saturating_add(1);
    let ue = Ipv4Addr::from(ue_octets);
    (host, ue)
}

/// Resolved names/addresses for one UE veth egress pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UeVethPlan {
    pub host_if: String,
    pub ue_if: String,
    pub host_addr: Ipv4Addr,
    pub ue_addr: Ipv4Addr,
    pub mtu: u32,
}

/// Build the plan for a UE egress veth pair from the isolation config.
pub fn plan_veth(namespace: &NetnsName, config: &UeIsolationConfig) -> UeVethPlan {
    let (host_addr, ue_addr) = veth_addrs_for(namespace);
    UeVethPlan {
        host_if: namespace.host_veth_name(&config.host_veth_prefix),
        ue_if: namespace.ue_veth_name(&config.ue_veth_prefix),
        host_addr,
        ue_addr,
        mtu: config.veth_mtu,
    }
}

/// Operations the worker must apply to its UE side of the egress veth pair.
/// The parent already created the pair, moved the UE peer into the namespace
/// and configured the host side.
pub fn veth_ue_side_ops(plan: &UeVethPlan) -> Vec<NetConfigOp> {
    vec![
        NetConfigOp::AddrReplace {
            ifname: plan.ue_if.clone(),
            cidr: format!("{}/30", plan.ue_addr),
        },
        NetConfigOp::LinkSetUp {
            ifname: plan.ue_if.clone(),
        },
        NetConfigOp::DefaultRouteReplace {
            via: plan.host_addr.to_string(),
            dev: plan.ue_if.clone(),
        },
    ]
}

/// Operations the worker applies to a `wwanX` client backend moved into the
/// UE namespace (VoLTE bearer in a later phase).
pub fn wwan_ue_side_ops(
    ifname: &str,
    addr: IpAddr,
    prefix: u8,
    gateway: Option<IpAddr>,
) -> Vec<NetConfigOp> {
    let mut ops = vec![
        NetConfigOp::AddrReplace {
            ifname: ifname.to_string(),
            cidr: format!("{addr}/{prefix}"),
        },
        NetConfigOp::LinkSetUp {
            ifname: ifname.to_string(),
        },
    ];
    if let Some(gateway) = gateway {
        ops.push(NetConfigOp::DefaultRouteReplace {
            via: gateway.to_string(),
            dev: ifname.to_string(),
        });
    }
    ops
}

/// Operations the worker applies after the VoWiFi TUN interface appeared in
/// the UE namespace: inner address + link up + host routes to every P-CSCF.
/// No host routing policy table is needed here because the namespace is
/// exclusive to one UE.
pub fn tun_ue_side_ops(
    tun_name: &str,
    inner_addr: IpAddr,
    inner_prefix: Option<u8>,
    pcscf_addrs: &[IpAddr],
) -> Vec<NetConfigOp> {
    let prefix = match inner_addr {
        IpAddr::V4(_) => inner_prefix.unwrap_or(32).clamp(1, 32),
        IpAddr::V6(_) => inner_prefix.unwrap_or(128).clamp(1, 128),
    };
    let mut ops = vec![
        NetConfigOp::AddrReplace {
            ifname: tun_name.to_string(),
            cidr: format!("{inner_addr}/{prefix}"),
        },
        NetConfigOp::LinkSetUp {
            ifname: tun_name.to_string(),
        },
    ];
    for pcscf in pcscf_addrs {
        ops.push(NetConfigOp::RouteReplace {
            target: pcscf.to_string(),
            via: None,
            dev: Some(tun_name.to_string()),
            src: Some(inner_addr.to_string()),
            table: None,
        });
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    fn namespace(line_id: &str) -> NetnsName {
        NetnsName::for_line("sa-ue", line_id)
    }

    fn config() -> UeIsolationConfig {
        UeIsolationConfig::default()
    }

    #[test]
    fn veth_addresses_are_deterministic_and_unique() {
        let a = namespace("line-a-11111111111111111111111");
        let b = namespace("line-b-22222222222222222222222");
        let (host_a, ue_a) = veth_addrs_for(&a);
        let (host_a2, ue_a2) = veth_addrs_for(&a);
        let (host_b, ue_b) = veth_addrs_for(&b);

        assert_eq!((host_a, ue_a), (host_a2, ue_a2));
        assert_ne!((host_a, ue_a), (host_b, ue_b));
        assert!(host_a.octets()[0] == 10 && host_a.octets()[1] == 200);
        assert_eq!(ue_a.octets()[3], host_a.octets()[3] + 1);
    }

    #[test]
    fn veth_plan_produces_worker_side_ops() {
        let ns = namespace("line-a-11111111111111111111111");
        let cfg = config();
        let plan = plan_veth(&ns, &cfg);
        let ops = veth_ue_side_ops(&plan);
        assert_eq!(
            ops,
            vec![
                NetConfigOp::AddrReplace {
                    ifname: plan.ue_if.clone(),
                    cidr: format!("{}/30", plan.ue_addr),
                },
                NetConfigOp::LinkSetUp {
                    ifname: plan.ue_if.clone(),
                },
                NetConfigOp::DefaultRouteReplace {
                    via: plan.host_addr.to_string(),
                    dev: plan.ue_if.clone(),
                },
            ]
        );
    }

    #[test]
    fn tun_plan_sets_inner_address_and_pcscf_routes() {
        let ops = tun_ue_side_ops(
            "sa_vwf05",
            IpAddr::V4(Ipv4Addr::new(10, 10, 0, 5)),
            Some(32),
            &[
                IpAddr::V4(Ipv4Addr::new(10, 10, 0, 1)),
                IpAddr::V4(Ipv4Addr::new(10, 10, 0, 2)),
            ],
        );
        assert!(ops.contains(&NetConfigOp::AddrReplace {
            ifname: "sa_vwf05".to_string(),
            cidr: "10.10.0.5/32".to_string(),
        }));
        assert!(ops.contains(&NetConfigOp::LinkSetUp {
            ifname: "sa_vwf05".to_string(),
        }));
        assert!(ops.contains(&NetConfigOp::RouteReplace {
            target: "10.10.0.1".to_string(),
            via: None,
            dev: Some("sa_vwf05".to_string()),
            src: Some("10.10.0.5".to_string()),
            table: None,
        }));
        assert_eq!(ops.len(), 4);
    }

    #[test]
    fn wwan_plan_sets_address_and_default_route() {
        let ops = wwan_ue_side_ops(
            "wwan3",
            IpAddr::V4(Ipv4Addr::new(10, 210, 45, 181)),
            29,
            Some(IpAddr::V4(Ipv4Addr::new(10, 210, 45, 180))),
        );
        assert!(ops.contains(&NetConfigOp::DefaultRouteReplace {
            via: "10.210.45.180".to_string(),
            dev: "wwan3".to_string(),
        }));
    }
}
