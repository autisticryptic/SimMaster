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
    Ok(parse_cgcontrdp_settings(&output, cid, apn))
}

/// Parse the full IP configuration (address, gateway, DNS, P-CSCF) for one CID
/// from a `+CGCONTRDP` response.
pub fn parse_cgcontrdp_settings(output: &str, expected_cid: u8, apn: &str) -> CgcontrdpSettings {
    let mut settings = CgcontrdpSettings::default();
    for line in output.lines() {
        let Some((_, values)) = line.split_once("+CGCONTRDP:") else {
            continue;
        };
        let fields: Vec<&str> = values.split(',').map(|field| field.trim()).collect();
        if fields.len() < 4
            || fields[0].parse::<u8>().ok() != Some(expected_cid)
            || !fields
                .get(2)
                .is_some_and(|value| value.trim_matches(['\'', '"']).eq_ignore_ascii_case(apn))
        {
            continue;
        }

        // Field 3: local address and subnet mask, as concatenated octets.
        if let Some((address, prefix)) = parse_cgcontrdp_addr_and_mask(fields[3]) {
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
        // Field 4: gateway.
        if let Some(gateway) = fields
            .get(4)
            .and_then(|f| parse_cgcontrdp_addresses(f).into_iter().next())
        {
            match gateway {
                IpAddr::V4(_) => settings.ipv4_gateway.get_or_insert(gateway),
                IpAddr::V6(_) => settings.ipv6_gateway.get_or_insert(gateway),
            };
        }
        // Fields 5..=6: DNS servers.
        for field in fields.iter().skip(5).take(2) {
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
        // Fields 7..=8: P-CSCF.
        for field in fields.iter().skip(7).take(2) {
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
