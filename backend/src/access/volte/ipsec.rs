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
//! (`alg=hmac-md5-96; ealg=null`) over a pair of SAs bound to the negotiated
//! client/server ports (`spi-c/spi-s/port-c/port-s`), per the P-CSCF
//! Security-Server offer.

use std::net::IpAddr;

use super::errors::{code, VolteError};

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

/// Build `ip xfrm state add ...` for one SA direction (transport mode,
/// integrity-only). Returns the argv (without the leading `ip`).
pub fn build_xfrm_state_add(sa: &XfrmSa) -> Vec<String> {
    let mut v = vec![
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
        "auth-trunc".into(),
        sa.algs.auth.into(),
        hex_key(&sa.auth_key),
        sa.algs.auth_trunc_bits.to_string(),
        "enc".into(),
        sa.algs.enc.into(),
        "0x".into(),
        "sel".into(),
        "src".into(),
        ip_str(sa.src),
        "dst".into(),
        ip_str(sa.dst),
    ];
    // Port selectors bind the SA to the negotiated sec-agree ports.
    v.push("sport".into());
    v.push(sa.sport.to_string());
    v.push("dport".into());
    v.push(sa.dport.to_string());
    v
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
    spi: u32,
) -> Vec<String> {
    vec![
        "xfrm".into(),
        "policy".into(),
        "add".into(),
        "src".into(),
        ip_str(src),
        "dst".into(),
        ip_str(dst),
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
        "spi".into(),
        format!("0x{:08x}", spi),
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
    sec: &SecAgree,
    auth_key: &[u8],
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
    let algs = XfrmAlgs::default();
    // Outbound: UE:port_c -> P-CSCF:port_s, protected by spi_s (server SA).
    let out_sa = XfrmSa {
        src: ue,
        dst: pcscf,
        spi: sec.spi_s,
        auth_key: auth_key.to_vec(),
        algs,
        sport: sec.port_c,
        dport: sec.port_s,
    };
    // Inbound: P-CSCF:port_s -> UE:port_c, protected by spi_c (client SA).
    let in_sa = XfrmSa {
        src: pcscf,
        dst: ue,
        spi: sec.spi_c,
        auth_key: auth_key.to_vec(),
        algs,
        sport: sec.port_s,
        dport: sec.port_c,
    };
    let policies = vec![
        build_xfrm_policy_add(ue, pcscf, sec.port_c, sec.port_s, PolicyDir::Out, sec.spi_s),
        build_xfrm_policy_add(pcscf, ue, sec.port_s, sec.port_c, PolicyDir::In, sec.spi_c),
    ];
    Ok(XfrmInstallPlan {
        states: vec![out_sa, in_sa],
        policies,
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
    let status = std::process::Command::new(ip)
        .args(argv)
        .status()
        .map_err(|e| VolteError::with_detail(code::COMMAND_SPAWN_FAILED, format!("ip:{e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(VolteError::with_detail(
            code::COMMAND_FAILED,
            format!("ip:{}", status.code().unwrap_or(-1)),
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
        assert!(joined.contains("enc cipher_null 0x"));
        assert!(joined.contains("sport 6000"));
        assert!(joined.contains("dport 6001"));
    }

    #[test]
    fn policy_add_binds_ports_and_direction() {
        let argv = build_xfrm_policy_add(v6(2), v6(1), 6000, 6001, PolicyDir::Out, 0xdead_beef);
        let joined = argv.join(" ");
        assert!(joined.contains("xfrm policy add"));
        assert!(joined.contains("dir out"));
        assert!(joined.contains("sport 6000"));
        assert!(joined.contains("dport 6001"));
        assert!(joined.contains("proto esp spi 0xdeadbeef"));
        assert!(joined.contains("mode transport"));
    }

    #[test]
    fn flush_produces_policy_then_state() {
        let cmds = build_xfrm_flush();
        assert_eq!(cmds[0], vec!["xfrm", "policy", "flush"]);
        assert_eq!(cmds[1], vec!["xfrm", "state", "flush"]);
    }

    #[test]
    fn install_plan_builds_two_sas_and_two_policies() {
        let sec = SecAgree {
            spi_c: 0x1111,
            spi_s: 0x2222,
            port_c: 6000,
            port_s: 6001,
        };
        let plan = build_install_plan(v6(2), v6(1), &sec, &[0x01; 16]).unwrap();
        assert_eq!(plan.states.len(), 2);
        assert_eq!(plan.policies.len(), 2);
        // Outbound SA uses server SPI; inbound uses client SPI.
        assert_eq!(plan.states[0].spi, 0x2222);
        assert_eq!(plan.states[0].sport, 6000);
        assert_eq!(plan.states[0].dport, 6001);
        assert_eq!(plan.states[1].spi, 0x1111);
        assert_eq!(plan.states[1].sport, 6001);
        assert_eq!(plan.states[1].dport, 6000);
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
        let err = build_install_plan(v4, v6(1), &sec, &[0x01; 16]).unwrap_err();
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
        let err = build_install_plan(v6(2), v6(1), &sec, &[]).unwrap_err();
        assert_eq!(err.code(), code::IPSEC_IK_INVALID);
    }

    #[test]
    fn hex_key_prefixes_0x() {
        assert_eq!(hex_key(&[0x0a, 0xff]), "0x0aff");
    }
}
