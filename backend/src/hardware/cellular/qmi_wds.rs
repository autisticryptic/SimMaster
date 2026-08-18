//! Parsers for the QMI WDS text returned by `qmicli`.
//!
//! QCM410 DATA6 owns the live WDS session in
//! `hardware::devices::qcm410::secondary_qmi_data`.  That driver keeps the
//! client id and command sequencing local to the device-specific module; this
//! shared module intentionally contains only the format-neutral parsers used
//! when reading the resulting settings.  The former generic WDS session
//! client was never wired into production and duplicated the QCM410 driver.

/// IP configuration reported by `--wds-get-current-settings`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CurrentSettings {
    pub ip_family: Option<String>,
    pub ipv4_address: Option<String>,
    pub ipv4_gateway: Option<String>,
    pub ipv4_dns: Vec<String>,
    pub ipv4_prefix: Option<u8>,
    pub ipv6_address: Option<String>,
    pub ipv6_gateway: Option<String>,
    pub ipv6_dns: Vec<String>,
    pub ipv6_prefix: Option<u8>,
    pub mtu: Option<u32>,
    pub pcscf: Vec<String>,
}

impl CurrentSettings {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.ipv4_address.is_none() && self.ipv6_address.is_none() && self.pcscf.is_empty()
    }
}

/// Parse the packet-data handle from a successful `--wds-start-network` call.
pub fn parse_packet_data_handle(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (_, value) = line.split_once("Packet data handle:")?;
        let value = value.trim().trim_matches('\'').trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Parse the two qmicli renderings used for the current network settings.
pub fn parse_current_settings(output: &str) -> CurrentSettings {
    let mut settings = CurrentSettings::default();
    for line in output.lines() {
        let Some((label, value)) = line.split_once(':') else {
            continue;
        };
        let label = label.trim().to_ascii_lowercase();
        let value = value.trim().trim_matches('\'').trim().to_string();
        if value.is_empty() || value.eq_ignore_ascii_case("none") {
            continue;
        }
        match label.as_str() {
            "ip family" => settings.ip_family = Some(value.to_ascii_lowercase()),
            "ipv4 address" => settings.ipv4_address = Some(value),
            "ipv4 gateway address" => settings.ipv4_gateway = Some(value),
            "ipv4 primary dns" | "ipv4 secondary dns" => settings.ipv4_dns.push(value),
            "ipv4 subnet mask" => settings.ipv4_prefix = prefix_from_ipv4_mask(&value),
            "ipv6 address" => {
                let (address, prefix) = split_prefix(&value);
                settings.ipv6_address = Some(address);
                settings.ipv6_prefix = prefix;
            }
            "ipv6 gateway address" => settings.ipv6_gateway = Some(split_prefix(&value).0),
            "ipv6 primary dns" | "ipv6 secondary dns" => settings.ipv6_dns.push(value),
            "mtu" => settings.mtu = value.parse().ok(),
            "pcscf address" | "p-cscf address" | "pcscf server address" => {
                settings.pcscf.push(value)
            }
            _ => {}
        }
    }
    settings
}

fn split_prefix(value: &str) -> (String, Option<u8>) {
    match value.split_once('/') {
        Some((address, prefix)) => (
            address.trim().to_string(),
            prefix.trim().parse::<u8>().ok().filter(|bits| *bits <= 128),
        ),
        None => (value.trim().to_string(), None),
    }
}

fn prefix_from_ipv4_mask(mask: &str) -> Option<u8> {
    let octets: Vec<u8> = mask
        .split('.')
        .map(str::trim)
        .map(str::parse::<u8>)
        .collect::<Result<_, _>>()
        .ok()?;
    if octets.len() != 4 {
        return None;
    }
    let bits = u32::from_be_bytes([octets[0], octets[1], octets[2], octets[3]]);
    let ones = bits.leading_ones();
    (bits.count_ones() == ones).then_some(ones as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_packet_handle_and_current_settings() {
        let start = "Packet data handle: '3263198272'";
        assert_eq!(
            parse_packet_data_handle(start).as_deref(),
            Some("3263198272")
        );
        let settings = parse_current_settings(
            "IP Family: IPv4\nIPv4 address: 10.0.0.2\nIPv4 subnet mask: 255.255.255.224\nIPv4 gateway address: 10.0.0.1\nIPv4 primary DNS: 1.1.1.1\nPCSCF address: 10.0.0.3\nMTU: 1500\n",
        );
        assert_eq!(settings.ip_family.as_deref(), Some("ipv4"));
        assert_eq!(settings.ipv4_prefix, Some(27));
        assert_eq!(settings.pcscf, vec!["10.0.0.3"]);
        assert_eq!(settings.mtu, Some(1500));
        assert!(!settings.is_empty());
    }

    #[test]
    fn rejects_non_contiguous_masks_and_splits_ipv6_prefix() {
        assert_eq!(prefix_from_ipv4_mask("255.0.255.0"), None);
        let settings = parse_current_settings("IPv6 address: 2001:db8::2/64\n");
        assert_eq!(settings.ipv6_address.as_deref(), Some("2001:db8::2"));
        assert_eq!(settings.ipv6_prefix, Some(64));
    }
}
