//! VoLTE IMS APN bearer management via ModemManager.
//!
//! Clean-room from 3GPP TS 24.301 (EPS bearers) + the ModemManager D-Bus API.
//! The reference "borrows" bearer setup from ModemManager: it starts an IMS APN
//! bearer (`apn=ims`), reads the assigned IP/PCO settings, and cleans up stale
//! disconnected bearers. The heavy IO lives behind `#[cfg(unix)]`; the pure
//! bits (APN selection, bearer object-path validation, roaming policy) are
//! testable everywhere.
//!
//! Observed anchors: `--wds-start-network=apn=ims,3gpp-profile=`,
//! `SIMADMIN_MM_IMS_BEARER`, `/org/freedesktop/ModemManager1/Bearer/`,
//! `recreating IMS bearer to match roaming policy`,
//! `Deleted stale disconnected IMS bearer`.

use std::{net::IpAddr, process::Output};

use tokio::process::Command;

use super::{
    errors::{code, VolteError},
    pcscf::ImsIpSettings,
};

/// The IMS APN used for the dedicated IMS bearer.
pub const IMS_APN: &str = "ims";

/// Environment override for the bearer object path (matches the reference's
/// `SIMADMIN_MM_IMS_BEARER`), letting an operator/tester pin a specific bearer.
pub const BEARER_ENV: &str = "SIMADMIN_MM_IMS_BEARER";

/// ModemManager bearer object-path prefix.
pub const BEARER_PATH_PREFIX: &str = "/org/freedesktop/ModemManager1/Bearer/";

/// Requested IMS bearer parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerRequest {
    pub apn: String,
    pub allow_roaming: bool,
    /// Optional 3GPP profile id (as in `--wds-start-network=...,3gpp-profile=<n>`).
    pub profile_id: Option<u32>,
}

/// Connected IMS bearer details returned by ModemManager. These values are
/// enough to configure only the dedicated WWAN link without touching the
/// host's normal default route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerConnection {
    pub path: String,
    pub interface: String,
    pub settings: ImsIpSettings,
    pub ipv4_prefix: Option<u8>,
    pub ipv6_prefix: Option<u8>,
    pub mtu: Option<u32>,
}

impl BearerConnection {
    pub fn local_addr(&self) -> Result<IpAddr, VolteError> {
        self.settings
            .local_addr()
            .ok_or_else(|| VolteError::new(code::IP_SETTINGS_MISSING))
    }
}

impl Default for BearerRequest {
    fn default() -> Self {
        Self {
            apn: IMS_APN.to_string(),
            allow_roaming: false,
            profile_id: None,
        }
    }
}

impl BearerRequest {
    /// Build the `--wds-start-network` argument observed in the reference.
    pub fn wds_start_network_arg(&self) -> String {
        match self.profile_id {
            Some(id) => format!("--wds-start-network=apn={},3gpp-profile={}", self.apn, id),
            None => format!("--wds-start-network=apn={}", self.apn),
        }
    }
}

/// Validate a ModemManager bearer object path.
pub fn is_valid_bearer_path(path: &str) -> bool {
    path.starts_with(BEARER_PATH_PREFIX)
        && path[BEARER_PATH_PREFIX.len()..]
            .chars()
            .all(|c| c.is_ascii_digit())
        && path.len() > BEARER_PATH_PREFIX.len()
}

/// Read the bearer-path override from the environment, if set and valid.
pub fn bearer_path_override() -> Option<String> {
    let raw = std::env::var(BEARER_ENV).ok()?;
    let trimmed = raw.trim();
    if is_valid_bearer_path(trimmed) {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// Roaming gate: if the network is roaming and roaming is not allowed, the IMS
/// bearer must not be brought up (mirrors `bearer_roaming_forbidden`).
pub fn check_roaming(is_roaming: bool, allow_roaming: bool) -> Result<(), VolteError> {
    if is_roaming && !allow_roaming {
        return Err(VolteError::new(code::RUNTIME_MM_BEARER_ROAMING_FORBIDDEN));
    }
    Ok(())
}

/// Create or reuse an `apn=ims` ModemManager bearer and connect it. Existing
/// non-IMS bearers are never changed or deleted.
pub async fn ensure_ims_bearer(
    modem: &str,
    request: &BearerRequest,
) -> Result<BearerConnection, VolteError> {
    if let Some(path) = bearer_path_override() {
        return connect_and_read(&path).await;
    }

    let modem_output = run_command("mmcli", &["-m", modem, "--output-keyvalue"]).await?;
    for path in parse_bearer_paths(&modem_output) {
        let details = run_command("mmcli", &["-b", &path, "--output-keyvalue"]).await?;
        if value(&details, "bearer.properties.apn").as_deref() == Some(IMS_APN) {
            return connect_and_read(&path).await;
        }
    }

    let properties = format!(
        "apn={},ip-type=ipv4v6,allow-roaming={}",
        request.apn,
        if request.allow_roaming { "yes" } else { "no" }
    );
    let created = run_command(
        "mmcli",
        &["-m", modem, &format!("--create-bearer={properties}")],
    )
    .await?;
    let path = parse_created_bearer_path(&created)
        .ok_or_else(|| VolteError::new(code::RUNTIME_MM_BEARER_PATH_MISSING))?;
    connect_and_read(&path).await
}

/// Disconnect an IMS bearer left active by a previous process before the
/// Qualcomm AT P-CSCF probe temporarily reuses its PDP context.  Keep the
/// bearer object itself so `ensure_ims_bearer` can reconnect it afterwards.
pub async fn disconnect_existing_ims_bearers(modem: &str) -> Result<(), VolteError> {
    let modem_output = run_command("mmcli", &["-m", modem, "--output-keyvalue"]).await?;
    for path in parse_bearer_paths(&modem_output) {
        let details = run_command("mmcli", &["-b", &path, "--output-keyvalue"]).await?;
        if value(&details, "bearer.properties.apn").as_deref() == Some(IMS_APN)
            && value(&details, "bearer.status.connected").as_deref() == Some("yes")
        {
            run_command("mmcli", &["-b", &path, "--disconnect"])
                .await
                .map_err(|error| {
                    VolteError::with_detail(
                        code::RUNTIME_MM_BEARER_CONNECT_FAILED,
                        format!("disconnect_before_pcscf:{error}"),
                    )
                })?;
        }
    }
    Ok(())
}

async fn connect_and_read(path: &str) -> Result<BearerConnection, VolteError> {
    let before = run_command("mmcli", &["-b", path, "--output-keyvalue"]).await?;
    if value(&before, "bearer.status.connected").as_deref() != Some("yes") {
        run_command("mmcli", &["-b", path, "--connect"])
            .await
            .map_err(|error| {
                VolteError::with_detail(code::RUNTIME_MM_BEARER_CONNECT_FAILED, error.to_string())
            })?;
    }
    let connected = run_command("mmcli", &["-b", path, "--output-keyvalue"]).await?;
    parse_bearer_connection(path, &connected)
}

pub fn parse_bearer_connection(
    path: &str,
    output: &str,
) -> Result<BearerConnection, VolteError> {
    if value(output, "bearer.status.connected").as_deref() != Some("yes") {
        return Err(VolteError::new(code::RUNTIME_MM_BEARER_NOT_CONNECTED));
    }
    let interface = value(output, "bearer.status.interface")
        .filter(|item| item != "--")
        .ok_or_else(|| VolteError::new(code::IP_SETTINGS_MISSING))?;
    let mut settings = ImsIpSettings::default();
    settings.ipv4_address = ip_value(output, "bearer.ipv4-config.address");
    settings.ipv4_gateway = ip_value(output, "bearer.ipv4-config.gateway");
    settings.ipv4_dns = list_ip_values(output, "bearer.ipv4-config.dns.value");
    settings.ipv6_address = ip_value(output, "bearer.ipv6-config.address");
    settings.ipv6_gateway = ip_value(output, "bearer.ipv6-config.gateway");
    settings.ipv6_dns = list_ip_values(output, "bearer.ipv6-config.dns.value");
    let ipv4_prefix = number_value(output, "bearer.ipv4-config.prefix");
    let ipv6_prefix = number_value(output, "bearer.ipv6-config.prefix");
    let mtu = number_value(output, "bearer.ipv6-config.mtu")
        .or_else(|| number_value(output, "bearer.ipv4-config.mtu"));
    if settings.local_addr().is_none() {
        return Err(VolteError::new(code::IP_SETTINGS_MISSING));
    }
    Ok(BearerConnection {
        path: path.to_string(),
        interface,
        settings,
        ipv4_prefix,
        ipv6_prefix,
        mtu,
    })
}

/// Configure the address and DNS host routes for the dedicated bearer. No
/// default route is added, preserving the management/Wi-Fi path.
pub async fn configure_bearer_network(bearer: &BearerConnection) -> Result<(), VolteError> {
    run_ip(&["link", "set", "dev", &bearer.interface, "up"]).await?;
    if let Some(mtu) = bearer.mtu {
        let mtu = mtu.to_string();
        run_ip(&[
            "link",
            "set",
            "dev",
            &bearer.interface,
            "mtu",
            &mtu,
        ])
        .await?;
    }

    let mut configured = false;
    if let Some(IpAddr::V6(address)) = bearer.settings.ipv6_address {
        let prefix = bearer.ipv6_prefix.unwrap_or(64);
        let address = format!("{address}/{prefix}");
        run_ip(&[
            "-6",
            "address",
            "replace",
            &address,
            "dev",
            &bearer.interface,
        ])
        .await?;
        for dns in &bearer.settings.ipv6_dns {
            route_host(&bearer.interface, *dns).await?;
        }
        configured = true;
    }
    if let Some(IpAddr::V4(address)) = bearer.settings.ipv4_address {
        let prefix = bearer.ipv4_prefix.unwrap_or(32);
        let address = format!("{address}/{prefix}");
        run_ip(&[
            "address",
            "replace",
            &address,
            "dev",
            &bearer.interface,
        ])
        .await?;
        for dns in &bearer.settings.ipv4_dns {
            route_host(&bearer.interface, *dns).await?;
        }
        configured = true;
    }
    if !configured {
        return Err(VolteError::new(code::IP_SETTINGS_MISSING));
    }
    Ok(())
}

pub async fn route_pcscf(
    bearer: &BearerConnection,
    pcscf: IpAddr,
) -> Result<(), VolteError> {
    route_host(&bearer.interface, pcscf).await
}

async fn route_host(interface: &str, host: IpAddr) -> Result<(), VolteError> {
    let (family, suffix) = if host.is_ipv6() { (Some("-6"), 128) } else { (None, 32) };
    let destination = format!("{host}/{suffix}");
    let mut args = Vec::new();
    if let Some(family) = family {
        args.push(family);
    }
    args.extend_from_slice(&[
        "route",
        "replace",
        &destination,
        "dev",
        interface,
    ]);
    run_ip(&args).await.map(|_| ())
}

/// Remove network state only from the dedicated bearer interface. This is
/// used on failed registration and normal teardown so stale IPv6 addresses or
/// host routes cannot accumulate across long-running retries.
pub async fn teardown_bearer_network(bearer: &BearerConnection) {
    let _ = run_ip(&["-6", "route", "flush", "dev", &bearer.interface]).await;
    let _ = run_ip(&["route", "flush", "dev", &bearer.interface]).await;
    let _ = run_ip(&["address", "flush", "dev", &bearer.interface]).await;
    let _ = run_ip(&["link", "set", "dev", &bearer.interface, "down"]).await;
}

pub async fn disconnect_bearer(path: &str) {
    let _ = run_command("mmcli", &["-b", path, "--disconnect"]).await;
}

async fn run_ip(args: &[&str]) -> Result<String, VolteError> {
    run_command("ip", args).await
}

async fn run_command(program: &str, args: &[&str]) -> Result<String, VolteError> {
    let output = Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|error| {
            VolteError::with_detail(code::COMMAND_SPAWN_FAILED, format!("{program}:{error}"))
        })?;
    command_output(program, output)
}

fn command_output(program: &str, output: Output) -> Result<String, VolteError> {
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(VolteError::with_detail(
            code::COMMAND_FAILED,
            format!("{program}:{}", output.status.code().unwrap_or(-1)),
        ))
    }
}

fn parse_bearer_paths(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(key, _)| key.trim().starts_with("modem.generic.bearers.value"))
        .map(|(_, value)| value.trim().to_string())
        .filter(|path| is_valid_bearer_path(path))
        .collect()
}

fn parse_created_bearer_path(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|item| is_valid_bearer_path(item.trim()))
        .map(|item| item.trim().to_string())
}

fn value(output: &str, key: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (candidate, value) = line.split_once(':')?;
        (candidate.trim() == key).then(|| value.trim().to_string())
    })
}

fn number_value<T>(output: &str, key: &str) -> Option<T>
where
    T: std::str::FromStr,
{
    value(output, key).and_then(|item| item.parse().ok())
}

fn ip_value(output: &str, key: &str) -> Option<IpAddr> {
    value(output, key)
        .filter(|item| item != "--")
        .and_then(|item| item.parse().ok())
}

fn list_ip_values(output: &str, key_prefix: &str) -> Vec<IpAddr> {
    output
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(key, _)| key.trim().starts_with(key_prefix))
        .filter_map(|(_, value)| value.trim().parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wds_arg_with_and_without_profile() {
        let r = BearerRequest::default();
        assert_eq!(r.wds_start_network_arg(), "--wds-start-network=apn=ims");
        let r2 = BearerRequest {
            apn: "ims".to_string(),
            allow_roaming: true,
            profile_id: Some(3),
        };
        assert_eq!(
            r2.wds_start_network_arg(),
            "--wds-start-network=apn=ims,3gpp-profile=3"
        );
    }

    #[test]
    fn bearer_path_validation() {
        assert!(is_valid_bearer_path(
            "/org/freedesktop/ModemManager1/Bearer/1"
        ));
        assert!(is_valid_bearer_path(
            "/org/freedesktop/ModemManager1/Bearer/42"
        ));
        assert!(!is_valid_bearer_path(
            "/org/freedesktop/ModemManager1/Bearer/"
        ));
        assert!(!is_valid_bearer_path(
            "/org/freedesktop/ModemManager1/Bearer/abc"
        ));
        assert!(!is_valid_bearer_path("/wrong/prefix/1"));
    }

    #[test]
    fn roaming_gate() {
        assert!(check_roaming(false, false).is_ok());
        assert!(check_roaming(true, true).is_ok());
        assert!(check_roaming(false, true).is_ok());
        assert_eq!(
            check_roaming(true, false).unwrap_err().code(),
            code::RUNTIME_MM_BEARER_ROAMING_FORBIDDEN
        );
    }

    #[test]
    fn default_request_uses_ims_apn() {
        assert_eq!(BearerRequest::default().apn, "ims");
        assert!(!BearerRequest::default().allow_roaming);
    }

    #[test]
    fn parses_connected_ipv6_only_bearer_without_default_route_assumptions() {
        let output = r#"
bearer.status.connected                  : yes
bearer.status.interface                  : wwan0
bearer.properties.apn                    : ims
bearer.ipv4-config.address               : --
bearer.ipv6-config.address               : 2001:db8:1::20
bearer.ipv6-config.prefix                : 64
bearer.ipv6-config.gateway               : 2001:db8:1::1
bearer.ipv6-config.dns.length            : 2
bearer.ipv6-config.dns.value[1]          : 2001:db8:53::1
bearer.ipv6-config.dns.value[2]          : 2001:db8:53::2
bearer.ipv6-config.mtu                   : 1500
"#;
        let bearer = parse_bearer_connection(
            "/org/freedesktop/ModemManager1/Bearer/2",
            output,
        )
        .unwrap();
        assert_eq!(bearer.interface, "wwan0");
        assert!(bearer.local_addr().unwrap().is_ipv6());
        assert_eq!(bearer.settings.ipv6_dns.len(), 2);
        assert_eq!(bearer.ipv6_prefix, Some(64));
        assert_eq!(bearer.mtu, Some(1500));
    }

    #[test]
    fn parses_dual_stack_bearer_without_dropping_either_family() {
        let output = r#"
bearer.status.connected                  : yes
bearer.status.interface                  : wwan0
bearer.properties.apn                    : ims
bearer.ipv4-config.address               : 10.23.4.5
bearer.ipv4-config.prefix                : 30
bearer.ipv4-config.gateway               : 10.23.4.6
bearer.ipv4-config.dns.value[1]          : 10.23.4.53
bearer.ipv6-config.address               : 2001:db8:1::20
bearer.ipv6-config.prefix                : 64
bearer.ipv6-config.gateway               : 2001:db8:1::1
bearer.ipv6-config.dns.value[1]          : 2001:db8:53::1
bearer.ipv6-config.mtu                   : 1428
"#;
        let bearer = parse_bearer_connection(
            "/org/freedesktop/ModemManager1/Bearer/9",
            output,
        )
        .unwrap();

        assert_eq!(
            bearer.settings.ipv4_address,
            Some("10.23.4.5".parse::<IpAddr>().unwrap())
        );
        assert_eq!(
            bearer.settings.ipv6_address,
            Some("2001:db8:1::20".parse::<IpAddr>().unwrap())
        );
        assert_eq!(
            bearer.settings.ipv4_dns,
            vec!["10.23.4.53".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            bearer.settings.ipv6_dns,
            vec!["2001:db8:53::1".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(bearer.ipv4_prefix, Some(30));
        assert_eq!(bearer.ipv6_prefix, Some(64));
        assert_eq!(bearer.mtu, Some(1428));
    }

    #[test]
    fn extracts_created_and_existing_bearer_paths() {
        let path = "/org/freedesktop/ModemManager1/Bearer/7";
        assert_eq!(
            parse_created_bearer_path(&format!("Successfully created new bearer:\n {path}")),
            Some(path.to_string())
        );
        assert_eq!(
            parse_bearer_paths(&format!("modem.generic.bearers.value[1] : {path}")),
            vec![path]
        );
    }
}
