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

use std::{collections::VecDeque, future::Future, net::IpAddr, process::Output};

use tokio::process::Command;

use super::{
    errors::{code, VolteError},
    pcscf::ImsIpSettings,
    plan::{FailureClass, ImsConnectionPlan, IpType},
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
    pub ip_type: String,
    pub settings: ImsIpSettings,
    pub ipv4_prefix: Option<u8>,
    pub ipv6_prefix: Option<u8>,
    pub mtu: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerAttempt {
    pub ip_type: String,
    pub source: String,
    pub outcome: String,
    pub error: Option<VolteError>,
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
    pub fn ims(allow_roaming: bool) -> Self {
        Self {
            allow_roaming,
            ..Self::default()
        }
    }

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
/// non-IMS bearers are never changed or deleted. The plan determines the initial
/// attempt type (dual-stack for `*First` modes) and the preference-ordered
/// single-family fallbacks — so `Ipv6First` now falls back v6→v4 rather than
/// always v4→v6. An explicit `Ipv6OnlyAllowed`/`Ipv4OnlyAllowed` response from
/// the network still collapses the fallback to that one family, regardless of
/// preference. Every failed temporary bearer is deleted before the next attempt.
pub async fn ensure_ims_bearer(
    modem: &str,
    request: &BearerRequest,
    plan: &ImsConnectionPlan,
) -> Result<BearerConnection, VolteError> {
    ensure_ims_bearer_observed(modem, request, plan, |_| async {}).await
}

pub async fn ensure_ims_bearer_observed<F, Fut>(
    modem: &str,
    request: &BearerRequest,
    plan: &ImsConnectionPlan,
    mut observe: F,
) -> Result<BearerConnection, VolteError>
where
    F: FnMut(BearerAttempt) -> Fut,
    Fut: Future<Output = ()>,
{
    if let Some(path) = bearer_path_override() {
        observe(BearerAttempt {
            ip_type: "override".to_string(),
            source: "environment".to_string(),
            outcome: "started".to_string(),
            error: None,
        })
        .await;
        let result = connect_and_read(&path).await;
        observe(BearerAttempt {
            ip_type: "override".to_string(),
            source: "environment".to_string(),
            outcome: if result.is_ok() {
                "succeeded"
            } else {
                "failed"
            }
            .to_string(),
            error: result.as_ref().err().cloned(),
        })
        .await;
        return result;
    }

    let modem_output = run_command("mmcli", &["-m", modem, "--output-keyvalue"]).await?;
    let mut required_fallback = None;
    let mut last_error = None;
    for path in parse_bearer_paths(&modem_output) {
        let details = run_command("mmcli", &["-b", &path, "--output-keyvalue"]).await?;
        if value(&details, "bearer.properties.apn").as_deref() == Some(IMS_APN) {
            if !bearer_roaming_policy_matches(&details, request.allow_roaming) {
                tracing::info!(
                    bearer_path = %path,
                    allow_roaming = request.allow_roaming,
                    "Recreating IMS bearer to match roaming policy"
                );
                if value(&details, "bearer.status.connected").as_deref() == Some("yes") {
                    disconnect_bearer(&path).await;
                }
                delete_bearer(modem, &path).await?;
                continue;
            }
            if is_dual_stack_bearer(&details) {
                if value(&details, "bearer.status.connected").as_deref() == Some("yes") {
                    let result = parse_bearer_connection(&path, &details);
                    observe(BearerAttempt {
                        ip_type: "ipv4v6".to_string(),
                        source: "reused".to_string(),
                        outcome: if result.is_ok() {
                            "succeeded"
                        } else {
                            "failed"
                        }
                        .to_string(),
                        error: result.as_ref().err().cloned(),
                    })
                    .await;
                    return result;
                }
                observe(BearerAttempt {
                    ip_type: "ipv4v6".to_string(),
                    source: "reconnected".to_string(),
                    outcome: "started".to_string(),
                    error: None,
                })
                .await;
                match connect_and_read(&path).await {
                    Ok(bearer) => {
                        observe(BearerAttempt {
                            ip_type: "ipv4v6".to_string(),
                            source: "reconnected".to_string(),
                            outcome: "succeeded".to_string(),
                            error: None,
                        })
                        .await;
                        return Ok(bearer);
                    }
                    Err(error) => {
                        let after = run_command("mmcli", &["-b", &path, "--output-keyvalue"])
                            .await
                            .unwrap_or_default();
                        required_fallback =
                            FailureClass::from_details(&after)
                                .forced_family()
                                .map(|f| match f {
                                    crate::connectivity::modems::softstack::volte::plan::IpFamily::Ipv6 => IpType::Ipv6,
                                    crate::connectivity::modems::softstack::volte::plan::IpFamily::Ipv4 => IpType::Ipv4,
                                });
                        observe(BearerAttempt {
                            ip_type: "ipv4v6".to_string(),
                            source: "reconnected".to_string(),
                            outcome: "failed".to_string(),
                            error: Some(error.clone()),
                        })
                        .await;
                        last_error = Some(error);
                    }
                }
            } else if value(&details, "bearer.status.connected").as_deref() == Some("yes") {
                disconnect_bearer(&path).await;
            }
            delete_bearer(modem, &path).await?;
            break;
        }
    }

    // Seed the attempt queue from the plan. If an earlier reconnect already
    // forced a single family (network told us v4-only or v6-only), collapse
    // directly to that; otherwise start with the plan's initial type.
    let initial: &'static str = match required_fallback {
        Some(IpType::Ipv6) => "ipv6",
        Some(IpType::Ipv4) => "ipv4",
        _ => plan.initial_bearer_attempt().as_mm_str(),
    };
    let mut pending: VecDeque<&'static str> = VecDeque::from([initial]);
    let mut attempted: Vec<&'static str> = Vec::with_capacity(3);
    while let Some(ip_type) = pending.pop_front() {
        if attempted.contains(&ip_type) {
            continue;
        }
        attempted.push(ip_type);
        observe(BearerAttempt {
            ip_type: ip_type.to_string(),
            source: "created".to_string(),
            outcome: "started".to_string(),
            error: None,
        })
        .await;
        match create_and_connect_attempt(modem, request, ip_type).await {
            Ok(bearer) => {
                observe(BearerAttempt {
                    ip_type: ip_type.to_string(),
                    source: "created".to_string(),
                    outcome: "succeeded".to_string(),
                    error: None,
                })
                .await;
                return Ok(bearer);
            }
            Err(failure) => {
                observe(BearerAttempt {
                    ip_type: ip_type.to_string(),
                    source: "created".to_string(),
                    outcome: "failed".to_string(),
                    error: Some(failure.error.clone()),
                })
                .await;
                last_error = Some(failure.error);
                // Only fan out into single-family fallbacks after a dual-stack
                // attempt fails. Single-family failures are terminal for that
                // family; the plan already decided the order.
                if ip_type == "ipv4v6" {
                    let class = FailureClass::from_details(&failure.details);
                    for fallback in plan.bearer_fallbacks_after(class) {
                        pending.push_back(fallback.as_mm_str());
                    }
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| VolteError::new(code::RUNTIME_MM_BEARER_CONNECT_FAILED)))
}

fn is_dual_stack_bearer(details: &str) -> bool {
    value(details, "bearer.properties.ip-type")
        .is_some_and(|ip_type| ip_type.eq_ignore_ascii_case("ipv4v6"))
}

fn bearer_roaming_policy_matches(details: &str, allow_roaming: bool) -> bool {
    bearer_allows_roaming(details) == Some(allow_roaming)
}

fn bearer_allows_roaming(details: &str) -> Option<bool> {
    for key in [
        "bearer.properties.roaming-allowance",
        "bearer.properties.allow-roaming",
    ] {
        let Some(raw) = value(details, key) else {
            continue;
        };
        let normalized = raw.trim().to_ascii_lowercase();
        let allowed = match normalized.as_str() {
            "yes" | "true" | "1" | "allowed" | "any" => true,
            "no" | "false" | "0" | "forbidden" | "none" | "home" => false,
            _ if normalized.contains("partner") || normalized.contains("non-partner") => true,
            _ => continue,
        };
        return Some(allowed);
    }
    None
}

struct BearerAttemptFailure {
    error: VolteError,
    details: String,
}

async fn create_and_connect_attempt(
    modem: &str,
    request: &BearerRequest,
    ip_type: &str,
) -> Result<BearerConnection, BearerAttemptFailure> {
    let properties = format!(
        "apn={},ip-type={ip_type},allow-roaming={}",
        request.apn,
        if request.allow_roaming { "yes" } else { "no" }
    );
    let created = run_command(
        "mmcli",
        &["-m", modem, &format!("--create-bearer={properties}")],
    )
    .await
    .map_err(|error| BearerAttemptFailure {
        error,
        details: String::new(),
    })?;
    let path = parse_created_bearer_path(&created).ok_or_else(|| BearerAttemptFailure {
        error: VolteError::new(code::RUNTIME_MM_BEARER_PATH_MISSING),
        details: String::new(),
    })?;
    match connect_and_read(&path).await {
        Ok(bearer) => Ok(bearer),
        Err(error) => {
            let details = run_command("mmcli", &["-b", &path, "--output-keyvalue"])
                .await
                .unwrap_or_default();
            if let Err(cleanup) = delete_bearer(modem, &path).await {
                return Err(BearerAttemptFailure {
                    error: cleanup,
                    details,
                });
            }
            Err(BearerAttemptFailure { error, details })
        }
    }
}

async fn delete_bearer(modem: &str, path: &str) -> Result<(), VolteError> {
    run_command("mmcli", &["-m", modem, &format!("--delete-bearer={path}")])
        .await
        .map(|_| ())
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

pub fn parse_bearer_connection(path: &str, output: &str) -> Result<BearerConnection, VolteError> {
    if value(output, "bearer.status.connected").as_deref() != Some("yes") {
        return Err(VolteError::new(code::RUNTIME_MM_BEARER_NOT_CONNECTED));
    }
    let interface = value(output, "bearer.status.interface")
        .filter(|item| item != "--")
        .ok_or_else(|| VolteError::new(code::IP_SETTINGS_MISSING))?;
    let settings = ImsIpSettings {
        ipv4_address: ip_value(output, "bearer.ipv4-config.address"),
        ipv4_gateway: ip_value(output, "bearer.ipv4-config.gateway"),
        ipv4_dns: list_ip_values(output, "bearer.ipv4-config.dns.value"),
        ipv6_address: ip_value(output, "bearer.ipv6-config.address"),
        ipv6_gateway: ip_value(output, "bearer.ipv6-config.gateway"),
        ipv6_dns: list_ip_values(output, "bearer.ipv6-config.dns.value"),
        ..Default::default()
    };
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
        ip_type: value(output, "bearer.properties.ip-type")
            .unwrap_or_else(|| "unknown".to_string()),
        settings,
        ipv4_prefix,
        ipv6_prefix,
        mtu,
    })
}

/// Configure the address and DNS host routes for the dedicated bearer. No
/// default route is added, preserving the management/Wi-Fi path.
pub async fn configure_bearer_network(bearer: &BearerConnection) -> Result<(), VolteError> {
    // A raw-IP bearer still has to be administratively UP before Linux accepts
    // host routes or transmits bound SIP/RTP sockets.  In particular, a
    // bam-dmux runtime-PM/firmware failure may surface here as EINVAL or
    // ETIMEDOUT.  Propagate that first failure instead of obscuring it with the
    // inevitable later "Device for nexthop is not up" route error.
    run_ip(&["link", "set", "dev", &bearer.interface, "up"]).await?;
    if let Some(mtu) = bearer.mtu {
        let mtu = mtu.to_string();
        run_ip(&["link", "set", "dev", &bearer.interface, "mtu", &mtu]).await?;
    }

    // Configure each available family independently. A per-family failure
    // (e.g. the network handed out an IPv6 address but the prefix/DNS route
    // cannot be installed) must not discard a working sibling family. Mirrors
    // 1.7's "IPv6 data configuration failed; retaining IPv4 data" behaviour.
    let mut configured = false;
    let mut last_error = None;
    if bearer.settings.ipv6_address.is_some() {
        match configure_ipv6(bearer).await {
            Ok(()) => configured = true,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    interface = %bearer.interface,
                    "VoLTE IPv6 data configuration failed; retaining any IPv4 data"
                );
                last_error = Some(error);
            }
        }
    }
    if bearer.settings.ipv4_address.is_some() {
        match configure_ipv4(bearer).await {
            Ok(()) => configured = true,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    interface = %bearer.interface,
                    "VoLTE IPv4 data configuration failed; retaining any IPv6 data"
                );
                last_error = Some(error);
            }
        }
    }
    if configured {
        return Ok(());
    }
    Err(last_error.unwrap_or_else(|| VolteError::new(code::IP_SETTINGS_MISSING)))
}

async fn configure_ipv6(bearer: &BearerConnection) -> Result<(), VolteError> {
    let Some(IpAddr::V6(address)) = bearer.settings.ipv6_address else {
        return Err(VolteError::new(code::IP_SETTINGS_MISSING));
    };
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
    Ok(())
}

async fn configure_ipv4(bearer: &BearerConnection) -> Result<(), VolteError> {
    let Some(IpAddr::V4(address)) = bearer.settings.ipv4_address else {
        return Err(VolteError::new(code::IP_SETTINGS_MISSING));
    };
    let prefix = bearer.ipv4_prefix.unwrap_or(32);
    let address = format!("{address}/{prefix}");
    run_ip(&["address", "replace", &address, "dev", &bearer.interface]).await?;
    for dns in &bearer.settings.ipv4_dns {
        route_host(&bearer.interface, *dns).await?;
    }
    Ok(())
}

pub async fn route_pcscf(bearer: &BearerConnection, pcscf: IpAddr) -> Result<(), VolteError> {
    route_host(&bearer.interface, pcscf).await
}

async fn route_host(interface: &str, host: IpAddr) -> Result<(), VolteError> {
    let (family, suffix) = if host.is_ipv6() {
        (Some("-6"), 128)
    } else {
        (None, 32)
    };
    let destination = format!("{host}/{suffix}");
    let mut args = Vec::new();
    if let Some(family) = family {
        args.push(family);
    }
    args.extend_from_slice(&["route", "replace", &destination, "dev", interface]);
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

/// Disconnect a ModemManager bearer.
///
/// A natively established bearer has no ModemManager object behind its `path`, so
/// sending it here would fail *and* leave the real WDS session running. Those are
/// torn down through `native_bearer::release_native_ims_bearer` instead, which
/// owns the session handle; this guard means a caller that does not yet know the
/// difference cannot silently leak a PDP context.
pub async fn disconnect_bearer(path: &str) {
    if super::native_bearer::is_native_bearer(path) {
        tracing::debug!(
            path = %path,
            "Skipping mmcli disconnect for a native QMI bearer; its WDS session is released separately"
        );
        return;
    }
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
    command_output(program, args, output)
}

fn command_output(program: &str, args: &[&str], output: Output) -> Result<String, VolteError> {
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr)
            .trim()
            .replace('\n', " ");
        Err(VolteError::with_detail(
            code::COMMAND_FAILED,
            format!(
                "{program}:{}:{}:{}",
                output.status.code().unwrap_or(-1),
                args.join(" "),
                stderr
            ),
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
        assert!(BearerRequest::ims(true).allow_roaming);
    }

    #[test]
    fn matches_modemmanager_roaming_policy_renderings() {
        let allowed = "bearer.properties.roaming-allowance : allowed\n";
        let forbidden = "bearer.properties.roaming-allowance : forbidden\n";
        let legacy_yes = "bearer.properties.allow-roaming : yes\n";
        let legacy_no = "bearer.properties.allow-roaming : no\n";

        assert!(bearer_roaming_policy_matches(allowed, true));
        assert!(bearer_roaming_policy_matches(forbidden, false));
        assert!(bearer_roaming_policy_matches(legacy_yes, true));
        assert!(bearer_roaming_policy_matches(legacy_no, false));
        assert!(!bearer_roaming_policy_matches(allowed, false));
        assert!(!bearer_roaming_policy_matches(forbidden, true));
    }

    #[test]
    fn unknown_roaming_policy_requires_bearer_recreation() {
        assert!(!bearer_roaming_policy_matches(
            "bearer.properties.apn : ims\n",
            true
        ));
        assert!(!bearer_roaming_policy_matches(
            "bearer.properties.apn : ims\n",
            false
        ));
    }

    #[test]
    fn network_family_rejection_selects_required_bearer_type() {
        use crate::connectivity::modems::softstack::volte::plan::{FailureClass, ImsConnectionPlan};
        use crate::platform::config::VolteIpFamilyPreference;
        let plan_v6 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv6First);
        let plan_v4 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4First);
        let ipv6 = "bearer.status.connection-error.name : org.freedesktop.ModemManager1.Error.MobileEquipment.Ipv6OnlyAllowed\n";
        assert_eq!(
            FailureClass::from_details(ipv6),
            FailureClass::NetworkForcedIpv6
        );
        let ipv4 = "bearer.status.connection-error.name : org.freedesktop.ModemManager1.Error.MobileEquipment.Ipv4OnlyAllowed\n";
        assert_eq!(
            FailureClass::from_details(ipv4),
            FailureClass::NetworkForcedIpv4
        );
        let generic = "bearer.status.connection-error.name : org.example.Failed\n";
        assert_eq!(FailureClass::from_details(generic), FailureClass::Other);
        // Forced families collapse to a single type regardless of preference.
        assert_eq!(
            plan_v4.bearer_fallbacks_after(FailureClass::from_details(ipv6)),
            vec![super::IpType::Ipv6]
        );
        assert_eq!(
            plan_v6.bearer_fallbacks_after(FailureClass::from_details(ipv4)),
            vec![super::IpType::Ipv4]
        );
        // Generic failure respects preference order.
        assert_eq!(
            plan_v4.bearer_fallbacks_after(FailureClass::from_details(generic)),
            vec![super::IpType::Ipv4, super::IpType::Ipv6]
        );
        assert_eq!(
            plan_v6.bearer_fallbacks_after(FailureClass::from_details(generic)),
            vec![super::IpType::Ipv6, super::IpType::Ipv4]
        );
    }

    #[test]
    fn only_explicit_dual_stack_bearers_are_reused() {
        assert!(is_dual_stack_bearer("bearer.properties.ip-type: ipv4v6"));
        assert!(is_dual_stack_bearer("bearer.properties.ip-type: IPV4V6"));
        assert!(!is_dual_stack_bearer("bearer.properties.ip-type: ipv6"));
        assert!(!is_dual_stack_bearer("bearer.status.connected: yes"));
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
        let bearer =
            parse_bearer_connection("/org/freedesktop/ModemManager1/Bearer/2", output).unwrap();
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
        let bearer =
            parse_bearer_connection("/org/freedesktop/ModemManager1/Bearer/9", output).unwrap();

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
