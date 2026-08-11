//! VoLTE IMS identity: IMPI/IMPU derivation and the USIM AKA run wrapper.
//!
//! Clean-room from 3GPP TS 23.003 (identity formats) and TS 31.102 (USIM).
//! When no ISIM is provisioned, the private/public identities are derived from
//! the IMSI per TS 23.003 §13:
//!   home domain = ims.mnc<MNC>.mcc<MCC>.3gppnetwork.org  (MNC zero-padded to 3)
//!   IMPI  = <IMSI>@<home domain>
//!   IMPU  = sip:<IMSI>@<home domain>
//!
//! The AKA run reuses `vowifi::qmi_uim` (transport-agnostic SIM hardware access).

use super::errors::{code, VolteError};
use super::sip::ImsIdentity;
use crate::connectivity::modems::ims::vowifi::qmi_uim::{UsimAkaApduResult, USIM_AID_PREFIX};

pub const ISIM_AID_PREFIX: &[u8] = &[0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiccApplications {
    pub usim_aid: Option<Vec<u8>>,
    pub isim_aid: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomePlmn {
    pub mcc: String,
    pub mnc: String,
    pub mnc_length_source: &'static str,
}

/// Extract the IMSI from ModemManager's `--command=AT+CIMI` response without
/// depending on its translated `response:` label or quote style.
pub fn parse_cimi_response(output: &str) -> Option<String> {
    output
        .split(|character: char| !character.is_ascii_digit())
        .find(|candidate| (14..=16).contains(&candidate.len()))
        .map(str::to_string)
}

/// EF_AD byte four is the authoritative MNC length for a USIM when it is 2 or
/// 3 (3GPP TS 31.102). `qmicli` renders the transparent file as colon-separated
/// octets below a `Read result:` marker.
pub fn parse_ef_ad_mnc_length(output: &str) -> Option<usize> {
    let (_, payload) = output.split_once("Read result:")?;
    for line in payload.lines().take(3) {
        let candidate = line.trim().trim_matches('\'');
        if candidate.is_empty() {
            continue;
        }
        let octets = candidate
            .split(':')
            .map(|octet| octet.trim().trim_matches('\''))
            .collect::<Vec<_>>();
        if octets.len() < 4
            || !octets
                .iter()
                .all(|octet| octet.len() == 2 && octet.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            continue;
        }
        let length = usize::from_str_radix(octets[3], 16).ok()?;
        return matches!(length, 2 | 3).then_some(length);
    }
    None
}

/// Resolve the IMS home PLMN. The currently registered operator is usable only
/// when it is a prefix of the IMSI; otherwise it is a visited network. EF_AD is
/// the next source, followed by beta8's three-digit default and China two-digit
/// compatibility fallback.
pub fn resolve_home_plmn(
    imsi: &str,
    registered_operator: Option<&str>,
    ef_ad_mnc_length: Option<usize>,
) -> Result<HomePlmn, VolteError> {
    let matching_operator_length = registered_operator
        .filter(|operator| {
            matches!(operator.len(), 5 | 6)
                && operator.bytes().all(|byte| byte.is_ascii_digit())
                && imsi.starts_with(*operator)
        })
        .map(|operator| operator.len() - 3);

    let (mnc_length, source) = if let Some(length) = matching_operator_length {
        (length, "modemmanager_home_operator")
    } else if let Some(length @ (2 | 3)) = ef_ad_mnc_length {
        (length, "sim_ef_ad")
    } else if imsi.starts_with("460") {
        (2, "china_compatibility_fallback")
    } else {
        (3, "three_digit_fallback")
    };
    let (mcc, mnc) = split_imsi(imsi, mnc_length)?;
    Ok(HomePlmn {
        mcc,
        mnc,
        mnc_length_source: source,
    })
}

/// Extract USIM/ISIM application identifiers from `qmicli
/// --uim-get-card-status` output. qmicli formatting differs between releases,
/// so recognition is based on the registered 3GPP RID/application prefixes
/// instead of translated labels or line positions.
pub fn parse_uicc_applications(output: &str) -> UiccApplications {
    let mut applications = UiccApplications::default();
    for line in output.lines() {
        let compact = line
            .bytes()
            .filter(|byte| byte.is_ascii_hexdigit())
            .map(|byte| (byte as char).to_ascii_lowercase())
            .collect::<String>();
        for (prefix, target) in [
            ("a0000000871002", &mut applications.usim_aid),
            ("a0000000871004", &mut applications.isim_aid),
        ] {
            let Some(start) = compact.find(prefix) else {
                continue;
            };
            let candidate = &compact[start..];
            let even_len = candidate.len() - candidate.len() % 2;
            if let Some(decoded) = decode_hex_aid(&candidate[..even_len]) {
                if target
                    .as_ref()
                    .is_none_or(|current| decoded.len() > current.len())
                {
                    *target = Some(decoded);
                }
            }
        }
    }
    applications
}

fn decode_hex_aid(value: &str) -> Option<Vec<u8>> {
    if value.len() < 14 || value.len() % 2 != 0 {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

pub fn aid_hex(aid: &[u8]) -> String {
    aid.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Build the IMS home domain from MCC/MNC (TS 23.003). MNC is zero-padded to 3
/// digits per the 3GPP domain rule (even when the operator uses a 2-digit MNC).
pub fn home_domain(mcc: &str, mnc: &str) -> String {
    format!("ims.mnc{}.mcc{}.3gppnetwork.org", pad_mnc(mnc), mcc)
}

fn pad_mnc(mnc: &str) -> String {
    if mnc.len() >= 3 {
        mnc.to_string()
    } else {
        format!("{:0>3}", mnc)
    }
}

/// Split a 15/16-digit IMSI into (MCC=3, MNC=2 or 3). We can't always know the
/// MNC length from the IMSI alone; callers that know the true MNC length should
/// pass it. This helper assumes a 2-digit MNC by default (the common case in
/// CN/most networks), which the caller can override.
pub fn split_imsi(imsi: &str, mnc_len: usize) -> Result<(String, String), VolteError> {
    if imsi.len() < 5 || !imsi.bytes().all(|b| b.is_ascii_digit()) {
        return Err(VolteError::new(code::IMSI_MISSING));
    }
    let mcc = imsi[..3].to_string();
    let mnc = imsi[3..3 + mnc_len.clamp(2, 3)].to_string();
    Ok((mcc, mnc))
}

/// Derive the full IMS identity set from an IMSI and its MCC/MNC.
pub fn derive_identity(imsi: &str, mcc: &str, mnc: &str) -> ImsIdentity {
    let domain = home_domain(mcc, mnc);
    derive_identity_with_domain(imsi, &domain)
}

/// Build IMS identities using the catalog's authoritative home domain.
pub fn derive_identity_with_domain(imsi: &str, domain: &str) -> ImsIdentity {
    ImsIdentity {
        private_user: format!("{imsi}@{domain}"),
        public_uri: format!("sip:{imsi}@{domain}"),
        contact_user: imsi.to_string(),
        home_domain: domain.to_string(),
        contact_user_phone: false,
    }
}

/// Whether a card-status application id looks like a USIM AID (starts with the
/// registered USIM application prefix). Used to validate discovered AIDs before
/// running AKA, falling back to the built-in prefix otherwise.
pub fn is_usim_aid(aid: &[u8]) -> bool {
    aid.starts_with(USIM_AID_PREFIX)
}

/// Resolve the USIM AID to use: the discovered one if it is a USIM application,
/// else the built-in prefix (matching the reference "using built-in fallback").
pub fn resolve_usim_aid(discovered: Option<&[u8]>) -> Vec<u8> {
    match discovered {
        Some(aid) if is_usim_aid(aid) => aid.to_vec(),
        _ => USIM_AID_PREFIX.to_vec(),
    }
}

/// Run USIM AKA on the SIM hardware via qmi-proxy (blocking; call from a
/// blocking context). Thin wrapper over the reused vowifi routine that maps its
/// `&'static str` reason into a `VolteError`.
#[allow(clippy::too_many_arguments)]
pub fn run_usim_aka(
    proxy_socket: &str,
    device_path: &str,
    slot: u8,
    aid: &[u8],
    rand: &[u8],
    autn: &[u8],
    attempts: usize,
    timeout: std::time::Duration,
    retry_delay: std::time::Duration,
) -> Result<UsimAkaApduResult, VolteError> {
    crate::connectivity::modems::ims::vowifi::qmi_uim::execute_usim_authenticate_via_proxy_reason_with_retry(
        proxy_socket,
        device_path,
        slot,
        aid,
        rand,
        autn,
        attempts,
        timeout,
        retry_delay,
    )
    .map_err(|reason| VolteError::with_detail(code::USIM_AKA_FAILED, reason))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_domain_pads_two_digit_mnc_to_three() {
        assert_eq!(
            home_domain("460", "01"),
            "ims.mnc001.mcc460.3gppnetwork.org"
        );
        assert_eq!(
            home_domain("310", "260"),
            "ims.mnc260.mcc310.3gppnetwork.org"
        );
    }

    #[test]
    fn derive_identity_builds_impi_impu() {
        let id = derive_identity("460001234567890", "460", "00");
        assert_eq!(
            id.private_user,
            "460001234567890@ims.mnc000.mcc460.3gppnetwork.org"
        );
        assert_eq!(
            id.public_uri,
            "sip:460001234567890@ims.mnc000.mcc460.3gppnetwork.org"
        );
        assert_eq!(id.contact_user, "460001234567890");
    }

    #[test]
    fn split_imsi_extracts_mcc_mnc() {
        let (mcc, mnc) = split_imsi("460001234567890", 2).unwrap();
        assert_eq!(mcc, "460");
        assert_eq!(mnc, "00");
        let (mcc3, mnc3) = split_imsi("310260123456789", 3).unwrap();
        assert_eq!(mcc3, "310");
        assert_eq!(mnc3, "260");
    }

    #[test]
    fn split_imsi_rejects_non_digits() {
        assert_eq!(
            split_imsi("46zzz", 2).unwrap_err().code(),
            code::IMSI_MISSING
        );
    }

    #[test]
    fn parses_modemmanager_cimi_response() {
        assert_eq!(
            parse_cimi_response("response: '460001234567890'\n").as_deref(),
            Some("460001234567890")
        );
        assert_eq!(parse_cimi_response("error: command rejected"), None);
    }

    #[test]
    fn parses_qmicli_ef_ad_mnc_length() {
        assert_eq!(
            parse_ef_ad_mnc_length("Read result:\n\t00:00:01:02\n"),
            Some(2)
        );
        assert_eq!(
            parse_ef_ad_mnc_length("Read result: '00:00:01:03'\n"),
            Some(3)
        );
        assert_eq!(
            parse_ef_ad_mnc_length("Read result:\n\t00:00:01:04\n"),
            None
        );
    }

    #[test]
    fn visited_operator_does_not_replace_imsi_home_plmn() {
        let home = resolve_home_plmn("460001234567890", Some("46011"), Some(2)).unwrap();
        assert_eq!(home.mcc, "460");
        assert_eq!(home.mnc, "00");
        assert_eq!(home.mnc_length_source, "sim_ef_ad");
    }

    #[test]
    fn matching_registered_operator_can_supply_mnc_length() {
        let home = resolve_home_plmn("310260123456789", Some("310260"), Some(2)).unwrap();
        assert_eq!(home.mcc, "310");
        assert_eq!(home.mnc, "260");
        assert_eq!(home.mnc_length_source, "modemmanager_home_operator");
    }

    #[test]
    fn home_plmn_fallback_matches_beta8_policy() {
        let china = resolve_home_plmn("460001234567890", None, None).unwrap();
        assert_eq!(china.mnc, "00");
        assert_eq!(china.mnc_length_source, "china_compatibility_fallback");

        let global = resolve_home_plmn("310260123456789", None, None).unwrap();
        assert_eq!(global.mnc, "260");
        assert_eq!(global.mnc_length_source, "three_digit_fallback");
    }

    #[test]
    fn resolve_usim_aid_prefers_valid_discovered() {
        let mut good = USIM_AID_PREFIX.to_vec();
        good.extend_from_slice(&[0xff, 0xff]);
        assert_eq!(resolve_usim_aid(Some(&good)), good);
        // Non-USIM discovered -> fall back to built-in prefix.
        assert_eq!(
            resolve_usim_aid(Some(&[0x01, 0x02])),
            USIM_AID_PREFIX.to_vec()
        );
        assert_eq!(resolve_usim_aid(None), USIM_AID_PREFIX.to_vec());
    }

    #[test]
    fn parses_qmicli_usim_and_isim_application_ids() {
        let output = r#"
Application type: 'usim (2)'
Application ID: 'A0:00:00:00:87:10:02:FF:86:FF'
Application type: 'isim (5)'
Application ID: 'A0:00:00:00:87:10:04:FF:86:FF'
"#;
        let applications = parse_uicc_applications(output);
        assert!(applications.usim_aid.as_deref().is_some_and(is_usim_aid));
        assert!(applications
            .isim_aid
            .as_deref()
            .is_some_and(|aid| aid.starts_with(ISIM_AID_PREFIX)));
    }
}
