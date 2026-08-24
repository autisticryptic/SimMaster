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

use crate::platform::network_routing::{
    host_selector, network_address, route_table, rule_priority, source_selector, RouteDomain,
};
use crate::services::ue_worker::{NetConfigOp, UeWorkerHandle};

use super::{
    errors::{code, VolteError},
    pcscf::ImsIpSettings,
    plan::{FailureClass, ImsConnectionPlan, IpType},
};

/// Test default only. Production registration receives the IMS APN from the
/// carrier catalog.
#[cfg(test)]
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

#[cfg(test)]
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
    #[cfg(test)]
    pub fn ims(allow_roaming: bool) -> Self {
        Self {
            allow_roaming,
            ..Self::default()
        }
    }

    pub fn for_apn(apn: impl Into<String>, allow_roaming: bool) -> Self {
        Self {
            apn: apn.into(),
            allow_roaming,
            profile_id: None,
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
        if value(&details, "bearer.properties.apn")
            .as_deref()
            .is_some_and(|apn| apn.eq_ignore_ascii_case(&request.apn))
        {
            let roaming_matches = bearer_roaming_policy_matches(&details, request.allow_roaming);
            let profile_matches = bearer_profile_matches(&details, request.profile_id);
            if !roaming_matches || !profile_matches {
                tracing::info!(
                    bearer_path = %path,
                    allow_roaming = request.allow_roaming,
                    profile_id = ?request.profile_id,
                    roaming_matches,
                    profile_matches,
                    "Recreating IMS bearer to match requested properties"
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
                                .map(|f| {
                                    match f {
                                crate::connectivity::modems::ims::volte::plan::IpFamily::Ipv6 => {
                                    IpType::Ipv6
                                }
                                crate::connectivity::modems::ims::volte::plan::IpFamily::Ipv4 => {
                                    IpType::Ipv4
                                }
                            }
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
    bearer_allows_roaming(details).map_or(true, |actual| actual == allow_roaming)
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

fn bearer_profile_matches(details: &str, expected: Option<u32>) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    number_value::<u32>(details, "bearer.properties.profile-id") == Some(expected)
}

fn create_bearer_properties(request: &BearerRequest, ip_type: &str) -> String {
    let mut properties = match request.profile_id {
        Some(profile_id) => format!("profile-id={profile_id},apn={}", request.apn),
        None => format!("apn={}", request.apn),
    };
    properties.push_str(&format!(
        ",ip-type={ip_type},allow-roaming={}",
        if request.allow_roaming { "yes" } else { "no" }
    ));
    properties
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
    let properties = create_bearer_properties(request, ip_type);
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
    ensure_bearer_interface_ready(&bearer.interface).await?;
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

/// Configure a dedicated native IMS netdev after it has been moved into its
/// per-line worker namespace. No policy-rule/table indirection is needed there:
/// the namespace itself is the route domain, so identical addresses on other
/// UEs cannot collide.
pub async fn configure_bearer_network_in_worker(
    bearer: &BearerConnection,
    worker: &UeWorkerHandle,
) -> Result<(), VolteError> {
    let mut ops = Vec::new();
    if let Some(mtu) = bearer.mtu {
        ops.push(NetConfigOp::LinkSetMtu {
            ifname: bearer.interface.clone(),
            mtu,
        });
    }
    for (address, prefix, dns) in [
        (
            bearer.settings.ipv6_address,
            bearer.ipv6_prefix.unwrap_or(64),
            bearer.settings.ipv6_dns.as_slice(),
        ),
        (
            bearer.settings.ipv4_address,
            bearer.ipv4_prefix.unwrap_or(32),
            bearer.settings.ipv4_dns.as_slice(),
        ),
    ] {
        let Some(address) = address else { continue };
        ops.push(NetConfigOp::AddrReplace {
            ifname: bearer.interface.clone(),
            cidr: format!("{address}/{prefix}"),
        });
        for server in dns {
            ops.push(worker_host_route_op(bearer, *server)?);
        }
    }
    if !ops
        .iter()
        .any(|op| matches!(op, NetConfigOp::AddrReplace { .. }))
    {
        return Err(VolteError::new(code::IP_SETTINGS_MISSING));
    }
    ops.push(NetConfigOp::LinkSetUp {
        ifname: bearer.interface.clone(),
    });
    apply_worker_ops(worker, ops).await
}

pub async fn route_pcscf_in_worker(
    bearer: &BearerConnection,
    pcscf: IpAddr,
    worker: &UeWorkerHandle,
) -> Result<(), VolteError> {
    apply_worker_ops(worker, vec![worker_host_route_op(bearer, pcscf)?]).await
}

pub async fn route_media_host_in_worker(
    bearer: &BearerConnection,
    host: IpAddr,
    worker: &UeWorkerHandle,
) -> Result<(), VolteError> {
    apply_worker_ops(worker, vec![worker_host_route_op(bearer, host)?]).await
}

fn worker_host_route_op(
    bearer: &BearerConnection,
    host: IpAddr,
) -> Result<NetConfigOp, VolteError> {
    let source = bearer
        .settings
        .local_addr_for_family(host)
        .ok_or_else(|| VolteError::new("volte_route_family_mismatch"))?;
    Ok(NetConfigOp::RouteReplace {
        target: host_selector(host),
        via: None,
        dev: Some(bearer.interface.clone()),
        src: Some(source.to_string()),
        table: None,
    })
}

async fn apply_worker_ops(
    worker: &UeWorkerHandle,
    ops: Vec<NetConfigOp>,
) -> Result<(), VolteError> {
    let outcome = worker.apply_net_config(ops).await.map_err(|error| {
        VolteError::with_detail(code::COMMAND_FAILED, format!("worker net-config: {error}"))
    })?;
    if outcome.ok {
        Ok(())
    } else {
        Err(VolteError::with_detail(
            code::COMMAND_FAILED,
            outcome
                .error
                .unwrap_or_else(|| "worker net-config failed".to_string()),
        ))
    }
}

/// Ensure that the kernel data path is usable before installing policy routes.
///
/// ModemManager reports a connected QMI bearer before the netdev has necessarily
/// completed its remote OPEN handshake, so one administrative UP plus polling is
/// the portable part of this function.
///
/// The non-portable part -- whether this baseband has latched a permanent error
/// state that makes OPEN impossible -- is asked of the platform's
/// [`BasebandFaultPolicy`] rather than tested inline. The 410's bam-dmux latch
/// used to be hard-coded here, which made a generic IMS path assert something
/// true of exactly one SoC and left other hardware nowhere to describe its own
/// firmware defects. See `hardware/devices/baseband_faults.rs`.
pub(crate) async fn ensure_bearer_interface_ready(interface: &str) -> Result<(), VolteError> {
    if interface_is_up(interface).await {
        return Ok(());
    }
    let faults = crate::hardware::devices::baseband_faults::detected_fault_policy();
    let latched_error = |interface: &str, when: &str| {
        let fault = faults.inspect_data_interface(interface);
        let detail = match faults.fault_note(fault) {
            Some(note) => format!("interface={interface}: {when} ({note})"),
            None => format!("interface={interface}: {when}"),
        };
        VolteError::with_detail(code::BEARER_NETDEV_RUNTIME_ERROR, detail)
    };
    if !faults
        .inspect_data_interface(interface)
        .permits_bring_up()
    {
        return Err(latched_error(interface, "runtime_status=error before OPEN"));
    }

    // One administrative UP is one remote OPEN request. Never issue several
    // OPENs in a readiness loop: duplicate requests can race the modem
    // firmware. Polling below only observes the result of this single request.
    if let Err(error) = run_ip(&["link", "set", "dev", interface, "up"]).await {
        if interface_is_up(interface).await {
            return Ok(());
        }
        return Err(
            if !faults
                .inspect_data_interface(interface)
                .permits_bring_up()
            {
                latched_error(interface, &error.to_string())
            } else {
                VolteError::with_detail(
                    code::BEARER_NETDEV_NOT_UP,
                    format!("interface={interface}: {error}"),
                )
            },
        );
    }

    for attempt in 0..4 {
        if interface_is_up(interface).await {
            return Ok(());
        }
        if !faults
            .inspect_data_interface(interface)
            .permits_bring_up()
        {
            return Err(latched_error(interface, "runtime_status=error after OPEN"));
        }
        if attempt < 3 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
    Err(VolteError::with_detail(
        code::BEARER_NETDEV_NOT_READY,
        format!("interface={interface} remained down after one OPEN request"),
    ))
}

async fn interface_is_up(interface: &str) -> bool {
    let Ok(output) = run_command("ip", &["-json", "link", "show", "dev", interface]).await else {
        return false;
    };
    link_output_is_up(&output)
}

fn link_output_is_up(output: &str) -> bool {
    let Ok(links) = serde_json::from_str::<Vec<serde_json::Value>>(output) else {
        return false;
    };
    links.first().is_some_and(|link| {
        link.get("operstate")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|state| state.eq_ignore_ascii_case("up"))
            || link
                .get("flags")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|flags| {
                    flags.iter().any(|flag| {
                        flag.as_str().is_some_and(|flag| {
                            flag.eq_ignore_ascii_case("up") || flag.eq_ignore_ascii_case("lower_up")
                        })
                    })
                })
    })
}

/// Confirm the address the policy routing was built around is still the one on
/// the interface.
///
/// The IMS bearer is torn down and re-established between attempts and the
/// network hands out a different address every time, so a `BearerConnection`
/// captured moments earlier can name an address the interface no longer owns.
/// The `ip rule` is keyed on that source address, so once it goes stale the
/// REGISTER misses the bearer's table entirely and follows the host default
/// route out of the wrong interface, where a private P-CSCF address is
/// unroutable and the transaction simply times out.
pub(crate) async fn interface_still_holds_address(interface: &str, address: IpAddr) -> bool {
    let Ok(output) = run_command("ip", &["-json", "address", "show", "dev", interface]).await
    else {
        return false;
    };
    addr_output_contains(&output, address)
}

fn addr_output_contains(output: &str, address: IpAddr) -> bool {
    let Ok(links) = serde_json::from_str::<Vec<serde_json::Value>>(output) else {
        return false;
    };
    let wanted = address.to_string();
    links.iter().any(|link| {
        link.get("addr_info")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|entries| {
                entries.iter().any(|entry| {
                    entry
                        .get("local")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|local| local.eq_ignore_ascii_case(&wanted))
                })
            })
    })
}

async fn configure_ipv6(bearer: &BearerConnection) -> Result<(), VolteError> {
    let Some(address @ IpAddr::V6(_)) = bearer.settings.ipv6_address else {
        return Err(VolteError::new(code::IP_SETTINGS_MISSING));
    };
    let prefix = bearer.ipv6_prefix.unwrap_or(64);
    let address_with_prefix = format!("{address}/{prefix}");
    run_ip(&[
        "-6",
        "address",
        "replace",
        &address_with_prefix,
        "dev",
        &bearer.interface,
    ])
    .await?;
    configure_source_policy(&bearer.interface, address, prefix).await?;
    for dns in &bearer.settings.ipv6_dns {
        route_host_on_bearer(bearer, *dns).await?;
    }
    Ok(())
}

async fn configure_ipv4(bearer: &BearerConnection) -> Result<(), VolteError> {
    let Some(address @ IpAddr::V4(_)) = bearer.settings.ipv4_address else {
        return Err(VolteError::new(code::IP_SETTINGS_MISSING));
    };
    let prefix = bearer.ipv4_prefix.unwrap_or(32);
    let address_with_prefix = format!("{address}/{prefix}");
    run_ip(&[
        "address",
        "replace",
        &address_with_prefix,
        "dev",
        &bearer.interface,
    ])
    .await?;
    configure_source_policy(&bearer.interface, address, prefix).await?;
    for dns in &bearer.settings.ipv4_dns {
        route_host_on_bearer(bearer, *dns).await?;
    }
    Ok(())
}

pub async fn route_pcscf(bearer: &BearerConnection, pcscf: IpAddr) -> Result<(), VolteError> {
    route_host_on_bearer(bearer, pcscf).await
}

/// Install a host route for an operator media address on the dedicated IMS
/// bearer. RTP/RTCP and video endpoints are supplied dynamically in SDP and
/// are not necessarily the P-CSCF address; without this route Linux may send
/// media through the management/Wi-Fi default route.
pub async fn route_media_host(bearer: &BearerConnection, host: IpAddr) -> Result<(), VolteError> {
    route_host_on_bearer(bearer, host).await
}

/// IMS traffic must be selected by the bearer source address, not by the
/// process-wide main route table. Multiple modems can receive the same remote
/// RTP address, and a main-table `/32` would let the last line win.
async fn route_host_on_bearer(bearer: &BearerConnection, host: IpAddr) -> Result<(), VolteError> {
    // A dual-stack ModemManager bearer has two independent local addresses.
    // `BearerConnection::local_addr()` intentionally returns the preferred
    // address (currently IPv6-first), which is not necessarily the family
    // selected by the current REGISTER attempt.  Comparing every destination
    // with that preferred address made a valid IPv4 attempt fail with
    // `volte_route_family_mismatch` whenever the same bearer also had IPv6.
    // Select the local address belonging to the destination family instead.
    let local = bearer
        .settings
        .local_addr_for_family(host)
        .ok_or_else(|| VolteError::new("volte_route_family_mismatch"))?;
    if local.is_ipv4() != host.is_ipv4() {
        return Err(VolteError::new("volte_route_family_mismatch"));
    }
    let table = route_table(RouteDomain::VolteIms, &bearer.interface, host);
    let destination = host_selector(host);
    let family = if host.is_ipv6() { Some("-6") } else { None };
    let table = table.to_string();
    let mut args = Vec::new();
    if let Some(family) = family {
        args.push(family);
    }
    args.extend_from_slice(&[
        "route",
        "replace",
        &destination,
        "dev",
        &bearer.interface,
        "table",
        &table,
    ]);
    run_ip(&args).await.map(|_| ())
}

async fn configure_source_policy(
    interface: &str,
    address: IpAddr,
    prefix: u8,
) -> Result<(), VolteError> {
    let family = if address.is_ipv6() { Some("-6") } else { None };
    let table = route_table(RouteDomain::VolteIms, interface, address).to_string();
    let priority = rule_priority(RouteDomain::VolteIms, interface, address).to_string();
    let source = source_selector(address);
    let connected = format!("{}/{prefix}", network_address(address, prefix));
    let mut flush = Vec::new();
    if let Some(family) = family {
        flush.push(family);
    }
    flush.extend_from_slice(&["route", "flush", "table", &table]);
    let _ = run_ip(&flush).await;

    let mut delete = Vec::new();
    if let Some(family) = family {
        delete.push(family);
    }
    delete.extend_from_slice(&["rule", "del", "priority", &priority]);
    let _ = run_ip(&delete).await;

    let mut add = Vec::new();
    if let Some(family) = family {
        add.push(family);
    }
    add.extend_from_slice(&[
        "rule", "add", "priority", &priority, "from", &source, "table", &table,
    ]);
    run_ip(&add).await?;

    let mut connected_route = Vec::new();
    if let Some(family) = family {
        connected_route.push(family);
    }
    connected_route.extend_from_slice(&[
        "route", "replace", &connected, "dev", interface, "table", &table,
    ]);
    run_ip(&connected_route).await.map(|_| ())
}

/// Remove network state only from the dedicated bearer interface. This is
/// used on failed registration and normal teardown so stale IPv6 addresses or
/// host routes cannot accumulate across long-running retries.
pub async fn teardown_bearer_network(bearer: &BearerConnection) {
    for (address, _prefix) in [
        (bearer.settings.ipv4_address, bearer.ipv4_prefix),
        (bearer.settings.ipv6_address, bearer.ipv6_prefix),
    ] {
        if let Some(address) = address {
            let table = route_table(RouteDomain::VolteIms, &bearer.interface, address).to_string();
            let priority =
                rule_priority(RouteDomain::VolteIms, &bearer.interface, address).to_string();
            let family = if address.is_ipv6() { Some("-6") } else { None };
            let mut flush = Vec::new();
            if let Some(family) = family {
                flush.push(family);
            }
            flush.extend_from_slice(&["route", "flush", "table", &table]);
            let _ = run_ip(&flush).await;
            let mut delete = Vec::new();
            if let Some(family) = family {
                delete.push(family);
            }
            delete.extend_from_slice(&["rule", "del", "priority", &priority]);
            let _ = run_ip(&delete).await;
        }
    }
    let _ = run_ip(&["-6", "route", "flush", "dev", &bearer.interface]).await;
    let _ = run_ip(&["route", "flush", "dev", &bearer.interface]).await;
    let _ = run_ip(&["address", "flush", "dev", &bearer.interface]).await;
    // Let ModemManager/QMI own the bam-dmux CLOSE handshake when the bearer is
    // disconnected. Sending `ip link down` here duplicates that operation and
    // can race the firmware on Qualcomm SoCs, especially after a failed route
    // setup. Ordinary interface state is cleaned up by the bearer disconnect.
}

/// Remove only network state owned by a native IMS interface in a UE worker.
/// The worker namespace may also contain VoWiFi/veth state, so cleanup is
/// deliberately device-scoped and never flushes the whole main table.
pub async fn teardown_bearer_network_in_worker(bearer: &BearerConnection, worker: &UeWorkerHandle) {
    let mut ops = vec![
        NetConfigOp::FlushRoutesForDevice {
            ifname: bearer.interface.clone(),
            ipv6: true,
        },
        NetConfigOp::FlushRoutesForDevice {
            ifname: bearer.interface.clone(),
            ipv6: false,
        },
    ];
    for (address, prefix) in [
        (
            bearer.settings.ipv6_address,
            bearer.ipv6_prefix.unwrap_or(64),
        ),
        (
            bearer.settings.ipv4_address,
            bearer.ipv4_prefix.unwrap_or(32),
        ),
    ] {
        if let Some(address) = address {
            ops.push(NetConfigOp::AddrDel {
                ifname: bearer.interface.clone(),
                cidr: format!("{address}/{prefix}"),
            });
        }
    }
    let _ = worker.apply_net_config(ops).await;
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

    /// Maxis re-addresses the IMS PDN on every activation, so the source-based
    /// policy rule goes stale and SIP silently leaves via the host default
    /// route. The liveness check has to notice the address is gone.
    #[test]
    fn address_liveness_follows_the_interface_not_the_snapshot() {
        let configured = r#"[{"ifname":"wwan1","addr_info":[
            {"family":"inet","local":"2.188.57.65","prefixlen":30},
            {"family":"inet6","local":"2001:d08:1504:2c26::1","prefixlen":64}]}]"#;

        assert!(addr_output_contains(
            configured,
            IpAddr::V4(std::net::Ipv4Addr::new(2, 188, 57, 65))
        ));
        // The address the bearer carried a moment ago is no longer present.
        assert!(!addr_output_contains(
            configured,
            IpAddr::V4(std::net::Ipv4Addr::new(2, 181, 21, 248))
        ));
        assert!(addr_output_contains(
            configured,
            "2001:d08:1504:2c26::1".parse::<IpAddr>().unwrap()
        ));

        // An interface with no addresses, and unparsable output, are both "gone".
        assert!(!addr_output_contains(
            r#"[{"ifname":"wwan1","addr_info":[]}]"#,
            IpAddr::V4(std::net::Ipv4Addr::new(2, 188, 57, 65))
        ));
        assert!(!addr_output_contains(
            "not json",
            IpAddr::V4(std::net::Ipv4Addr::new(2, 188, 57, 65))
        ));
    }

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
    fn parses_ip_json_link_state_without_substring_false_positives() {
        assert!(link_output_is_up(
            r#"[{"ifname":"wwan0","flags":["POINTOPOINT","UP","LOWER_UP"],"operstate":"UNKNOWN"}]"#
        ));
        assert!(link_output_is_up(
            r#"[{"ifname":"wwan0","flags":["POINTOPOINT"],"operstate":"UP"}]"#
        ));
        assert!(!link_output_is_up(
            r#"[{"ifname":"backup0","flags":["POINTOPOINT"],"operstate":"DOWN"}]"#
        ));
        assert!(!link_output_is_up("not-json"));
    }

    // Runtime-PM latch detection moved to
    // hardware/devices/qcm410/baseband_faults.rs, which owns both the sysfs
    // parsing and its tests. This module is platform-agnostic again.

    #[test]
    fn default_request_uses_ims_apn() {
        assert_eq!(BearerRequest::default().apn, "ims");
        assert!(!BearerRequest::default().allow_roaming);
        assert!(BearerRequest::ims(true).allow_roaming);
    }

    #[test]
    fn modemmanager_properties_include_requested_3gpp_profile() {
        let mut request = BearerRequest::ims(true);
        request.profile_id = Some(2);
        assert_eq!(
            create_bearer_properties(&request, "ipv4v6"),
            "profile-id=2,apn=ims,ip-type=ipv4v6,allow-roaming=yes"
        );

        request.profile_id = None;
        assert_eq!(
            create_bearer_properties(&request, "ipv4"),
            "apn=ims,ip-type=ipv4,allow-roaming=yes"
        );
    }

    #[test]
    fn requested_profile_rejects_unbound_or_different_bearers() {
        let exact = "bearer.properties.profile-id : 2\n";
        let different = "bearer.properties.profile-id : 1\n";
        let unbound = "bearer.properties.profile-id : --\n";

        assert!(bearer_profile_matches(exact, Some(2)));
        assert!(!bearer_profile_matches(different, Some(2)));
        assert!(!bearer_profile_matches(unbound, Some(2)));
        assert!(bearer_profile_matches(unbound, None));
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
    fn unknown_roaming_policy_does_not_churn_a_profile_bound_bearer() {
        assert!(bearer_roaming_policy_matches(
            "bearer.properties.apn : ims\n",
            true
        ));
        assert!(bearer_roaming_policy_matches(
            "bearer.properties.apn : ims\n",
            false
        ));
    }

    #[test]
    fn network_family_rejection_selects_required_bearer_type() {
        use crate::connectivity::modems::ims::volte::plan::{FailureClass, ImsConnectionPlan};
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
