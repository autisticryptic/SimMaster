//! VoLTE P-CSCF discovery.
//!
//! Clean-room from 3GPP TS 24.229 (P-CSCF discovery) + TS 27.007. The P-CSCF
//! address is obtained from the IMS APN bearer's PCO / connection settings.
//! Different modems surface it differently; the reference uses a `+CGCONTRDP`
//! query and parses the IP settings block. Here we keep the parsing (fully
//! testable) separate from the ModemManager/AT IO (which runs on device).
//!
//! Observed data-path settings block anchors (from the reference):
//!   `IPv6 address:` / `IPv6 gateway address:` / `IPv6 primary DNS:` /
//!   `IPv4 address:` / `IPv4 gateway address:` / `IPv4 primary DNS:` ...
//! The P-CSCF is normally delivered via PCO. DNS addresses are resolver
//! endpoints, not implicit SIP proxies; they are only used to resolve the
//! standard P-CSCF/SRV names.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use tokio::{net::UdpSocket, process::Command, time::sleep};

use crate::platform::config::VolteIpFamilyPreference;

use super::errors::{code, VolteError};
use super::plan::{ImsConnectionPlan, IpFamily};

const DNS_TIMEOUT: Duration = Duration::from_secs(4);
const DNS_PORT: u16 = 53;
const SIP_PORT: u16 = 5060;
const ENV_PCSCF: &str = "SIMADMIN_VOLTE_PCSCF";
const ENV_IMS_CID: &str = "SIMADMIN_VOLTE_IMS_CID";
const DEFAULT_IMS_CID: u8 = 2;
const PROFILE_PCSCF_READ_ROUNDS: usize = 4;
const PROFILE_PCSCF_READ_DELAY: Duration = Duration::from_millis(750);
const BETA2_PROFILE_CANDIDATES: [u8; 2] = [2, 1];

#[derive(Debug, Clone)]
pub struct AtPcscfDiscovery {
    pub candidates: Vec<IpAddr>,
    pub cid: u8,
}

/// IMS PDP profile selected for this registration attempt.
///
/// Qualcomm's WDS `3gpp-profile` and the AT PDP context id use the same profile
/// index on the reference firmware. Keeping the selected id explicit prevents
/// an APN-only WDS start from attaching to the ordinary Internet profile and
/// losing the IMS PCO options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImsProfileContext {
    pub cid: u8,
    pub created: bool,
}

/// A temporary IMS PDP definition kept alive for the lifetime of registration.
///
/// The beta2 runtime activates this context before starting the WDS bearer and
/// retains it while SIP is registered. Cleanup restores the definition that was
/// present before the attempt so an Internet profile is not permanently changed.
#[derive(Debug)]
pub struct ImsProfileLease {
    modem: String,
    pub cid: u8,
    restore_command: String,
}

impl ImsProfileLease {
    pub async fn cleanup(self) {
        cleanup_profile_context(&self.modem, self.cid, &self.restore_command).await;
    }
}

#[derive(Debug)]
pub struct ImsProfilePrefetch {
    pub candidates: Vec<IpAddr>,
    pub lease: ImsProfileLease,
}

/// Parsed IP settings for the IMS bearer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImsIpSettings {
    pub ipv6_address: Option<IpAddr>,
    pub ipv6_gateway: Option<IpAddr>,
    pub ipv6_dns: Vec<IpAddr>,
    pub ipv4_address: Option<IpAddr>,
    pub ipv4_gateway: Option<IpAddr>,
    pub ipv4_dns: Vec<IpAddr>,
    /// Explicit P-CSCF addresses if delivered via PCO.
    pub pcscf: Vec<IpAddr>,
}

impl ImsIpSettings {
    /// Choose the local UE address for SIP: prefer IPv6 (IMS is usually v6).
    pub fn local_addr(&self) -> Option<IpAddr> {
        self.ipv6_address.or(self.ipv4_address)
    }

    /// Available bearer addresses in the plan's family order.
    pub fn ordered_local_addrs(&self, plan: &ImsConnectionPlan) -> Vec<IpAddr> {
        let mut addresses = Vec::with_capacity(2);
        for family in plan.pcscf_order() {
            let addr = match family {
                IpFamily::Ipv6 => self.ipv6_address,
                IpFamily::Ipv4 => self.ipv4_address,
            };
            push_optional_addr(&mut addresses, addr);
        }
        addresses
    }

    /// Return an explicit P-CSCF delivered by the bearer PCO.
    ///
    /// DNS server addresses must never be returned here. Some Qualcomm
    /// devices expose public carrier resolvers in the IMS bearer DNS slots;
    /// sending SIP REGISTER to those addresses produces a misleading timeout.
    pub fn resolve_pcscf_for(&self, local: IpAddr) -> Result<IpAddr, VolteError> {
        if let Some(p) = self
            .pcscf
            .iter()
            .copied()
            .find(|candidate| same_family(local, *candidate))
        {
            return Ok(p);
        }
        Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))
    }

    /// Validate the family invariant: local addr and P-CSCF must share family.
    pub fn ensure_family_match(&self, local: IpAddr, pcscf: IpAddr) -> Result<IpAddr, VolteError> {
        if !same_family(local, pcscf) {
            return Err(VolteError::new(code::PCSCF_FAMILY_MISMATCH));
        }
        Ok(pcscf)
    }
}

/// Parse a settings block that lists `Label: value` lines, tolerant of the
/// modem/`mmcli` style output. Recognizes the IPv4/IPv6 address/gateway/DNS
/// labels and optional `P-CSCF:` lines.
pub fn parse_ip_settings(block: &str) -> ImsIpSettings {
    let mut s = ImsIpSettings::default();
    for line in block.lines() {
        let Some((label, value)) = line.split_once(':') else {
            continue;
        };
        let label = label.trim().to_ascii_lowercase();
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match label.as_str() {
            "ipv6 address" => s.ipv6_address = parse_addr(value),
            "ipv6 gateway address" | "ipv6 gateway" => s.ipv6_gateway = parse_addr(value),
            "ipv6 primary dns" | "ipv6 secondary dns" => push_addr(&mut s.ipv6_dns, value),
            "ipv4 address" => s.ipv4_address = parse_addr(value),
            "ipv4 gateway address" | "ipv4 gateway" => s.ipv4_gateway = parse_addr(value),
            "ipv4 primary dns" | "ipv4 secondary dns" => push_addr(&mut s.ipv4_dns, value),
            "p-cscf" | "pcscf" => push_addr(&mut s.pcscf, value),
            _ => {}
        }
    }
    s
}

/// The IMS PDP context id used for P-CSCF discovery and the IPv6 WDS preflight.
/// Honors `SIMADMIN_VOLTE_IMS_CID` (1..=16), else falls back to CID 2. Exposed
/// so callers that skip the AT probe (e.g. when it is non-fatal and returns no
/// candidates) still have a stable CID hint for the preflight.
pub fn configured_ims_cid() -> u8 {
    std::env::var(ENV_IMS_CID)
        .ok()
        .and_then(|value| value.trim().parse::<u8>().ok())
        .filter(|value| (1..=16).contains(value))
        .unwrap_or(DEFAULT_IMS_CID)
}

/// Locate the modem's IMS PDP context, creating an inactive definition only
/// when no IMS context exists. This function never activates or deactivates a
/// context; the WDS bearer remains the sole owner of activation.
pub async fn prepare_ims_profile_context(
    modem: &str,
    plan: &ImsConnectionPlan,
) -> Result<ImsProfileContext, VolteError> {
    let contexts_output = run_at(modem, "AT+CGDCONT?").await?;
    let contexts = parse_pdp_contexts(&contexts_output);
    let preferred = configured_ims_cid();

    if let Some(context) = contexts
        .iter()
        .filter(|context| context.apn.eq_ignore_ascii_case("ims"))
        .min_by_key(|context| (context.cid != preferred, context.cid))
    {
        return Ok(ImsProfileContext {
            cid: context.cid,
            created: false,
        });
    }

    let cid = if !contexts.iter().any(|context| context.cid == preferred) {
        preferred
    } else {
        (1..=16)
            .find(|candidate| !contexts.iter().any(|context| context.cid == *candidate))
            .ok_or_else(|| {
                VolteError::with_detail(
                    code::RUNTIME_PROFILE_PCSCF_MISSING,
                    "ims_profile_no_free_cid".to_string(),
                )
            })?
    };
    let pdp_type = plan.pdp_types().into_iter().next().unwrap_or("IPV4V6");
    run_at(modem, &format!("AT+CGDCONT={cid},\"{pdp_type}\",\"ims\"")).await?;
    Ok(ImsProfileContext { cid, created: true })
}

/// Enable or disable Qualcomm P-CSCF delivery for one IMS profile.
///
/// Deliberately does not issue `AT+CGACT`: that activation sequence caused a
/// baseband restart on the reference MSM8916 firmware. The following native or
/// ModemManager bearer activation consumes this setting safely.
pub async fn set_pcscf_reporting(modem: &str, cid: u8, enabled: bool) -> Result<(), VolteError> {
    let value = if enabled { "1,1,1" } else { "0,0,0" };
    run_at(modem, &format!("AT$QCPDPIMSCFGE={cid},{value}"))
        .await
        .map(|_| ())
}

/// Reproduce beta2's pre-bearer IMS profile sequence.
///
/// IDA shows the working binary performing, for one profile and PDP type:
/// `CGACT=0`, `CGDCONT=<cid>,<type>,ims`, `$QCPDPIMSCFGE=<cid>,1,1,1`,
/// `CGACT=1`, then repeated `CGCONTRDP=<cid>` reads. The resulting P-CSCF list
/// is preferred over the later WDS/active-bearer/DNS fallbacks.
pub async fn prefetch_pcscf_from_ims_profile(
    modem: &str,
    plan: &ImsConnectionPlan,
) -> Result<ImsProfilePrefetch, VolteError> {
    let contexts_output = run_at(modem, "AT+CGDCONT?").await?;
    let contexts = parse_pdp_contexts(&contexts_output);
    let mut profile_ids = Vec::with_capacity(3);
    push_profile_candidate(&mut profile_ids, configured_ims_cid());
    for cid in BETA2_PROFILE_CANDIDATES {
        push_profile_candidate(&mut profile_ids, cid);
    }

    let pdp_types = plan.pdp_types();
    let mut last_error = None;
    for cid in profile_ids {
        let restore_command = restore_profile_command(&contexts, cid);
        for pdp_type in &pdp_types {
            let _ = run_at(modem, &format!("AT+CGACT=0,{cid}")).await;
            if let Err(error) =
                run_at(modem, &format!("AT+CGDCONT={cid},\"{pdp_type}\",\"ims\"")).await
            {
                last_error = Some(error);
                cleanup_profile_context(modem, cid, &restore_command).await;
                continue;
            }
            if let Err(error) = set_pcscf_reporting(modem, cid, true).await {
                last_error = Some(error);
                cleanup_profile_context(modem, cid, &restore_command).await;
                continue;
            }
            match run_at(modem, &format!("AT+CGACT=1,{cid}")).await {
                Ok(_) => {
                    let mut candidates = Vec::new();
                    for round in 0..PROFILE_PCSCF_READ_ROUNDS {
                        if round > 0 {
                            sleep(PROFILE_PCSCF_READ_DELAY).await;
                        }
                        match run_at(modem, &format!("AT+CGCONTRDP={cid}")).await {
                            Ok(settings) => {
                                for candidate in parse_cgcontrdp_pcscf(&settings, cid) {
                                    if !candidates.contains(&candidate) {
                                        candidates.push(candidate);
                                    }
                                }
                                if !candidates.is_empty() {
                                    break;
                                }
                            }
                            Err(error) => tracing::debug!(
                                cid,
                                round,
                                error = %error,
                                "IMS profile CGCONTRDP prefetch read failed"
                            ),
                        }
                    }
                    return Ok(ImsProfilePrefetch {
                        candidates,
                        lease: ImsProfileLease {
                            modem: modem.to_string(),
                            cid,
                            restore_command,
                        },
                    });
                }
                Err(error) => {
                    last_error = Some(error);
                    cleanup_profile_context(modem, cid, &restore_command).await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        VolteError::with_detail(
            code::RUNTIME_PROFILE_PCSCF_MISSING,
            "beta2_profile_candidates_exhausted".to_string(),
        )
    }))
}

fn push_profile_candidate(candidates: &mut Vec<u8>, cid: u8) {
    if (1..=16).contains(&cid) && !candidates.contains(&cid) {
        candidates.push(cid);
    }
}

fn restore_profile_command(contexts: &[PdpContext], cid: u8) -> String {
    contexts
        .iter()
        .find(|context| context.cid == cid)
        .map(|context| {
            format!(
                "AT+CGDCONT={cid},\"{}\",\"{}\"",
                context.pdp_type, context.apn
            )
        })
        .unwrap_or_else(|| format!("AT+CGDCONT={cid},\"IPV4V6\",\"\""))
}

async fn cleanup_profile_context(modem: &str, cid: u8, restore_command: &str) {
    let _ = run_at(modem, &format!("AT+CGACT=0,{cid}")).await;
    let _ = set_pcscf_reporting(modem, cid, false).await;
    let _ = run_at(modem, restore_command).await;
}

/// Discover P-CSCF candidates from the IMS context that ModemManager already
/// activated for the connected bearer.
///
/// This fallback is deliberately read-only. Reconfiguring or toggling a fixed
/// CID here races ModemManager and can tear down the bearer whose PCO we are
/// trying to inspect. `SIMADMIN_VOLTE_IMS_CID` is only a preference when more
/// than one active IMS context exists; it never causes a context to be changed.
pub async fn discover_pcscf_via_active_at_context(
    modem: &str,
    _plan: &ImsConnectionPlan,
) -> Result<AtPcscfDiscovery, VolteError> {
    let active_output = run_at(modem, "AT+CGACT?").await?;
    let contexts_output = run_at(modem, "AT+CGDCONT?").await?;
    let mut active_cids = parse_active_context_cids(&active_output);
    let configured_ims_cids = parse_ims_context_cids(&contexts_output);
    let preferred_cid = configured_ims_cid();
    active_cids.sort_by_key(|cid| {
        (
            if *cid == preferred_cid { 0 } else { 1 },
            if configured_ims_cids.contains(cid) {
                0
            } else {
                1
            },
        )
    });

    if active_cids.is_empty() {
        return Err(VolteError::with_detail(
            code::RUNTIME_ALL_PCSCF_FAILED,
            format!("at_active_context_missing:ims={configured_ims_cids:?}"),
        ));
    }

    let mut attempted = Vec::with_capacity(active_cids.len());
    for cid in active_cids {
        attempted.push(cid);
        match run_at(modem, &format!("AT+CGCONTRDP={cid}")).await {
            Ok(settings) => {
                let candidates = parse_cgcontrdp_pcscf(&settings, cid);
                if !candidates.is_empty() {
                    return Ok(AtPcscfDiscovery { candidates, cid });
                }
            }
            Err(error) => {
                tracing::debug!(cid, error = %error, "VoLTE active-context CGCONTRDP query failed");
            }
        }
    }
    Err(VolteError::with_detail(
        code::RUNTIME_ALL_PCSCF_FAILED,
        format!("at_active_ims_context_no_pcscf:cids={attempted:?}:ims={configured_ims_cids:?}"),
    ))
}

/// Read the full IP configuration (address, gateway, DNS, prefix, P-CSCF) of one
/// IMS context via `AT+CGCONTRDP`.
///
/// This is beta2's IMS source of truth: after the WDS session is up, the modem
/// describes the context here (`Native VoLTE P-CSCF candidates discovered from
/// active IMS bearer`, `volte.rs:3671`), so the native bearer reads its addresses
/// and P-CSCF from this rather than from `--wds-get-current-settings`.
pub async fn read_cgcontrdp_settings(
    modem: &str,
    cid: u8,
) -> Result<CgcontrdpSettings, VolteError> {
    let output = run_at(modem, &format!("AT+CGCONTRDP={cid}")).await?;
    Ok(parse_cgcontrdp_settings(&output, cid))
}

async fn run_at(modem: &str, command: &str) -> Result<String, VolteError> {
    let argument = format!("--command={command}");
    let output = Command::new("mmcli")
        .args(["-m", modem, &argument])
        .output()
        .await
        .map_err(|error| {
            VolteError::with_detail(code::COMMAND_SPAWN_FAILED, format!("mmcli:{error}"))
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr)
            .trim()
            .replace('\n', " ");
        Err(VolteError::with_detail(
            code::COMMAND_FAILED,
            format!(
                "mmcli:{}:-m {modem} {argument}:{stderr}",
                output.status.code().unwrap_or(-1)
            ),
        ))
    }
}

fn parse_active_context_cids(output: &str) -> Vec<u8> {
    let mut cids = Vec::new();
    for line in output.lines() {
        let Some((_, values)) = line.split_once("+CGACT:") else {
            continue;
        };
        let fields: Vec<&str> = values.split(',').map(|field| field.trim()).collect();
        if fields.len() < 2 || fields[1].trim_matches('\'') != "1" {
            continue;
        }
        if let Ok(cid) = fields[0].trim_matches('\'').parse::<u8>() {
            if !cids.contains(&cid) {
                cids.push(cid);
            }
        }
    }
    cids
}

fn parse_ims_context_cids(output: &str) -> Vec<u8> {
    parse_pdp_contexts(output)
        .into_iter()
        .filter(|context| context.apn.eq_ignore_ascii_case("ims"))
        .map(|context| context.cid)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PdpContext {
    cid: u8,
    pdp_type: String,
    apn: String,
}

fn parse_pdp_contexts(output: &str) -> Vec<PdpContext> {
    let mut contexts = Vec::new();
    for line in output.lines() {
        let Some((_, values)) = line.split_once("+CGDCONT:") else {
            continue;
        };
        let fields: Vec<&str> = values.split(',').map(|field| field.trim()).collect();
        if fields.len() < 3 {
            continue;
        }
        let Ok(cid) = fields[0].trim_matches(['\'', '"']).parse::<u8>() else {
            continue;
        };
        if contexts
            .iter()
            .any(|context: &PdpContext| context.cid == cid)
        {
            continue;
        }
        contexts.push(PdpContext {
            cid,
            pdp_type: fields[1].trim_matches(['\'', '"']).to_string(),
            apn: fields[2].trim_matches(['\'', '"']).to_string(),
        });
    }
    contexts
}

/// Parse the primary/secondary P-CSCF columns from a 3GPP +CGCONTRDP response.
/// Qualcomm renders IPv6 values as 16 dot-separated decimal octets.
pub fn parse_cgcontrdp_pcscf(output: &str, expected_cid: u8) -> Vec<IpAddr> {
    let mut candidates = Vec::new();
    for line in output.lines() {
        let Some((_, values)) = line.split_once("+CGCONTRDP:") else {
            continue;
        };
        let fields: Vec<&str> = values.split(',').map(|field| field.trim()).collect();
        if fields.len() < 8
            || fields[0].parse::<u8>().ok() != Some(expected_cid)
            || !fields[2]
                .trim_matches(['\'', '"'])
                .eq_ignore_ascii_case("ims")
        {
            continue;
        }
        for field in fields.iter().skip(7).take(2) {
            for address in parse_cgcontrdp_addresses(field) {
                if !candidates.contains(&address) {
                    candidates.push(address);
                }
            }
        }
    }
    candidates
}

/// Full IP configuration read back from `+CGCONTRDP`, matching what beta2 pulls
/// from an active IMS bearer (`volte.rs:3671`, "P-CSCF candidates discovered from
/// active IMS bearer").
///
/// The 3GPP field layout (TS 27.007) for one line is:
/// `<cid>,<bearer_id>,<apn>,<local_addr_and_mask>,<gw>,<dns1>,<dns2>,<pcscf1>,<pcscf2>,...`
/// Qualcomm renders the local-address-and-mask field as address octets followed
/// by mask octets (8 decimals for IPv4, 32 for IPv6), which is where the prefix
/// length is recovered from.
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

impl CgcontrdpSettings {
    pub fn is_empty(&self) -> bool {
        self.ipv4_address.is_none() && self.ipv6_address.is_none() && self.pcscf.is_empty()
    }
}

/// Parse the full IP configuration (address, gateway, DNS, P-CSCF) for one CID
/// from a `+CGCONTRDP` response. This is beta2's primary IMS source, so it reads
/// every field, not just the P-CSCF columns.
pub fn parse_cgcontrdp_settings(output: &str, expected_cid: u8) -> CgcontrdpSettings {
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
                .is_some_and(|apn| apn.trim_matches(['\'', '"']).eq_ignore_ascii_case("ims"))
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
        if let Some(gateway) = fields.get(4).and_then(|f| parse_cgcontrdp_addresses(f).into_iter().next()) {
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

fn parse_cgcontrdp_addresses(field: &str) -> Vec<IpAddr> {
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

/// Discover a P-CSCF without changing the system resolver. IMS APNs commonly
/// provide private DNS servers that are reachable only through the dedicated
/// bearer, so queries are sent directly from the bearer address.
pub async fn discover_pcscf(
    settings: &ImsIpSettings,
    home_domain: &str,
    local: IpAddr,
) -> Result<IpAddr, VolteError> {
    if let Ok(explicit) = std::env::var(ENV_PCSCF) {
        if let Some(address) = parse_pcscf_override(&explicit)
            .into_iter()
            .find(|candidate| same_family(local, *candidate))
        {
            return settings.ensure_family_match(local, address);
        }
    }
    if let Ok(address) = settings.resolve_pcscf_for(local) {
        return settings.ensure_family_match(local, address);
    }

    let dns_servers = if local.is_ipv6() {
        &settings.ipv6_dns
    } else {
        &settings.ipv4_dns
    };
    let pcscf_name = format!("pcscf.{home_domain}");
    let srv_names = pcscf_srv_names(home_domain);

    for server in dns_servers {
        if server.is_ipv4() != local.is_ipv4() {
            continue;
        }
        let address_type = if local.is_ipv6() { 28 } else { 1 };
        match query_dns(local, *server, &pcscf_name, address_type).await {
            Ok(records) => {
                if let Some(address) = records
                    .addresses
                    .into_iter()
                    .find(|item| item.is_ipv4() == local.is_ipv4())
                {
                    tracing::info!(dns_server = %server, name = %pcscf_name, %address, "VoLTE P-CSCF discovered by DNS address query");
                    return Ok(address);
                }
                tracing::debug!(dns_server = %server, name = %pcscf_name, record_type = address_type, "VoLTE P-CSCF DNS address query returned no matching address");
            }
            Err(error) => {
                tracing::debug!(dns_server = %server, name = %pcscf_name, record_type = address_type, error = %error, "VoLTE P-CSCF DNS address query failed")
            }
        }

        for srv_name in &srv_names {
            let records = match query_dns(local, *server, srv_name, 33).await {
                Ok(records) => records,
                Err(error) => {
                    tracing::debug!(dns_server = %server, name = %srv_name, error = %error, "VoLTE P-CSCF DNS SRV query failed");
                    continue;
                }
            };
            for target in records.srv_targets {
                if let Ok(target_records) = query_dns(local, *server, &target, address_type).await {
                    if let Some(address) = target_records
                        .addresses
                        .into_iter()
                        .find(|item| item.is_ipv4() == local.is_ipv4())
                    {
                        tracing::info!(dns_server = %server, name = %srv_name, target = %target, %address, "VoLTE P-CSCF discovered by DNS SRV query");
                        return Ok(address);
                    }
                }
            }
        }
    }
    Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))
}

fn pcscf_srv_names(home_domain: &str) -> Vec<String> {
    vec![
        format!("_sip._udp.pcscf.{home_domain}"),
        format!("_sip._tcp.pcscf.{home_domain}"),
        format!("_sip._udp.{home_domain}"),
        format!("_sip._tcp.{home_domain}"),
    ]
}

pub fn pcscf_socket(address: IpAddr) -> SocketAddr {
    SocketAddr::new(address, SIP_PORT)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DnsRecords {
    addresses: Vec<IpAddr>,
    srv_targets: Vec<String>,
}

async fn query_dns(
    local: IpAddr,
    server: IpAddr,
    name: &str,
    record_type: u16,
) -> Result<DnsRecords, VolteError> {
    let query_id = dns_query_id(name, record_type);
    let query = build_dns_query(query_id, name, record_type)?;
    let socket = UdpSocket::bind(SocketAddr::new(local, 0))
        .await
        .map_err(|_| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
    socket
        .send_to(&query, SocketAddr::new(server, DNS_PORT))
        .await
        .map_err(|_| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
    let mut response = [0u8; 4096];
    let (read, _) = tokio::time::timeout(DNS_TIMEOUT, socket.recv_from(&mut response))
        .await
        .map_err(|_| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?
        .map_err(|_| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
    parse_dns_response(query_id, &response[..read])
}

fn dns_query_id(name: &str, record_type: u16) -> u16 {
    let mut hash = 0x5a17u16 ^ record_type;
    for byte in name.bytes() {
        hash = hash.rotate_left(5) ^ u16::from(byte);
    }
    hash
}

fn build_dns_query(id: u16, name: &str, record_type: u16) -> Result<Vec<u8>, VolteError> {
    let mut query = Vec::with_capacity(64 + name.len());
    query.extend_from_slice(&id.to_be_bytes());
    query.extend_from_slice(&0x0100u16.to_be_bytes());
    query.extend_from_slice(&1u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED));
        }
        query.push(label.len() as u8);
        query.extend_from_slice(label.as_bytes());
    }
    query.push(0);
    query.extend_from_slice(&record_type.to_be_bytes());
    query.extend_from_slice(&1u16.to_be_bytes());
    Ok(query)
}

fn parse_dns_response(id: u16, packet: &[u8]) -> Result<DnsRecords, VolteError> {
    if packet.len() < 12 || u16::from_be_bytes([packet[0], packet[1]]) != id {
        return Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED));
    }
    let flags = u16::from_be_bytes([packet[2], packet[3]]);
    if flags & 0x000f != 0 {
        return Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED));
    }
    let questions = usize::from(u16::from_be_bytes([packet[4], packet[5]]));
    let answers = usize::from(u16::from_be_bytes([packet[6], packet[7]]));
    let authorities = usize::from(u16::from_be_bytes([packet[8], packet[9]]));
    let additional = usize::from(u16::from_be_bytes([packet[10], packet[11]]));
    let mut offset = 12usize;
    for _ in 0..questions {
        offset = read_dns_name(packet, offset)?.1;
        offset = offset
            .checked_add(4)
            .filter(|end| *end <= packet.len())
            .ok_or_else(|| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
    }

    let mut records = DnsRecords::default();
    for _ in 0..answers + authorities + additional {
        offset = read_dns_name(packet, offset)?.1;
        if offset + 10 > packet.len() {
            return Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED));
        }
        let record_type = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
        let length = usize::from(u16::from_be_bytes([packet[offset + 8], packet[offset + 9]]));
        let data_offset = offset + 10;
        let data_end = data_offset
            .checked_add(length)
            .filter(|end| *end <= packet.len())
            .ok_or_else(|| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
        match (record_type, length) {
            (1, 4) => records.addresses.push(IpAddr::V4(Ipv4Addr::new(
                packet[data_offset],
                packet[data_offset + 1],
                packet[data_offset + 2],
                packet[data_offset + 3],
            ))),
            (28, 16) => {
                let octets: [u8; 16] = packet[data_offset..data_end]
                    .try_into()
                    .map_err(|_| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
                records.addresses.push(IpAddr::V6(Ipv6Addr::from(octets)));
            }
            (33, 6..) => {
                let (target, _) = read_dns_name(packet, data_offset + 6)?;
                if !target.is_empty() && !records.srv_targets.contains(&target) {
                    records.srv_targets.push(target);
                }
            }
            _ => {}
        }
        offset = data_end;
    }
    Ok(records)
}

fn read_dns_name(packet: &[u8], start: usize) -> Result<(String, usize), VolteError> {
    let mut labels = Vec::new();
    let mut offset = start;
    let mut end = None;
    for _ in 0..128 {
        let length = *packet
            .get(offset)
            .ok_or_else(|| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
        if length == 0 {
            return Ok((labels.join("."), end.unwrap_or(offset + 1)));
        }
        if length & 0xc0 == 0xc0 {
            let low = *packet
                .get(offset + 1)
                .ok_or_else(|| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
            end.get_or_insert(offset + 2);
            offset = (usize::from(length & 0x3f) << 8) | usize::from(low);
            continue;
        }
        if length & 0xc0 != 0 {
            return Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED));
        }
        let label_start = offset + 1;
        let label_end = label_start + usize::from(length);
        let label = std::str::from_utf8(
            packet
                .get(label_start..label_end)
                .ok_or_else(|| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?,
        )
        .map_err(|_| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))?;
        labels.push(label.to_string());
        offset = label_end;
    }
    Err(VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))
}

/// Strip a possible prefix length / netmask suffix and parse an IP.
fn parse_addr(value: &str) -> Option<IpAddr> {
    let head = value
        .split_whitespace()
        .next()
        .unwrap_or(value)
        .split('/')
        .next()
        .unwrap_or(value);
    head.parse::<IpAddr>().ok()
}

fn push_addr(list: &mut Vec<IpAddr>, value: &str) {
    if let Some(addr) = parse_addr(value) {
        if !list.contains(&addr) {
            list.push(addr);
        }
    }
}

fn push_optional_addr(list: &mut Vec<IpAddr>, value: Option<IpAddr>) {
    if let Some(value) = value {
        if !list.contains(&value) {
            list.push(value);
        }
    }
}

fn same_family(left: IpAddr, right: IpAddr) -> bool {
    left.is_ipv4() == right.is_ipv4()
}

fn parse_pcscf_override(value: &str) -> Vec<IpAddr> {
    value
        .split(|character: char| character == ',' || character == ';' || character.is_whitespace())
        .filter_map(|candidate| candidate.trim().parse::<IpAddr>().ok())
        .fold(Vec::new(), |mut addresses, address| {
            if !addresses.contains(&address) {
                addresses.push(address);
            }
            addresses
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    const SAMPLE: &str = "\
IPv6 address: 2001:db8::2/64
IPv6 gateway address: 2001:db8::1
IPv6 primary DNS: 2001:db8::53
IPv6 secondary DNS: 2001:db8::54
IPv4 address: 10.0.0.2
IPv4 gateway address: 10.0.0.1
IPv4 primary DNS: 10.0.0.53";

    #[test]
    fn parses_ipv6_and_ipv4_blocks() {
        let s = parse_ip_settings(SAMPLE);
        assert_eq!(
            s.ipv6_address,
            Some(IpAddr::V6("2001:db8::2".parse::<Ipv6Addr>().unwrap()))
        );
        assert_eq!(
            s.ipv6_gateway,
            Some(IpAddr::V6("2001:db8::1".parse::<Ipv6Addr>().unwrap()))
        );
        assert_eq!(s.ipv6_dns.len(), 2);
        assert_eq!(s.ipv4_address, Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))));
    }

    #[test]
    fn address_order_honors_preference_and_strict_modes() {
        use crate::connectivity::modems::softstack::volte::plan::ImsConnectionPlan;
        let s = parse_ip_settings(SAMPLE);
        assert_eq!(
            s.ordered_local_addrs(&ImsConnectionPlan::from_preference(
                VolteIpFamilyPreference::Ipv6First
            )),
            vec![
                IpAddr::V6("2001:db8::2".parse::<Ipv6Addr>().unwrap()),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            ]
        );
        assert_eq!(
            s.ordered_local_addrs(&ImsConnectionPlan::from_preference(
                VolteIpFamilyPreference::Ipv4First
            )),
            vec![
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                IpAddr::V6("2001:db8::2".parse::<Ipv6Addr>().unwrap()),
            ]
        );
        assert_eq!(
            s.ordered_local_addrs(&ImsConnectionPlan::from_preference(
                VolteIpFamilyPreference::Ipv6Only
            )),
            vec![IpAddr::V6("2001:db8::2".parse::<Ipv6Addr>().unwrap())]
        );
        assert_eq!(
            s.ordered_local_addrs(&ImsConnectionPlan::from_preference(
                VolteIpFamilyPreference::Ipv4Only
            )),
            vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))]
        );
    }

    #[test]
    fn resolve_pcscf_accepts_only_explicit_pco_address() {
        let mut s = parse_ip_settings(SAMPLE);
        assert_eq!(
            s.resolve_pcscf_for(IpAddr::V6("2001:db8::2".parse().unwrap()))
                .unwrap_err()
                .code(),
            code::RUNTIME_ALL_PCSCF_FAILED
        );
        s.pcscf
            .push(IpAddr::V6("2001:db8::99".parse::<Ipv6Addr>().unwrap()));
        assert_eq!(
            s.resolve_pcscf_for(IpAddr::V6("2001:db8::2".parse().unwrap()))
                .unwrap(),
            IpAddr::V6("2001:db8::99".parse::<Ipv6Addr>().unwrap())
        );
    }

    #[test]
    fn resolve_pcscf_errors_when_nothing() {
        let s = ImsIpSettings::default();
        assert_eq!(
            s.resolve_pcscf_for(IpAddr::V6(Ipv6Addr::LOCALHOST))
                .unwrap_err()
                .code(),
            code::RUNTIME_ALL_PCSCF_FAILED
        );
    }

    #[test]
    fn family_mismatch_detected() {
        let s = parse_ip_settings("IPv6 address: 2001:db8::2");
        let v4 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        assert_eq!(
            s.ensure_family_match(s.local_addr().unwrap(), v4)
                .unwrap_err()
                .code(),
            code::PCSCF_FAMILY_MISMATCH
        );
        let v6 = IpAddr::V6("2001:db8::1".parse::<Ipv6Addr>().unwrap());
        assert_eq!(
            s.ensure_family_match(s.local_addr().unwrap(), v6).unwrap(),
            v6
        );
    }

    #[test]
    fn explicit_pcscf_and_override_candidates_are_filtered_by_family() {
        let mut s = parse_ip_settings(SAMPLE);
        let v4 = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
        let v6 = IpAddr::V6("2001:db8::99".parse().unwrap());
        s.pcscf.extend([v6, v4]);

        assert_eq!(s.resolve_pcscf_for(s.ipv4_address.unwrap()).unwrap(), v4);
        assert_eq!(s.resolve_pcscf_for(s.ipv6_address.unwrap()).unwrap(), v6);
        assert_eq!(
            parse_pcscf_override("2001:db8::99, 192.0.2.10;invalid 192.0.2.10"),
            vec![v6, v4]
        );
    }

    #[test]
    fn parses_qualcomm_cgcontrdp_pcscf_columns() {
        let response = "response: '+CGCONTRDP: 2,5,ims,36.14.87.128.10.128.45.91.1.2.3.4.5.6.7.8,36.14.87.128.10.128.45.91.8.7.6.5.4.3.2.1,36.14.0.90.0.0.0.0.0.0.0.0.0.102.102.36,36.14.0.91.0.0.0.0.0.0.0.0.0.102.102.254,36.14.0.46.130.1.192.0.0.9.0.0.0.0.0.1,36.14.0.46.130.1.192.0.0.9.0.0.0.0.0.2'";
        assert_eq!(
            parse_cgcontrdp_pcscf(response, 2),
            vec![
                "240e:2e:8201:c000:9::1".parse::<IpAddr>().unwrap(),
                "240e:2e:8201:c000:9::2".parse::<IpAddr>().unwrap(),
            ]
        );
        assert!(parse_cgcontrdp_pcscf(response, 3).is_empty());

        let internet_context = response.replace(",ims,", ",internet,");
        assert!(parse_cgcontrdp_pcscf(&internet_context, 2).is_empty());
    }

    #[test]
    fn at_probe_family_order_matches_runtime_preference() {
        use crate::connectivity::modems::softstack::volte::plan::ImsConnectionPlan;
        assert_eq!(
            ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv6First).pdp_types(),
            vec!["IPV4V6", "IPV6", "IP"]
        );
        assert_eq!(
            ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4First).pdp_types(),
            vec!["IPV4V6", "IP", "IPV6"]
        );
        assert_eq!(
            ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv6Only).pdp_types(),
            vec!["IPV6"]
        );
        assert_eq!(
            ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4Only).pdp_types(),
            vec!["IP"]
        );
    }

    #[test]
    fn selects_only_active_ims_contexts_for_read_only_discovery() {
        let contexts = "response: '+CGDCONT: 1,\"IPV4V6\",\"ctnet\",\"0.0.0.0\",0,0\n+CGDCONT: 3,\"IPV4V6\",\"ims\",\"0.0.0.0\",0,0\n+CGDCONT: 7,\"IPV6\",\"IMS\",\"0.0.0.0\",0,0'";
        let active = "response: '+CGACT: 1,1\n+CGACT: 3,0\n+CGACT: 7,1'";
        assert_eq!(parse_ims_context_cids(contexts), vec![3, 7]);
        assert_eq!(parse_active_context_cids(active), vec![1, 7]);
        let active_cids = parse_active_context_cids(active);
        let selected = parse_ims_context_cids(contexts)
            .into_iter()
            .filter(|cid| active_cids.contains(cid))
            .collect::<Vec<_>>();
        assert_eq!(selected, vec![7]);
    }

    #[test]
    fn parses_pdp_profiles_for_ims_profile_selection() {
        let contexts = "response: '+CGDCONT: 1,\"IPV4V6\",\"internet\",\"0.0.0.0\",0,0\n+CGDCONT: 2,\"IPV4V6\",\"ims\",\"0.0.0.0\",0,0\n+CGDCONT: 7,\"IPV6\",\"IMS\",\"0.0.0.0\",0,0'";
        assert_eq!(
            parse_pdp_contexts(contexts),
            vec![
                PdpContext {
                    cid: 1,
                    pdp_type: "IPV4V6".to_string(),
                    apn: "internet".to_string(),
                },
                PdpContext {
                    cid: 2,
                    pdp_type: "IPV4V6".to_string(),
                    apn: "ims".to_string(),
                },
                PdpContext {
                    cid: 7,
                    pdp_type: "IPV6".to_string(),
                    apn: "IMS".to_string(),
                },
            ]
        );
        assert_eq!(parse_ims_context_cids(contexts), vec![2, 7]);
    }

    #[test]
    fn beta2_profile_cleanup_restores_the_previous_definition() {
        let contexts = parse_pdp_contexts(
            "response: '+CGDCONT: 1,\"IPV4V6\",\"internet\"\n+CGDCONT: 2,\"IPV6\",\"private-ims\"'",
        );
        assert_eq!(
            restore_profile_command(&contexts, 2),
            "AT+CGDCONT=2,\"IPV6\",\"private-ims\""
        );
        assert_eq!(
            restore_profile_command(&contexts, 3),
            "AT+CGDCONT=3,\"IPV4V6\",\"\""
        );
    }

    #[test]
    fn beta2_profile_candidates_are_unique_and_bounded() {
        let mut candidates = Vec::new();
        for cid in [2, 2, 1, 0, 17, 3] {
            push_profile_candidate(&mut candidates, cid);
        }
        assert_eq!(candidates, vec![2, 1, 3]);
    }

    #[test]
    fn pcscf_dns_srv_names_try_3gpp_pcscf_domain_first() {
        assert_eq!(
            pcscf_srv_names("ims.mnc001.mcc001.3gppnetwork.org"),
            vec![
                "_sip._udp.pcscf.ims.mnc001.mcc001.3gppnetwork.org",
                "_sip._tcp.pcscf.ims.mnc001.mcc001.3gppnetwork.org",
                "_sip._udp.ims.mnc001.mcc001.3gppnetwork.org",
                "_sip._tcp.ims.mnc001.mcc001.3gppnetwork.org",
            ]
        );
    }

    #[test]
    fn parse_addr_strips_prefix_len() {
        assert_eq!(
            parse_addr("2001:db8::2/64"),
            Some(IpAddr::V6("2001:db8::2".parse::<Ipv6Addr>().unwrap()))
        );
        assert_eq!(
            parse_addr("10.0.0.2 255.255.255.0"),
            Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)))
        );
    }

    #[test]
    fn parses_compressed_aaaa_dns_answer() {
        let id = 0x1234;
        let name = "pcscf.ims.example";
        let query = build_dns_query(id, name, 28).unwrap();
        let mut packet = query.clone();
        packet[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
        packet[6..8].copy_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&[0xc0, 0x0c]);
        packet.extend_from_slice(&28u16.to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&60u32.to_be_bytes());
        packet.extend_from_slice(&16u16.to_be_bytes());
        packet.extend_from_slice(&Ipv6Addr::LOCALHOST.octets());

        let records = parse_dns_response(id, &packet).unwrap();
        assert_eq!(records.addresses, vec![IpAddr::V6(Ipv6Addr::LOCALHOST)]);
    }

    #[test]
    fn pcscf_socket_uses_standard_sip_port() {
        assert_eq!(pcscf_socket(IpAddr::V4(Ipv4Addr::LOCALHOST)).port(), 5060);
    }
}
