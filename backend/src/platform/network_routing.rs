use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteDomain {
    ModemData,
    VolteIms,
    VowifiIms,
}

impl RouteDomain {
    const fn table_base(self) -> u32 {
        match self {
            Self::ModemData => 12_000,
            Self::VolteIms => 14_000,
            Self::VowifiIms => 16_000,
        }
    }

    const fn priority_base(self) -> u32 {
        match self {
            Self::ModemData => 10_000,
            Self::VolteIms => 14_000,
            Self::VowifiIms => 18_000,
        }
    }
}

pub fn route_table(domain: RouteDomain, interface: &str, address: IpAddr) -> u32 {
    domain.table_base() + interface_slot(interface) * 2 + u32::from(address.is_ipv6())
}

pub fn rule_priority(domain: RouteDomain, interface: &str, address: IpAddr) -> u32 {
    domain.priority_base() + interface_slot(interface) * 2 + u32::from(address.is_ipv6())
}

pub fn source_selector(address: IpAddr) -> String {
    format!("{address}/{}", if address.is_ipv6() { 128 } else { 32 })
}

pub fn host_selector(address: IpAddr) -> String {
    source_selector(address)
}

/// Return the network address for an interface address and prefix length.
/// Linux rejects a connected route such as `2.26.79.181/30`; the route must
/// use the network address (`2.26.79.180/30`) while the source rule continues
/// to use the original host address.
pub fn network_address(address: IpAddr, prefix: u8) -> IpAddr {
    match address {
        IpAddr::V4(address) => {
            let prefix = prefix.min(32);
            let bits = u32::from(address);
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - u32::from(prefix))
            };
            IpAddr::V4(Ipv4Addr::from(bits & mask))
        }
        IpAddr::V6(address) => {
            let prefix = prefix.min(128);
            let bits = u128::from(address);
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - u32::from(prefix))
            };
            IpAddr::V6(Ipv6Addr::from(bits & mask))
        }
    }
}

fn interface_slot(interface: &str) -> u32 {
    if let Some(suffix) = interface
        .strip_prefix("wwan")
        .and_then(|value| value.parse::<u32>().ok())
    {
        return suffix.min(999);
    }
    interface.bytes().fold(2_166_136_261u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    }) % 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wwan_tables_are_stable_and_family_separated() {
        let v4 = "10.0.0.2".parse().unwrap();
        let v6 = "2001:db8::2".parse().unwrap();
        assert_eq!(route_table(RouteDomain::ModemData, "wwan0", v4), 12_000);
        assert_eq!(route_table(RouteDomain::VolteIms, "wwan0", v4), 14_000);
        assert_eq!(route_table(RouteDomain::VolteIms, "wwan0", v6), 14_001);
        assert_eq!(route_table(RouteDomain::VolteIms, "wwan7", v4), 14_014);
    }

    #[test]
    fn route_domains_and_line_interfaces_do_not_share_tables() {
        let address = "10.0.0.2".parse().unwrap();
        let data = route_table(RouteDomain::ModemData, "wwan2", address);
        let volte = route_table(RouteDomain::VolteIms, "wwan2", address);
        let vowifi_a = route_table(RouteDomain::VowifiIms, "sa_vwf0c93197", address);
        let vowifi_b = route_table(RouteDomain::VowifiIms, "sa_vwf8a14d20", address);
        assert_ne!(data, volte);
        assert_ne!(volte, vowifi_a);
        assert_ne!(vowifi_a, vowifi_b);
    }

    #[test]
    fn selectors_are_host_scoped() {
        assert_eq!(source_selector("10.0.0.2".parse().unwrap()), "10.0.0.2/32");
        assert_eq!(
            source_selector("2001:db8::2".parse().unwrap()),
            "2001:db8::2/128"
        );
    }

    #[test]
    fn network_address_is_used_for_connected_routes() {
        assert_eq!(
            network_address("2.26.79.181".parse().unwrap(), 30),
            "2.26.79.180".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            network_address("2001:db8::1234".parse().unwrap(), 64),
            "2001:db8::".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            network_address("192.0.2.9".parse().unwrap(), 32),
            "192.0.2.9".parse::<IpAddr>().unwrap()
        );
    }
}
