//! Typed IP configuration of one owned ModemManager IMS bearer.
//!
//! MM's QMI provider reads Current Settings on its own retained WDS client and
//! publishes address/prefix/gateway/dns1..dns3 in Ip4Config/Ip6Config. AT output
//! is not an equivalent source: firmware may omit the DNS fields there. Never
//! invent P-CSCF from an IP-config field; MM 1.18's bearer API has no such key.

use std::{collections::HashMap, net::IpAddr};

use zbus::zvariant::OwnedValue;

use crate::hardware::cellular::cgcontrdp::CgcontrdpSettings;

pub(super) type Properties = HashMap<String, OwnedValue>;

fn invalid(field: &str) -> String {
    // The field is a code constant, never an untrusted value or credential.
    format!("qca410_primary_mm_ip_config_invalid:{field}")
}

fn dictionary(properties: &Properties, key: &str) -> Result<Properties, String> {
    let value = properties.get(key).ok_or_else(|| invalid(key))?;
    value
        .try_clone()
        .and_then(Properties::try_from)
        .map_err(|_| invalid(key))
}

fn string<'a>(properties: &'a Properties, key: &str) -> Result<Option<&'a str>, String> {
    properties
        .get(key)
        .map(|value| <&str>::try_from(value).map_err(|_| invalid(key)))
        .transpose()
}

fn number(properties: &Properties, key: &str) -> Result<Option<u32>, String> {
    properties
        .get(key)
        .map(|value| u32::try_from(value).map_err(|_| invalid(key)))
        .transpose()
}

/// Revalidate status both in the GetAll snapshot and after the asynchronous
/// read. A matching APN alone does not identify an owned, connected interface.
pub(super) fn validate_binding(
    connected: bool,
    interface: &str,
    apn: &str,
    expected_interface: &str,
    expected_apn: &str,
) -> Result<(), String> {
    if !connected {
        return Err("qca410_primary_mm_bearer_not_connected".to_string());
    }
    if interface != expected_interface {
        return Err("qca410_primary_mm_data_interface_mismatch".to_string());
    }
    if !apn.eq_ignore_ascii_case(expected_apn) {
        return Err("qca410_primary_mm_apn_mismatch".to_string());
    }
    Ok(())
}

fn address(
    properties: &Properties,
    key: &str,
    family: u8,
    local: bool,
) -> Result<Option<IpAddr>, String> {
    let Some(text) = string(properties, key)?.filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    let address: IpAddr = text.parse().map_err(|_| invalid(key))?;
    if address.is_ipv4() != (family == 4)
        || address.is_loopback()
        || address.is_multicast()
        || matches!(address, IpAddr::V4(value) if value.is_broadcast())
        || matches!(address, IpAddr::V6(value) if value.to_ipv4_mapped().is_some())
    {
        return Err(invalid(key));
    }
    // Optional gateway/DNS zero values mean "not supplied", not a usable route.
    if address.is_unspecified() {
        return if local { Err(invalid(key)) } else { Ok(None) };
    }
    if local
        && match address {
            IpAddr::V4(value) => value.is_link_local(),
            IpAddr::V6(value) => value.is_unicast_link_local(),
        }
    {
        // DHCP/SLAAC link-local bootstrap is not a granted IMS source address.
        return Err(invalid(key));
    }
    Ok(Some(address))
}

/// Parse a single GetAll(Bearer) response, selecting only the family actually
/// started. None is a not-yet-published IP configuration; malformed properties
/// and unsupported assignment methods are errors, never a reason to guess AT
/// addressing or silently start DHCP/PPP in the host namespace.
pub(super) fn parse(
    properties: &Properties,
    expected_interface: &str,
    expected_apn: &str,
    family: u8,
) -> Result<Option<CgcontrdpSettings>, String> {
    if !matches!(family, 4 | 6) {
        return Err("qca410_primary_mm_requires_explicit_ip_family".to_string());
    }
    let connected = properties
        .get("Connected")
        .ok_or_else(|| invalid("Connected"))
        .and_then(|value| bool::try_from(value).map_err(|_| invalid("Connected")))?;
    let interface = string(properties, "Interface")?.ok_or_else(|| invalid("Interface"))?;
    let bearer_properties = dictionary(properties, "Properties")?;
    let apn = string(&bearer_properties, "apn")?.ok_or_else(|| invalid("apn"))?;
    validate_binding(connected, interface, apn, expected_interface, expected_apn)?;

    let key = if family == 4 {
        "Ip4Config"
    } else {
        "Ip6Config"
    };
    let config = dictionary(properties, key)?;
    if config.is_empty() {
        return Ok(None);
    }
    match number(&config, "method")? {
        Some(0) => return Ok(None), // MM_BEARER_IP_METHOD_UNKNOWN
        Some(2) => {}               // MM_BEARER_IP_METHOD_STATIC
        Some(_) => return Err("qca410_primary_mm_ip_method_unsupported".to_string()),
        None => return Err(invalid("method")),
    }
    let local = address(&config, "address", family, true)?;
    let prefix = number(&config, "prefix")?;
    if prefix.is_some_and(|prefix| prefix > if family == 4 { 32 } else { 128 }) {
        return Err(invalid("prefix"));
    }
    let gateway = address(&config, "gateway", family, false)?;
    let mut dns = Vec::new();
    for key in ["dns1", "dns2", "dns3"] {
        if let Some(server) = address(&config, key, family, false)? {
            if !dns.contains(&server) {
                dns.push(server);
            }
        }
    }
    let (Some(local), Some(prefix)) = (local, prefix) else {
        return Ok(None);
    };
    // This existing plain data container is shared with the AT parser. Its
    // historical name does not imply that these fields came from CGCONTRDP.
    let mut settings = CgcontrdpSettings::default();
    if family == 4 {
        settings.ipv4_address = Some(local);
        settings.ipv4_prefix = Some(prefix as u8);
        settings.ipv4_gateway = gateway;
        settings.ipv4_dns = dns;
    } else {
        settings.ipv6_address = Some(local);
        settings.ipv6_prefix = Some(prefix as u8);
        settings.ipv6_gateway = gateway;
        settings.ipv6_dns = dns;
    }
    Ok(Some(settings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    fn text(value: &str) -> OwnedValue {
        OwnedValue::try_from(Value::from(value)).unwrap()
    }

    fn config(family: u8) -> Properties {
        let (local, gateway, dns, prefix) = if family == 4 {
            ("192.0.2.2", "192.0.2.1", "192.0.2.53", 30_u32)
        } else {
            ("2001:db8::2", "2001:db8::1", "2001:db8::53", 64_u32)
        };
        Properties::from([
            ("method".into(), OwnedValue::from(2_u32)),
            ("address".into(), text(local)),
            ("prefix".into(), OwnedValue::from(prefix)),
            ("gateway".into(), text(gateway)),
            ("dns1".into(), text(dns)),
            ("dns2".into(), text(dns)),
        ])
    }

    fn snapshot_with(family: u8, config: Properties) -> Properties {
        Properties::from([
            ("Connected".into(), OwnedValue::from(true)),
            ("Interface".into(), text("wwan0")),
            (
                "Properties".into(),
                OwnedValue::from(Properties::from([("apn".into(), text("ims"))])),
            ),
            (
                (if family == 4 {
                    "Ip4Config"
                } else {
                    "Ip6Config"
                })
                .into(),
                OwnedValue::from(config),
            ),
        ])
    }

    fn parse_config(family: u8, config: Properties) -> Result<Option<CgcontrdpSettings>, String> {
        parse(&snapshot_with(family, config), "wwan0", "ims", family)
    }

    #[test]
    fn mm_static_ipv4_and_ipv6_settings_preserve_dns_and_prefix() {
        let v4 = parse_config(4, config(4)).unwrap().unwrap();
        assert_eq!(v4.ipv4_address, Some("192.0.2.2".parse().unwrap()));
        assert_eq!(v4.ipv4_prefix, Some(30));
        assert_eq!(v4.ipv4_gateway, Some("192.0.2.1".parse().unwrap()));
        assert_eq!(v4.ipv4_dns, vec!["192.0.2.53".parse::<IpAddr>().unwrap()]);
        assert!(v4.ipv6_address.is_none());
        let mut ipv6 = config(6);
        ipv6.insert("dns3".into(), text("2001:db8::54"));
        let v6 = parse_config(6, ipv6).unwrap().unwrap();
        assert_eq!(v6.ipv6_prefix, Some(64));
        assert_eq!(v6.ipv6_dns.len(), 2);
        assert!(v6.ipv4_address.is_none());
        assert!(v6.pcscf.is_empty(), "DNS must not be relabelled P-CSCF");
    }

    #[test]
    fn only_the_started_family_is_consumed() {
        let mut snapshot = snapshot_with(6, config(6));
        snapshot.insert("Ip4Config".into(), text("unstarted stale data"));
        assert!(parse(&snapshot, "wwan0", "ims", 6).unwrap().is_some());
        assert!(parse(&snapshot, "wwan0", "ims", 0).is_err());
    }

    #[test]
    fn unpublished_config_is_pending_not_an_at_fallback() {
        assert!(parse_config(6, Properties::new()).unwrap().is_none());
        let mut pending = config(6);
        pending.insert("method".into(), OwnedValue::from(0_u32));
        assert!(parse_config(6, pending).unwrap().is_none());
        let mut pending = config(6);
        pending.remove("address");
        assert!(parse_config(6, pending).unwrap().is_none());
        let mut pending = config(6);
        pending.remove("prefix");
        assert!(parse_config(6, pending).unwrap().is_none());
    }

    #[test]
    fn ppp_dhcp_and_unknown_methods_do_not_become_static_ims() {
        for method in [1_u32, 3, 99] {
            let mut invalid = config(6);
            invalid.insert("method".into(), OwnedValue::from(method));
            assert_eq!(
                parse_config(6, invalid).unwrap_err(),
                "qca410_primary_mm_ip_method_unsupported"
            );
        }
    }

    #[test]
    fn wrong_types_and_invalid_prefixes_fail_closed_without_value_disclosure() {
        for (key, value) in [
            ("method", text("2")),
            ("prefix", OwnedValue::from(129_u32)),
            ("prefix", OwnedValue::from(-1_i32)),
            ("dns1", OwnedValue::from(9_u32)),
            ("address", text("subscriber-secret")),
        ] {
            let mut bad = config(6);
            bad.insert(key.into(), value);
            let error = parse_config(6, bad).unwrap_err();
            assert!(error.starts_with("qca410_primary_mm_ip_config_invalid:"));
            assert!(!error.contains("subscriber-secret"));
        }
    }

    #[test]
    fn foreign_family_and_unusable_local_addresses_are_rejected() {
        for key in ["address", "gateway", "dns1"] {
            let mut bad = config(6);
            bad.insert(key.into(), text("192.0.2.9"));
            assert!(parse_config(6, bad).is_err());
        }
        for value in ["::", "::1", "ff02::1", "fe80::2", "::ffff:192.0.2.2"] {
            let mut bad = config(6);
            bad.insert("address".into(), text(value));
            assert!(parse_config(6, bad).is_err());
        }
    }

    #[test]
    fn missing_or_zero_optional_network_fields_do_not_invent_servers() {
        let mut minimal = config(6);
        minimal.remove("gateway");
        minimal.insert("dns1".into(), text("::"));
        minimal.insert("dns2".into(), text(""));
        let settings = parse_config(6, minimal).unwrap().unwrap();
        assert!(settings.ipv6_gateway.is_none());
        assert!(settings.ipv6_dns.is_empty());
        assert!(settings.pcscf.is_empty());
    }

    #[test]
    fn snapshot_requires_the_expected_connected_apn_and_interface() {
        let valid = snapshot_with(6, config(6));
        assert!(parse(&valid, "wwan1", "ims", 6).is_err());
        assert!(parse(&valid, "wwan0", "internet", 6).is_err());
        assert!(parse(&valid, "wwan0", "IMS", 6).is_ok());
        let mut disconnected = snapshot_with(6, config(6));
        disconnected.insert("Connected".into(), OwnedValue::from(false));
        assert!(parse(&disconnected, "wwan0", "ims", 6).is_err());
        let mut wrong_dict = snapshot_with(6, config(6));
        wrong_dict.insert("Properties".into(), text("not a dictionary"));
        assert!(parse(&wrong_dict, "wwan0", "ims", 6).is_err());
    }
}
