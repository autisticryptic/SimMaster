//! IMS PDP context settings read over `AT+CGCONTRDP`.
//!
//! This is the device-agnostic source of truth for an IMS bearer's IP
//! configuration and P-CSCF: address + mask, gateway, DNS and P-CSCF, all on the
//! active IMS context (3GPP TS 27.007). It is shared by the ModemManager path
//! (P-CSCF discovery) and by the device IMS bearer drivers (e.g. the Qualcomm 410
//! native WDS bearer), which is why it lives here rather than under a protocol
//! layer.
//!
//! The 3GPP field layout for one line is:
//! `<cid>,<bearer_id>,<apn>,<local_addr_and_mask>,<gw>,<dns1>,<dns2>,<pcscf1>,<pcscf2>,...`
//! Qualcomm renders the local-address-and-mask field as address octets followed
//! by mask octets (8 decimals for IPv4, 32 for IPv6), which is where the prefix
//! length is recovered from.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use tokio::process::Command;

/// IP configuration and P-CSCF reported for one IMS PDP context.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CgcontrdpSettings {
    pub ipv4_address: Option<IpAddr>,
    pub ipv4_gateway: Option<IpAddr>,
    pub ipv4_dns: Vec<IpAddr>,
    pub ipv4_prefix: Option<u8>,
    pub ipv6_address: Option<IpAddr>,
    pub ipv6_gateway: Option<IpAddr>,
    pub ipv6_dns: Vec<IpAddr>,
    pub ipv6_prefix: Option<u8>,
    pub pcscf: Vec<IpAddr>,
}

/// Failure detail from the `AT+CGCONTRDP` read. `detail` carries a stable string
/// for classification (e.g. `mmcli:...`), kept separate from the structured
/// layers above so an IMS bearer driver can fold it into its own error type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgcontrdpError {
    pub detail: String,
}

impl fmt::Display for CgcontrdpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

/// Read the full IP configuration (address, gateway, DNS, P-CSCF) for one CID
/// from `AT+CGCONTRDP`. This is the primary IMS source, so it reads every field,
/// not just the P-CSCF columns.
pub async fn read_cgcontrdp_settings(
    modem: &str,
    cid: u8,
    apn: &str,
) -> Result<CgcontrdpSettings, CgcontrdpError> {
    let output = run_at(modem, &format!("AT+CGCONTRDP={cid}")).await?;
    // Some Qualcomm firmware puts the two local addresses in separate CSV
    // columns. Do not guess that an opposite-family gateway is a local address:
    // confirm the alternate layout with CGPADDR for this exact active CID.
    let local_addresses = if has_possible_paired_local_columns(&output, cid, apn) {
        run_at(modem, &format!("AT+CGPADDR={cid}"))
            .await
            .map(|response| parse_cgpaddr_addresses(&response, cid))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(parse_cgcontrdp_settings_with_local_addresses(
        &output,
        cid,
        apn,
        &local_addresses,
    ))
}

/// Parse the full IP configuration (address, gateway, DNS, P-CSCF) for one CID
/// from a `+CGCONTRDP` response.
pub fn parse_cgcontrdp_settings(output: &str, expected_cid: u8, apn: &str) -> CgcontrdpSettings {
    parse_cgcontrdp_settings_with_local_addresses(output, expected_cid, apn, &[])
}

fn context_fields<'a>(line: &'a str, expected_cid: u8, apn: &str) -> Option<Vec<&'a str>> {
    let (_, values) = line.split_once("+CGCONTRDP:")?;
    let fields: Vec<&str> = values.split(',').map(str::trim).collect();
    (fields.len() >= 4
        && fields[0].parse::<u8>().ok() == Some(expected_cid)
        && fields[2]
            .trim_matches(['\'', '"'])
            .eq_ignore_ascii_case(apn))
    .then_some(fields)
}

fn paired_local_columns(fields: &[&str]) -> Option<[IpAddr; 2]> {
    // Observed compact IPv4v6 layout:
    // cid,bearer,apn,local4,local6,gw,dns1,dns2,pcscf1v4,pcscf1v6,pcscf2v4,pcscf2v6
    // Requiring both addresses plus the extended columns avoids interpreting
    // an ordinary v4 row's v6 gateway as a second local address.
    if fields.len() < 12 {
        return None;
    }
    let first = parse_cgcontrdp_addr_and_mask(fields[3])?.0;
    let second = parse_cgcontrdp_addr_and_mask(fields[4])?.0;
    (first.is_ipv4() != second.is_ipv4() && !first.is_unspecified() && !second.is_unspecified())
        .then_some([first, second])
}

fn has_possible_paired_local_columns(output: &str, cid: u8, apn: &str) -> bool {
    output.lines().any(|line| {
        context_fields(line, cid, apn)
            .and_then(|fields| paired_local_columns(&fields))
            .is_some()
    })
}

fn parse_cgpaddr_addresses(output: &str, expected_cid: u8) -> Vec<IpAddr> {
    let mut addresses = Vec::new();
    for line in output.lines() {
        let Some((_, values)) = line.split_once("+CGPADDR:") else {
            continue;
        };
        let mut fields = values.split(',').map(str::trim);
        if fields.next().and_then(|cid| cid.parse::<u8>().ok()) != Some(expected_cid) {
            continue;
        }
        for address in fields.take(2).flat_map(parse_cgcontrdp_addresses) {
            if !address.is_unspecified() && !addresses.contains(&address) {
                addresses.push(address);
            }
        }
    }
    addresses
}

fn parse_cgcontrdp_settings_with_local_addresses(
    output: &str,
    expected_cid: u8,
    apn: &str,
    confirmed_local_addresses: &[IpAddr],
) -> CgcontrdpSettings {
    let mut settings = CgcontrdpSettings::default();
    for line in output.lines() {
        let Some(fields) = context_fields(line, expected_cid, apn) else {
            continue;
        };
        let paired = paired_local_columns(&fields).is_some_and(|addresses| {
            addresses
                .iter()
                .all(|address| confirmed_local_addresses.contains(address))
        });
        let gateway_index = if paired { 5 } else { 4 };

        for field in &fields[3..gateway_index] {
            let Some((address, prefix)) = parse_cgcontrdp_addr_and_mask(field) else {
                continue;
            };
            match address {
                IpAddr::V4(_) => {
                    settings.ipv4_address.get_or_insert(address);
                    if settings.ipv4_prefix.is_none() {
                        settings.ipv4_prefix = prefix;
                    }
                }
                IpAddr::V6(_) => {
                    settings.ipv6_address.get_or_insert(address);
                    if settings.ipv6_prefix.is_none() {
                        settings.ipv6_prefix = prefix;
                    }
                }
            }
        }
        if let Some(gateway) = fields
            .get(gateway_index)
            .and_then(|f| parse_cgcontrdp_addresses(f).into_iter().next())
        {
            match gateway {
                IpAddr::V4(_) => settings.ipv4_gateway.get_or_insert(gateway),
                IpAddr::V6(_) => settings.ipv6_gateway.get_or_insert(gateway),
            };
        }
        for field in fields.iter().skip(gateway_index + 1).take(2) {
            for dns in parse_cgcontrdp_addresses(field) {
                let bucket = if dns.is_ipv6() {
                    &mut settings.ipv6_dns
                } else {
                    &mut settings.ipv4_dns
                };
                if !bucket.contains(&dns) {
                    bucket.push(dns);
                }
            }
        }
        for field in fields
            .iter()
            .skip(gateway_index + 3)
            .take(if paired { 4 } else { 2 })
        {
            for pcscf in parse_cgcontrdp_addresses(field) {
                if !settings.pcscf.contains(&pcscf) {
                    settings.pcscf.push(pcscf);
                }
            }
        }
    }
    settings
}

/// Split the `+CGCONTRDP` local-address-and-mask field into an address and a
/// prefix length. IPv4 arrives as 8 octets (4 address + 4 mask), IPv6 as 32
/// octets (16 address + 16 mask); a bare address with no mask yields `None` for
/// the prefix.
fn parse_cgcontrdp_addr_and_mask(field: &str) -> Option<(IpAddr, Option<u8>)> {
    let cleaned = field.trim_matches(|c| c == '\'' || c == '"').trim();
    // A pre-formatted address (with or without an inline /prefix) short-circuits.
    if let Some((addr, prefix)) = cleaned.split_once('/') {
        if let Ok(address) = addr.trim().parse::<IpAddr>() {
            return Some((address, prefix.trim().parse::<u8>().ok()));
        }
    }
    if let Ok(address) = cleaned.parse::<IpAddr>() {
        return Some((address, None));
    }
    let octets: Vec<u8> = cleaned
        .split('.')
        .map(str::parse::<u8>)
        .collect::<Result<_, _>>()
        .ok()?;
    match octets.len() {
        4 => Some((
            IpAddr::V4(Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3])),
            None,
        )),
        8 => {
            let address = IpAddr::V4(Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3]));
            let mask = u32::from_be_bytes([octets[4], octets[5], octets[6], octets[7]]);
            Some((address, prefix_from_mask_bits(mask)))
        }
        16 => {
            let bytes: [u8; 16] = octets.try_into().ok()?;
            Some((IpAddr::V6(Ipv6Addr::from(bytes)), None))
        }
        32 => {
            let addr_bytes: [u8; 16] = octets[..16].try_into().ok()?;
            let mask_bytes: [u8; 16] = octets[16..].try_into().ok()?;
            let ones: u32 = mask_bytes.iter().map(|b| b.count_ones()).sum();
            let contiguous = u128::from_be_bytes(mask_bytes).leading_ones() == ones;
            Some((
                IpAddr::V6(Ipv6Addr::from(addr_bytes)),
                contiguous.then_some(ones as u8),
            ))
        }
        _ => None,
    }
}

/// Convert a 32-bit IPv4 netmask into a prefix length, rejecting discontiguous
/// masks so a wrong on-link prefix is never installed.
fn prefix_from_mask_bits(mask: u32) -> Option<u8> {
    let ones = mask.leading_ones();
    (mask.count_ones() == ones).then_some(ones as u8)
}

pub fn parse_cgcontrdp_addresses(field: &str) -> Vec<IpAddr> {
    field
        .trim_matches(|character| character == '\'' || character == '"')
        .split_whitespace()
        .filter_map(parse_cgcontrdp_address)
        .collect()
}

fn parse_cgcontrdp_address(value: &str) -> Option<IpAddr> {
    if let Ok(address) = value.parse::<IpAddr>() {
        return Some(address);
    }
    let octets: Vec<u8> = value
        .split('.')
        .map(str::parse::<u8>)
        .collect::<Result<_, _>>()
        .ok()?;
    match octets.len() {
        4 => Some(IpAddr::V4(Ipv4Addr::new(
            octets[0], octets[1], octets[2], octets[3],
        ))),
        16 => {
            let bytes: [u8; 16] = octets.try_into().ok()?;
            Some(IpAddr::V6(Ipv6Addr::from(bytes)))
        }
        _ => None,
    }
}

async fn run_at(modem: &str, command: &str) -> Result<String, CgcontrdpError> {
    let argument = format!("--command={command}");
    let output = Command::new("mmcli")
        .args(["-m", modem, &argument])
        .output()
        .await
        .map_err(|error| CgcontrdpError {
            detail: format!("mmcli:{error}"),
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr)
            .trim()
            .replace('\n', " ");
        Err(CgcontrdpError {
            detail: format!(
                "mmcli:{}:-m {modem} {argument}:{stderr}",
                output.status.code().unwrap_or(-1)
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPACT_DUAL: &str =
        "+CGCONTRDP: 2,6,ims,192.0.2.10,32.1.13.184.0.1.0.0.0.0.0.0.0.0.0.10,fe80::1,,,192.0.2.20,2001:db8::20,192.0.2.21,2001:db8::21";

    #[test]
    fn compact_dual_stack_requires_matching_cgpaddr_for_the_same_cid() {
        let confirmed = parse_cgpaddr_addresses(
            "response: '+CGPADDR: 1,192.0.2.99\n+CGPADDR: 2,192.0.2.10,32.1.13.184.0.1.0.0.0.0.0.0.0.0.0.10'",
            2,
        );
        let settings =
            parse_cgcontrdp_settings_with_local_addresses(COMPACT_DUAL, 2, "ims", &confirmed);
        assert_eq!(settings.ipv4_address, Some("192.0.2.10".parse().unwrap()));
        assert_eq!(
            settings.ipv6_address,
            Some("2001:db8:1::a".parse().unwrap())
        );
        assert_eq!(settings.ipv6_gateway, Some("fe80::1".parse().unwrap()));
        assert!(settings.ipv4_dns.is_empty());
        assert!(settings.ipv6_dns.is_empty());
        assert_eq!(
            settings.pcscf,
            ["192.0.2.20", "2001:db8::20", "192.0.2.21", "2001:db8::21"]
                .map(|value| value.parse::<IpAddr>().unwrap())
        );
    }

    #[test]
    fn unconfirmed_or_stale_second_address_is_not_promoted_from_gateway() {
        for confirmed in [
            Vec::new(),
            vec!["192.0.2.10".parse().unwrap()],
            vec![
                "192.0.2.10".parse().unwrap(),
                "2001:db8:1::b".parse().unwrap(),
            ],
            parse_cgpaddr_addresses("+CGPADDR: 1,192.0.2.10,2001:db8:1::a", 2),
        ] {
            let settings =
                parse_cgcontrdp_settings_with_local_addresses(COMPACT_DUAL, 2, "ims", &confirmed);
            assert!(settings.ipv6_address.is_none());
        }
        assert!(has_possible_paired_local_columns(COMPACT_DUAL, 2, "IMS"));
        assert!(!has_possible_paired_local_columns(COMPACT_DUAL, 1, "ims"));
        assert!(!has_possible_paired_local_columns(
            COMPACT_DUAL,
            2,
            "internet"
        ));
    }

    #[test]
    fn standard_single_family_rows_keep_their_existing_layout() {
        let settings = parse_cgcontrdp_settings(
            "+CGCONTRDP: 2,6,\"ims\",192.0.2.10.255.255.255.0,192.0.2.1,192.0.2.53,,192.0.2.20,192.0.2.21\n\
             +CGCONTRDP: 2,6,\"ims\",2001:db8:1::a/64,fe80::1,2001:db8::53,,2001:db8::20,2001:db8::21",
            2,
            "ims",
        );
        assert_eq!(settings.ipv4_address, Some("192.0.2.10".parse().unwrap()));
        assert_eq!(settings.ipv4_prefix, Some(24));
        assert_eq!(
            settings.ipv6_address,
            Some("2001:db8:1::a".parse().unwrap())
        );
        assert_eq!(settings.ipv6_prefix, Some(64));
        assert_eq!(
            settings.ipv4_dns,
            vec!["192.0.2.53".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            settings.ipv6_dns,
            vec!["2001:db8::53".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(settings.pcscf.len(), 4);
    }
}
