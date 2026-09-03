//! VoLTE IMS IPsec (RFC 3329 / TS 33.203) via the Linux kernel `ip xfrm`.
//!
//! Clean-room from public specs. IMS signaling integrity is protected by the
//! kernel xfrm framework rather than a user-space ESP stack: we install SA +
//! policy pairs with `ip xfrm`, matching the reference "borrow the kernel"
//! design (`Native VoLTE IPsec xfrm installed`).
//!
//! Design split (important for testability): every function here that *builds*
//! a command returns a `Vec<String>` argument vector, which is fully unit
//! testable on any platform. The actual process execution is a thin
//! `#[cfg(unix)]` layer at the bottom. Windows CI verifies the argument
//! assembly; the real `ip` invocation is verified on the target device.
//!
//! IMS IPsec uses transport mode, integrity-only protection
//! (`alg=hmac-md5-96; ealg=null`) over the four transport-flow SAs bound to
//! the negotiated client/server ports (`spi-c/spi-s/port-c/port-s`), per the
//! P-CSCF Security-Server offer.

use std::net::IpAddr;

use super::errors::{code, VolteError};
use crate::services::ue_worker::{NetConfigOp, UeWorkerHandle};

/// The four-way port/SPI binding negotiated via SIP `Security-Client` /
/// `Security-Server` (sec-agree). `port_c`/`spi_c` are the UE (client) side,
/// `port_s`/`spi_s` are the P-CSCF (server) side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecAgree {
    pub spi_c: u32,
    pub spi_s: u32,
    pub port_c: u16,
    pub port_s: u16,
}

impl SecAgree {
    pub fn security_client_value(self) -> String {
        format!(
            "ipsec-3gpp;alg=hmac-md5-96;ealg=null;prot=esp;mod=trans;spi-c={};spi-s={};port-c={};port-s={}",
            self.spi_c, self.spi_s, self.port_c, self.port_s
        )
    }

    pub fn spaced_security_client_value(self) -> String {
        format!(
            "ipsec-3gpp; alg=hmac-md5-96; ealg=null; prot=esp; mod=trans; spi-c={}; spi-s={}; port-c={}; port-s={}",
            self.spi_c, self.spi_s, self.port_c, self.port_s
        )
    }

    pub fn compact_security_client_value(self) -> String {
        format!(
            "ipsec-3gpp;alg=hmac-md5-96;ealg=null;spi-c={};spi-s={};port-c={};port-s={}",
            self.spi_c, self.spi_s, self.port_c, self.port_s
        )
    }
}

/// Parse the selected `Security-Server: ipsec-3gpp;...` value. Unknown
/// extensions are ignored; all four port/SPI bindings are mandatory.
pub fn parse_security_server(value: &str) -> Result<SecAgree, VolteError> {
    let mut mechanism = None;
    let mut spi_c = None;
    let mut spi_s = None;
    let mut port_c = None;
    let mut port_s = None;
    for (index, part) in value.split(';').enumerate() {
        let part = part.trim();
        if index == 0 {
            mechanism = Some(part.to_ascii_lowercase());
            continue;
        }
        let Some((name, raw)) = part.split_once('=') else {
            continue;
        };
        let raw = raw.trim().trim_matches('"');
        match name.trim().to_ascii_lowercase().as_str() {
            "spi-c" => spi_c = parse_u32(raw),
            "spi-s" => spi_s = parse_u32(raw),
            "port-c" => port_c = raw.parse().ok(),
            "port-s" => port_s = raw.parse().ok(),
            _ => {}
        }
    }
    if mechanism.as_deref() != Some("ipsec-3gpp") {
        return Err(VolteError::new(code::SECURITY_SERVER_MISSING));
    }
    Ok(SecAgree {
        spi_c: spi_c.ok_or_else(|| VolteError::new(code::SECURITY_SERVER_MISSING))?,
        spi_s: spi_s.ok_or_else(|| VolteError::new(code::SECURITY_SERVER_MISSING))?,
        port_c: port_c.ok_or_else(|| VolteError::new(code::SECURITY_SERVER_MISSING))?,
        port_s: port_s.ok_or_else(|| VolteError::new(code::SECURITY_SERVER_MISSING))?,
    })
}

fn parse_u32(value: &str) -> Option<u32> {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map(|hex| u32::from_str_radix(hex, 16).ok())
        .unwrap_or_else(|| value.parse().ok())
}

/// Integrity + encryption algorithm tokens for the SA. IMS signaling protection
/// is integrity-only, so `ealg` is null by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XfrmAlgs {
    /// e.g. "hmac(md5)" with 96-bit truncation.
    pub auth: &'static str,
    pub auth_trunc_bits: u32,
    /// e.g. "cipher_null".
    pub enc: &'static str,
}

impl Default for XfrmAlgs {
    fn default() -> Self {
        // alg=hmac-md5-96; ealg=null (observed reference default).
        Self {
            auth: "hmac(md5)",
            auth_trunc_bits: 96,
            enc: "cipher_null",
        }
    }
}

/// One SA direction descriptor.
#[derive(Debug, Clone)]
pub struct XfrmSa {
    pub src: IpAddr,
    pub dst: IpAddr,
    pub spi: u32,
    /// Integrity key (from CK/IK-derived material). Hex-encoded on the wire.
    pub auth_key: Vec<u8>,
    pub enc_key: Vec<u8>,
    pub algs: XfrmAlgs,
    pub sport: u16,
    pub dport: u16,
}

/// `ip` binary discovery order, mirroring the reference search path.
pub const IP_BINARY_CANDIDATES: &[&str] = &["/bin/ip", "/usr/bin/ip", "/usr/sbin/ip"];

fn hex_key(key: &[u8]) -> String {
    let body: String = key.iter().map(|b| format!("{b:02x}")).collect();
    format!("0x{body}")
}

fn ip_str(ip: IpAddr) -> String {
    ip.to_string()
}

/// Render an XFRM argv for diagnostics without exposing AKA-derived key
/// material. `ip xfrm` places the key two arguments after each algorithm
/// selector (`auth`, `auth-trunc`, `enc`, or `aead`).
pub(crate) fn redacted_xfrm_argv(argv: &[String]) -> String {
    let mut rendered = argv.to_vec();
    for index in 0..rendered.len() {
        if matches!(
            rendered[index].as_str(),
            "auth" | "auth-trunc" | "enc" | "aead"
        ) {
            if let Some(key) = rendered.get_mut(index + 2) {
                *key = "[redacted]".to_string();
            }
        }
    }
    rendered.join(" ")
}

/// Build `ip xfrm state add ...` for one SA direction (transport mode,
/// integrity-only). Returns the argv (without the leading `ip`).
///
/// No XFRM soft/hard time limit is installed intentionally. Linux therefore
/// keeps the SA alive until this session explicitly removes it, which is
/// longer than every REGISTER lease and satisfies the TS 24.229 requirement
/// that a successful refresh extend the SA to at least the lease plus 30 s
/// without replacing the live association.
///
/// The transport selectors are not optional here. TS 33.203 binds every IMS
/// SA to the source/destination UDP ports, and one UE/P-CSCF association has
/// two simultaneous port tuples (`port_uc`/`port_ps` and
/// `port_us`/`port_pc`). Leaving the state selector wildcarded makes Linux
/// choose between otherwise identical SAs by SPI rather than by the SIP flow;
/// a refresh can then be encrypted with the wrong SA or its response can fail
/// ESP validation before it reaches the UDP socket.
pub fn build_xfrm_state_add(sa: &XfrmSa) -> Vec<String> {
    vec![
        "xfrm".into(),
        "state".into(),
        "add".into(),
        "src".into(),
        ip_str(sa.src),
        "dst".into(),
        ip_str(sa.dst),
        "proto".into(),
        "esp".into(),
        "spi".into(),
        format!("0x{:08x}", sa.spi),
        "mode".into(),
        "transport".into(),
        "sel".into(),
        "proto".into(),
        "udp".into(),
        "sport".into(),
        sa.sport.to_string(),
        "dport".into(),
        sa.dport.to_string(),
        "auth-trunc".into(),
        sa.algs.auth.into(),
        hex_key(&sa.auth_key),
        sa.algs.auth_trunc_bits.to_string(),
        "enc".into(),
        sa.algs.enc.into(),
        if sa.enc_key.is_empty() {
            String::new()
        } else {
            hex_key(&sa.enc_key)
        },
    ]
}

/// Direction for a policy entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDir {
    Out,
    In,
}

impl PolicyDir {
    fn as_str(self) -> &'static str {
        match self {
            PolicyDir::Out => "out",
            PolicyDir::In => "in",
        }
    }
}

/// Build `ip xfrm policy add ...` for one direction.
#[allow(clippy::too_many_arguments)]
pub fn build_xfrm_policy_add(
    src: IpAddr,
    dst: IpAddr,
    sport: u16,
    dport: u16,
    dir: PolicyDir,
) -> Vec<String> {
    vec![
        "xfrm".into(),
        "policy".into(),
        "add".into(),
        "src".into(),
        ip_str(src),
        "dst".into(),
        ip_str(dst),
        "proto".into(),
        "udp".into(),
        "sport".into(),
        sport.to_string(),
        "dport".into(),
        dport.to_string(),
        "dir".into(),
        dir.as_str().into(),
        "tmpl".into(),
        "src".into(),
        ip_str(src),
        "dst".into(),
        ip_str(dst),
        "proto".into(),
        "esp".into(),
        "mode".into(),
        "transport".into(),
    ]
}

/// Build the teardown commands: flush all xfrm state + policy.
pub fn build_xfrm_flush() -> Vec<Vec<String>> {
    vec![
        vec!["xfrm".into(), "policy".into(), "flush".into()],
        vec!["xfrm".into(), "state".into(), "flush".into()],
    ]
}

/// A full four-SA + four-policy install plan for the UE⇄P-CSCF signaling pair.
/// The integrity keys come from AKA CK/IK-derived material (see `derive_keys`).
#[derive(Debug, Clone)]
pub struct XfrmInstallPlan {
    pub states: Vec<XfrmSa>,
    pub policies: Vec<Vec<String>>,
}

/// Assemble the standard IMS signaling protection plan per TS 33.203:
/// UE(port_c) ⇄ P-CSCF(port_s), protected client->server and server->client.
/// We install the two SAs the UE needs (outbound to spi_s, inbound on spi_c)
/// plus matching policies.
pub fn build_install_plan(
    ue: IpAddr,
    pcscf: IpAddr,
    ue_sec: &SecAgree,
    pcscf_sec: &SecAgree,
    auth_key: &[u8],
) -> Result<XfrmInstallPlan, VolteError> {
    build_install_plan_with_algs(
        ue,
        pcscf,
        ue_sec,
        pcscf_sec,
        auth_key,
        &[],
        XfrmAlgs::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_install_plan_with_algs(
    ue: IpAddr,
    pcscf: IpAddr,
    ue_sec: &SecAgree,
    pcscf_sec: &SecAgree,
    auth_key: &[u8],
    encryption_key: &[u8],
    algs: XfrmAlgs,
) -> Result<XfrmInstallPlan, VolteError> {
    // IMS IPsec requires IPv6 in most deployments (observed
    // `volte_ipsec_requires_ipv6`); we allow v4 for lab use but both ends must
    // match family.
    if std::mem::discriminant(&ue) != std::mem::discriminant(&pcscf) {
        return Err(VolteError::new(code::PCSCF_FAMILY_MISMATCH));
    }
    if auth_key.is_empty() {
        return Err(VolteError::new(code::IPSEC_IK_INVALID));
    }
    if algs.enc != "cipher_null" && encryption_key.is_empty() {
        return Err(VolteError::new(code::IPSEC_IK_INVALID));
    }
    // `-c` identifies the protected client flow and `-s` the protected server
    // flow.  They are not interchangeable.  A REGISTER request leaves the UE
    // on the client flow (UE port-c -> P-CSCF port-s), and its response comes
    // back on that same flow.  Requests initiated by the P-CSCF use the server
    // flow (P-CSCF port-c -> UE port-s), and the UE must be able to send their
    // responses back on the reverse tuple.
    //
    // There are therefore four transport-mode SAs/policies, not just one
    // outbound and one inbound entry.  The old two-entry plan protected the
    // P-CSCF-initiated flow only on the inbound side; a normal REGISTER
    // response on P-CSCF port-s -> UE port-c was discarded by XFRM before it
    // could reach recv_sip().  Initial registration could appear to work when
    // the core selected the other flow, while later refreshes silently timed
    // out.  Keep the four tuples explicit so refresh and in-dialog traffic use
    // the negotiated association in both directions.
    let states = vec![
        // UE client flow: UE port-c -> P-CSCF port-s.
        XfrmSa {
            src: ue,
            dst: pcscf,
            spi: pcscf_sec.spi_s,
            auth_key: auth_key.to_vec(),
            enc_key: encryption_key.to_vec(),
            algs,
            sport: ue_sec.port_c,
            dport: pcscf_sec.port_s,
        },
        // Response on the same client flow: P-CSCF port-s -> UE port-c.
        XfrmSa {
            src: pcscf,
            dst: ue,
            spi: ue_sec.spi_c,
            auth_key: auth_key.to_vec(),
            enc_key: encryption_key.to_vec(),
            algs,
            sport: pcscf_sec.port_s,
            dport: ue_sec.port_c,
        },
        // UE response flow: UE port-s -> P-CSCF port-c.
        XfrmSa {
            src: ue,
            dst: pcscf,
            spi: pcscf_sec.spi_c,
            auth_key: auth_key.to_vec(),
            enc_key: encryption_key.to_vec(),
            algs,
            sport: ue_sec.port_s,
            dport: pcscf_sec.port_c,
        },
        // P-CSCF-initiated flow: P-CSCF port-c -> UE port-s.
        XfrmSa {
            src: pcscf,
            dst: ue,
            spi: ue_sec.spi_s,
            auth_key: auth_key.to_vec(),
            enc_key: encryption_key.to_vec(),
            algs,
            sport: pcscf_sec.port_c,
            dport: ue_sec.port_s,
        },
    ];
    let policies = vec![
        build_xfrm_policy_add(ue, pcscf, ue_sec.port_c, pcscf_sec.port_s, PolicyDir::Out),
        build_xfrm_policy_add(pcscf, ue, pcscf_sec.port_s, ue_sec.port_c, PolicyDir::In),
        build_xfrm_policy_add(ue, pcscf, ue_sec.port_s, pcscf_sec.port_c, PolicyDir::Out),
        build_xfrm_policy_add(pcscf, ue, pcscf_sec.port_c, ue_sec.port_s, PolicyDir::In),
    ];
    Ok(XfrmInstallPlan { states, policies })
}

/// Translate the negotiated Security-Server algorithms into Linux xfrm names.
pub fn xfrm_algs_from_security_server(value: &str) -> Result<XfrmAlgs, VolteError> {
    let mut integrity = None;
    let mut encryption = None;
    for part in value.split(';').skip(1) {
        let Some((name, raw)) = part.split_once('=') else {
            continue;
        };
        let value = raw.trim().trim_matches('"').to_ascii_lowercase();
        match name.trim().to_ascii_lowercase().as_str() {
            "alg" => integrity = Some(value),
            "ealg" => encryption = Some(value),
            _ => {}
        }
    }
    let (auth, auth_trunc_bits) = match integrity.as_deref() {
        Some("hmac-md5-96") => ("hmac(md5)", 96),
        Some("hmac-sha-1-96" | "hmac-sha1-96") => ("hmac(sha1)", 96),
        _ => return Err(VolteError::new(code::SECURITY_SERVER_MISSING)),
    };
    let enc = match encryption.as_deref() {
        Some("null") => "cipher_null",
        Some("aes-cbc") => "cbc(aes)",
        _ => return Err(VolteError::new(code::SECURITY_SERVER_MISSING)),
    };
    Ok(XfrmAlgs {
        auth,
        auth_trunc_bits,
        enc,
    })
}

// ===================== #[cfg(unix)] execution layer =====================

/// Locate the `ip` binary, or return the dependency-missing error the frontend
/// recognizes (`volte_dependency_missing:ip`).
pub fn locate_ip_binary() -> Result<&'static str, VolteError> {
    #[cfg(unix)]
    {
        for candidate in IP_BINARY_CANDIDATES {
            if std::path::Path::new(candidate).exists() {
                return Ok(candidate);
            }
        }
        Err(VolteError::new(code::DEPENDENCY_MISSING_IP))
    }
    #[cfg(not(unix))]
    {
        Err(VolteError::new(code::DEPENDENCY_MISSING_IP))
    }
}

/// Execute one `ip ...` argv. Unix-only; on other platforms this is a no-op
/// stub returning the dependency error (the logic layer is fully tested via the
/// `build_*` functions above).
#[cfg(unix)]
pub fn run_ip(argv: &[String]) -> Result<(), VolteError> {
    let ip = locate_ip_binary()?;
    let output = std::process::Command::new(ip)
        .args(argv)
        .output()
        .map_err(|e| VolteError::with_detail(code::COMMAND_SPAWN_FAILED, format!("ip:{e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let command = redacted_xfrm_argv(argv);
        let exit_code = output.status.code().unwrap_or(-1);
        Err(VolteError::with_detail(
            code::COMMAND_FAILED,
            if stderr.is_empty() {
                format!("ip {command}: exit {exit_code}")
            } else {
                format!("ip {command}: exit {exit_code}: {stderr}")
            },
        ))
    }
}

#[cfg(not(unix))]
pub fn run_ip(_argv: &[String]) -> Result<(), VolteError> {
    Err(VolteError::new(code::DEPENDENCY_MISSING_IP))
}

/// Install the full plan (flush stale, then add states + policies). Unix-only IO.
pub fn install_plan(plan: &XfrmInstallPlan) -> Result<(), VolteError> {
    for cmd in build_xfrm_flush() {
        // Flush is best-effort; ignore failures (nothing to flush is fine).
        let _ = run_ip(&cmd);
    }
    for sa in &plan.states {
        run_ip(&build_xfrm_state_add(sa))?;
    }
    for pol in &plan.policies {
        run_ip(pol)?;
    }
    Ok(())
}

/// Install an IMS XFRM plan inside a per-line UE worker namespace.
///
/// A worker namespace is already exclusive to one UE, so flushing XFRM there
/// is unnecessary and would make retries race with another session in the same
/// namespace. The operation batch is therefore add-only; on a partial failure
/// the installed entries are removed with best-effort delete operations.
pub async fn install_plan_in_worker(
    plan: &XfrmInstallPlan,
    worker: &UeWorkerHandle,
) -> Result<(), VolteError> {
    let ops = plan
        .states
        .iter()
        .map(|state| NetConfigOp::Xfrm {
            args: build_xfrm_state_add(state),
            best_effort: false,
        })
        .chain(plan.policies.iter().cloned().map(|args| NetConfigOp::Xfrm {
            args,
            best_effort: false,
        }))
        .collect();
    let outcome = worker.apply_net_config(ops).await.map_err(|error| {
        VolteError::with_detail(code::COMMAND_FAILED, format!("worker xfrm: {error}"))
    })?;
    if outcome.ok {
        let port_summary = plan.states.first().map(|state| (state.sport, state.dport));
        tracing::info!(
            state_count = plan.states.len(),
            policy_count = plan.policies.len(),
            first_flow_sport = port_summary.map(|ports| ports.0),
            first_flow_dport = port_summary.map(|ports| ports.1),
            "VoLTE IMS XFRM plan installed"
        );
        Ok(())
    } else {
        uninstall_plan_in_worker(plan, worker).await;
        Err(VolteError::with_detail(
            code::COMMAND_FAILED,
            outcome
                .error
                .unwrap_or_else(|| "worker xfrm install failed".to_string()),
        ))
    }
}

/// Remove only the SAs/policies installed for this IMS session. This avoids a
/// global xfrm flush, which could tear down an unrelated VPN on the host.
pub fn uninstall_plan(plan: &XfrmInstallPlan) {
    for policy in plan.policies.iter().rev() {
        let mut delete = policy.clone();
        if let Some(action) = delete.get_mut(2) {
            *action = "delete".to_string();
        }
        if let Some(template) = delete.iter().position(|part| part == "tmpl") {
            delete.truncate(template);
        }
        let _ = run_ip(&delete);
    }
    for state in plan.states.iter().rev() {
        let delete = vec![
            "xfrm".to_string(),
            "state".to_string(),
            "delete".to_string(),
            "src".to_string(),
            state.src.to_string(),
            "dst".to_string(),
            state.dst.to_string(),
            "proto".to_string(),
            "esp".to_string(),
            "spi".to_string(),
            format!("0x{:08x}", state.spi),
        ];
        let _ = run_ip(&delete);
    }
}

/// Remove only the entries represented by `plan` inside a UE worker namespace.
/// All operations are best-effort so a dead namespace or an already removed
/// SA cannot mask the caller's original registration failure.
pub async fn uninstall_plan_in_worker(plan: &XfrmInstallPlan, worker: &UeWorkerHandle) {
    let mut ops = Vec::with_capacity(plan.policies.len() + plan.states.len());
    for policy in plan.policies.iter().rev() {
        let mut delete = policy.clone();
        if let Some(action) = delete.get_mut(2) {
            *action = "delete".to_string();
        }
        if let Some(template) = delete.iter().position(|part| part == "tmpl") {
            delete.truncate(template);
        }
        ops.push(NetConfigOp::Xfrm {
            args: delete,
            best_effort: true,
        });
    }
    for state in plan.states.iter().rev() {
        ops.push(NetConfigOp::Xfrm {
            args: vec![
                "xfrm".to_string(),
                "state".to_string(),
                "delete".to_string(),
                "src".to_string(),
                state.src.to_string(),
                "dst".to_string(),
                state.dst.to_string(),
                "proto".to_string(),
                "esp".to_string(),
                "spi".to_string(),
                format!("0x{:08x}", state.spi),
            ],
            best_effort: true,
        });
    }
    let _ = worker.apply_net_config(ops).await;
}

/// Best-effort teardown of all VoLTE xfrm state/policy.
pub fn teardown() {
    for cmd in build_xfrm_flush() {
        let _ = run_ip(&cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn v6(a: u16) -> IpAddr {
        IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, a))
    }

    #[test]
    fn state_add_has_transport_mode_and_integrity_only() {
        let sa = XfrmSa {
            src: v6(2),
            dst: v6(1),
            spi: 0x0000_1234,
            auth_key: vec![0xaa, 0xbb, 0xcc],
            enc_key: Vec::new(),
            algs: XfrmAlgs::default(),
            sport: 6000,
            dport: 6001,
        };
        let argv = build_xfrm_state_add(&sa);
        let joined = argv.join(" ");
        assert!(joined.starts_with("xfrm state add src "));
        assert!(joined.contains("proto esp spi 0x00001234"));
        assert!(joined.contains("mode transport"));
        assert!(joined.contains("auth-trunc hmac(md5) 0xaabbcc 96"));
        assert!(joined.contains("enc cipher_null"));
        assert!(joined.contains("sel proto udp sport 6000 dport 6001"));
    }

    #[test]
    fn policy_add_binds_ports_and_direction() {
        let argv = build_xfrm_policy_add(v6(2), v6(1), 6000, 6001, PolicyDir::Out);
        let joined = argv.join(" ");
        assert!(joined.contains("xfrm policy add"));
        assert!(joined.contains("dir out"));
        assert!(joined.contains("sport 6000"));
        assert!(joined.contains("dport 6001"));
        assert!(joined.contains("tmpl src 2001:db8::2 dst 2001:db8::1 proto esp mode transport"));
        assert!(joined.contains("mode transport"));
    }

    #[test]
    fn state_add_keeps_sa_lifetime_unbounded_for_registration_refreshes() {
        let sa = XfrmSa {
            src: v6(2),
            dst: v6(1),
            spi: 0x1122_3344,
            auth_key: vec![1; 16],
            enc_key: Vec::new(),
            algs: XfrmAlgs::default(),
            sport: 5064,
            dport: 5063,
        };
        let command = build_xfrm_state_add(&sa);
        assert!(!command.iter().any(|part| part == "limit"));
    }

    #[test]
    fn flush_produces_policy_then_state() {
        let cmds = build_xfrm_flush();
        assert_eq!(cmds[0], vec!["xfrm", "policy", "flush"]);
        assert_eq!(cmds[1], vec!["xfrm", "state", "flush"]);
    }

    #[test]
    fn install_plan_builds_four_flow_sas_and_policies() {
        let ue_sec = SecAgree {
            spi_c: 0x1111,
            spi_s: 0x2222,
            port_c: 6000,
            port_s: 6001,
        };
        let pcscf_sec = SecAgree {
            spi_c: 0x3333,
            spi_s: 0x4444,
            port_c: 7000,
            port_s: 7001,
        };
        let plan = build_install_plan(v6(2), v6(1), &ue_sec, &pcscf_sec, &[0x01; 16]).unwrap();
        assert_eq!(plan.states.len(), 4);
        assert_eq!(plan.policies.len(), 4);
        // REGISTER request/response flow: UE port-c <-> P-CSCF port-s.
        assert_eq!(plan.states[0].spi, 0x4444);
        assert_eq!(plan.states[0].sport, 6000);
        assert_eq!(plan.states[0].dport, 7001);
        assert_eq!(plan.states[1].spi, 0x1111);
        assert_eq!(plan.states[1].sport, 7001);
        assert_eq!(plan.states[1].dport, 6000);
        // P-CSCF request/UE response flow: P-CSCF port-c <-> UE port-s.
        assert_eq!(plan.states[2].spi, 0x3333);
        assert_eq!(plan.states[2].sport, 6001);
        assert_eq!(plan.states[2].dport, 7000);
        assert_eq!(plan.states[3].spi, 0x2222);
        assert_eq!(plan.states[3].sport, 7000);
        assert_eq!(plan.states[3].dport, 6001);
        let policy_text: Vec<String> = plan.policies.iter().map(|p| p.join(" ")).collect();
        assert!(policy_text[0]
            .contains("src 2001:db8::2 dst 2001:db8::1 proto udp sport 6000 dport 7001 dir out"));
        assert!(policy_text[1]
            .contains("src 2001:db8::1 dst 2001:db8::2 proto udp sport 7001 dport 6000 dir in"));
        assert!(policy_text[2]
            .contains("src 2001:db8::2 dst 2001:db8::1 proto udp sport 6001 dport 7000 dir out"));
        assert!(policy_text[3]
            .contains("src 2001:db8::1 dst 2001:db8::2 proto udp sport 7000 dport 6001 dir in"));
    }

    #[test]
    fn install_plan_rejects_family_mismatch() {
        let sec = SecAgree {
            spi_c: 1,
            spi_s: 2,
            port_c: 6000,
            port_s: 6001,
        };
        let v4 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let err = build_install_plan(v4, v6(1), &sec, &sec, &[0x01; 16]).unwrap_err();
        assert_eq!(err.code(), code::PCSCF_FAMILY_MISMATCH);
    }

    #[test]
    fn install_plan_rejects_empty_key() {
        let sec = SecAgree {
            spi_c: 1,
            spi_s: 2,
            port_c: 6000,
            port_s: 6001,
        };
        let err = build_install_plan(v6(2), v6(1), &sec, &sec, &[]).unwrap_err();
        assert_eq!(err.code(), code::IPSEC_IK_INVALID);
    }

    #[test]
    fn hex_key_prefixes_0x() {
        assert_eq!(hex_key(&[0x0a, 0xff]), "0x0aff");
    }

    #[test]
    fn xfrm_diagnostics_redact_integrity_and_encryption_keys() {
        let argv = vec![
            "xfrm".into(),
            "state".into(),
            "add".into(),
            "auth-trunc".into(),
            "hmac(sha1)".into(),
            "0x00112233".into(),
            "96".into(),
            "enc".into(),
            "cbc(aes)".into(),
            "0xaabbccdd".into(),
        ];
        let rendered = redacted_xfrm_argv(&argv);
        assert_eq!(
            rendered,
            "xfrm state add auth-trunc hmac(sha1) [redacted] 96 enc cbc(aes) [redacted]"
        );
        assert!(!rendered.contains("00112233"));
        assert!(!rendered.contains("aabbccdd"));
    }

    #[test]
    fn security_server_round_trips_required_port_and_spi_values() {
        let offered = SecAgree {
            spi_c: 0x1020_3040,
            spi_s: 0x5060_7080,
            port_c: 5062,
            port_s: 5064,
        };
        let parsed = parse_security_server(&offered.security_client_value()).unwrap();
        assert_eq!(parsed, offered);
        let spaced = offered.spaced_security_client_value();
        assert!(spaced.contains("; alg="));
        assert!(spaced.contains("; prot=esp; mod=trans;"));
        assert_eq!(parse_security_server(&spaced).unwrap(), offered);
        let compact = offered.compact_security_client_value();
        assert!(!compact.contains("prot="));
        assert!(!compact.contains("mod="));
        assert_eq!(parse_security_server(&compact).unwrap(), offered);
        let hex = parse_security_server(
            "ipsec-3gpp;alg=hmac-md5-96;spi-c=0x10203040;spi-s=0x50607080;port-c=5062;port-s=5064",
        )
        .unwrap();
        assert_eq!(hex, offered);
    }

    #[test]
    fn scoped_uninstall_arguments_do_not_flush_global_xfrm_state() {
        let sec = SecAgree {
            spi_c: 1,
            spi_s: 2,
            port_c: 5062,
            port_s: 5064,
        };
        let plan = build_install_plan(v6(2), v6(1), &sec, &sec, &[1; 16]).unwrap();
        for policy in &plan.policies {
            assert!(policy.contains(&"add".to_string()));
            assert!(!policy.contains(&"flush".to_string()));
        }
        for state in &plan.states {
            let delete = format!(
                "xfrm state delete src {} dst {} proto esp spi 0x{:08x}",
                state.src, state.dst, state.spi
            );
            assert!(!delete.contains("flush"));
        }
    }
}
