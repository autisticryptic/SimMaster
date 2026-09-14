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

use super::errors::{code, CellularImsError};
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
        // EF_AD byte four stores the MNC length in its low nibble. The upper
        // nibble carries unrelated administrative flags and must not make a
        // valid 0x82/0x83 value look ambiguous.
        let value = u8::from_str_radix(octets[3], 16).ok()?;
        let length = usize::from(value & 0x0f);
        return matches!(length, 2 | 3).then_some(length);
    }
    None
}

/// Decode the fixed READ BINARY EF_AD reply, never a failed/status-only CRSM
/// response or a partial payload. This is read-only SIM access (TS 27.007).
pub fn parse_crsm_ef_ad_mnc_length(output: &str) -> Option<u8> {
    let mut replies = output.lines().filter_map(|line| {
        let line = line.trim();
        let line = line
            .strip_prefix("response:")
            .unwrap_or(line)
            .trim()
            .trim_matches('\'')
            .trim();
        line.strip_prefix("+CRSM:")
    });
    let reply = replies.next()?;
    if replies.next().is_some() {
        return None;
    }
    let fields = reply.split(',').map(str::trim).collect::<Vec<_>>();
    if fields.len() != 3
        || fields[0].parse::<u8>().ok()? != 0x90
        || fields[1].parse::<u8>().ok()? != 0
    {
        return None;
    }
    let hex = fields[2].strip_prefix('"')?.strip_suffix('"')?;
    if hex.len() != 8 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let length = u8::from_str_radix(&hex[6..8], 16).ok()? & 0x0f;
    matches!(length, 2 | 3).then_some(length)
}

/// Recover missing MNC-length metadata without guessing a carrier rule.
/// Bracket the read with IMSI observations so stale MM metadata or a SIM swap
/// cannot attach another subscription's EF_AD to the pending IMS identity.
pub async fn read_mnc_length_via_at<F, Fut>(imsi: &str, mut query: F) -> Option<u8>
where
    F: FnMut(&'static str) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let before = query("AT+CIMI").await.ok()?;
    if parse_cimi_response(&before).as_deref() != Some(imsi) {
        return None;
    }
    let response = query("AT+CRSM=176,28589,0,0,4").await.ok()?;
    let length = parse_crsm_ef_ad_mnc_length(&response)?;
    let after = query("AT+CIMI").await.ok()?;
    (parse_cimi_response(&after).as_deref() == Some(imsi)).then_some(length)
}

/// Resolve the IMS home PLMN from authoritative subscription-owned data.
///
/// The serving PLMN is deliberately not accepted here: while roaming it names
/// the visited access network, and an IMSI prefix match is not enough to prove
/// whether the home MNC has two or three digits. EF_AD byte four is the
/// authoritative fallback when ModemManager exposes no SIM operator code.
/// Catalog-based inference is performed by `ProfileStore`, where ambiguity can
/// be checked against all known 5/6-digit PLMN rules.
pub fn resolve_home_plmn(
    imsi: &str,
    sim_operator: Option<&str>,
    ef_ad_mnc_length: Option<usize>,
) -> Result<HomePlmn, CellularImsError> {
    let matching_operator_length = sim_operator
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
    } else {
        return Err(CellularImsError::with_detail(
            code::CARRIER_PROFILE_MISSING,
            "home_plmn_mnc_length_ambiguous",
        ));
    };
    let (mcc, mnc) = split_imsi(imsi, mnc_length)?;
    Ok(HomePlmn {
        mcc,
        mnc,
        mnc_length_source: source,
    })
}

/// Select the requested one-based QMI slot before interpreting any AID.
/// qmicli 1.28/1.32 use `Slot [n]:`, not an MM modem index or the reference
/// package's zero-based mock card labels. Missing/duplicate slot sections are
/// ambiguous and must not borrow an application from another subscription.
pub fn parse_uicc_applications_for_slot(output: &str, slot: u8) -> UiccApplications {
    if slot == 0 {
        return UiccApplications::default();
    }
    let mut selected = String::new();
    let mut active = false;
    let mut found = false;
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("slot ["))
        {
            active = trimmed
                .get(6..)
                .and_then(|rest| rest.strip_suffix("]:"))
                .and_then(|number| number.parse::<u8>().ok())
                == Some(slot);
            if active {
                if found {
                    return UiccApplications::default();
                }
                found = true;
            }
            continue;
        }
        if active {
            selected.push_str(line);
            selected.push('\n');
        }
    }
    parse_uicc_applications(&selected)
}

/// Extract USIM/ISIM application identifiers from `qmicli
/// --uim-get-card-status` output. qmicli formatting differs between releases,
/// so values accept colon/space-separated or compact hex. Application type
/// is determined by the registered 3GPP RID, not by application order.
pub fn parse_uicc_applications(output: &str) -> UiccApplications {
    let mut applications = UiccApplications::default();
    let mut lines = output.lines().peekable();
    while let Some(line) = lines.next() {
        // Match the field, never hex letters from labels or unrelated text.
        let Some((label, value)) = line.split_once(':') else {
            continue;
        };
        if !label.trim().eq_ignore_ascii_case("application id") {
            continue;
        }
        let decoded = if value.trim().is_empty() {
            // qmicli-uim prints Application ID on its own line, followed by
            // one more-indented hex row (the maximum 16-byte AID fits in it).
            // Do not scan through blank rows or consume the next field.
            let Some(next) = lines.peek().copied() else {
                continue;
            };
            let indentation = |text: &str| text.len() - text.trim_start().len();
            if indentation(next) <= indentation(line) {
                continue;
            }
            let Some(decoded) = parse_aid_field(next) else {
                continue;
            };
            lines.next();
            decoded
        } else {
            let Some(decoded) = parse_aid_field(value) else {
                continue;
            };
            decoded
        };
        let target = if decoded.starts_with(USIM_AID_PREFIX) {
            &mut applications.usim_aid
        } else if decoded.starts_with(ISIM_AID_PREFIX) {
            &mut applications.isim_aid
        } else {
            continue;
        };
        if target
            .as_ref()
            .is_none_or(|current| decoded.len() > current.len())
        {
            *target = Some(decoded);
        }
    }
    applications
}

fn parse_aid_field(value: &str) -> Option<Vec<u8>> {
    let value = value.trim();
    let value = if let Some(quote) = value.chars().next().filter(|c| matches!(c, '\'' | '"')) {
        value.strip_prefix(quote)?.strip_suffix(quote)?
    } else {
        value
    };
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit() || byte == b':' || byte.is_ascii_whitespace())
    {
        return None;
    }
    let compact = value
        .bytes()
        .filter(|byte| byte.is_ascii_hexdigit())
        .map(char::from)
        .collect::<String>();
    decode_hex_aid(&compact)
}

fn decode_hex_aid(value: &str) -> Option<Vec<u8>> {
    if !(14..=32).contains(&value.len()) || value.len() % 2 != 0 {
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

/// Split the MCC/MNC prefix from an IMSI. The caller must provide a resolved
/// two- or three-digit MNC length; this function does not guess one.
pub fn split_imsi(imsi: &str, mnc_len: usize) -> Result<(String, String), CellularImsError> {
    if !matches!(mnc_len, 2 | 3)
        || imsi.len() < 3 + mnc_len
        || !imsi.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(CellularImsError::new(code::IMSI_MISSING));
    }
    let mcc = imsi[..3].to_string();
    let mnc = imsi[3..3 + mnc_len].to_string();
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
/// `&'static str` reason into a `CellularImsError`.
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
) -> Result<UsimAkaApduResult, CellularImsError> {
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
    .map_err(|reason| CellularImsError::with_detail(code::USIM_AKA_FAILED, reason))
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
    fn split_imsi_rejects_short_input_without_panicking() {
        assert_eq!(
            split_imsi("12345", 3).unwrap_err().code(),
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
            parse_ef_ad_mnc_length("Read result: '00:00:01:82'\n"),
            Some(2)
        );
        assert_eq!(
            parse_ef_ad_mnc_length("Read result:\n\t00:00:01:04\n"),
            None
        );
    }

    #[test]
    fn ef_ad_resolves_home_plmn_without_using_the_visited_operator() {
        let home = resolve_home_plmn("460001234567890", None, Some(2)).unwrap();
        assert_eq!(home.mcc, "460");
        assert_eq!(home.mnc, "00");
        assert_eq!(home.mnc_length_source, "sim_ef_ad");
    }

    #[test]
    fn cmlink_roaming_on_china_mobile_keeps_its_two_digit_home_mnc() {
        let home = resolve_home_plmn("234331234567890", None, Some(2)).unwrap();
        assert_eq!(home.mcc, "234");
        assert_eq!(home.mnc, "33");
        assert_eq!(home.mnc_length_source, "sim_ef_ad");
    }

    #[test]
    fn matching_sim_operator_can_supply_mnc_length() {
        let home = resolve_home_plmn("310260123456789", Some("310260"), Some(2)).unwrap();
        assert_eq!(home.mcc, "310");
        assert_eq!(home.mnc, "260");
        assert_eq!(home.mnc_length_source, "modemmanager_home_operator");
    }

    #[test]
    fn imsi_alone_does_not_guess_the_mnc_length() {
        let error = resolve_home_plmn("310260123456789", None, None).unwrap_err();
        assert_eq!(error.code(), code::CARRIER_PROFILE_MISSING);
        assert!(error.to_string().contains("home_plmn_mnc_length_ambiguous"));
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

    #[test]
    fn application_id_parser_ignores_hex_letters_in_the_label() {
        let applications = parse_uicc_applications("Application ID: 'A0:00:00:00:87:10:02:FF'\n");
        assert_eq!(
            applications.usim_aid.as_deref(),
            Some(&[0xa0, 0, 0, 0, 0x87, 0x10, 2, 0xff][..])
        );
    }

    #[test]
    fn aid_parser_rejects_corrupt_or_oversized_values_instead_of_truncating() {
        for value in [
            "a0000000871002f",
            "00a0000000871002",
            "a0000000871002z",
            "a0000000871002ffffffffffffffffffff00",
        ] {
            assert_eq!(
                parse_uicc_applications(&format!("Application ID: '{value}'")),
                UiccApplications::default()
            );
        }
        assert_eq!(
            parse_uicc_applications("note Application ID: 'a0000000871002'"),
            UiccApplications::default()
        );
        assert!(
            parse_uicc_applications(" application id : \"A0 00 00 00 87 10 02\" ")
                .usim_aid
                .is_some()
        );
    }

    #[test]
    fn application_ids_accept_real_qmicli_indented_value_rows() {
        // qmicli-uim.c emits a separate, more-indented raw-data row.
        let applications = parse_uicc_applications(concat!(
            "\tApplication [1]:\n",
            "\t\tApplication type:  'usim (2)'\n",
            "\t\tApplication state: 'ready'\n",
            "\t\tApplication ID:\n",
            "\t\t\tA0:00:00:00:87:10:02:01:02:03:04:05:06:07:08:09\n",
            "\tApplication [2]:\n",
            "\t\tApplication type:  'isim (5)'\n",
            "\t\tApplication ID:\n",
            "\t\t\tA0:00:00:00:87:10:04:FF:86:FF\n",
        ));
        assert_eq!(
            applications.usim_aid.as_deref(),
            Some(&[0xa0, 0, 0, 0, 0x87, 0x10, 2, 1, 2, 3, 4, 5, 6, 7, 8, 9,][..])
        );
        assert_eq!(
            applications.isim_aid.as_deref(),
            Some(&[0xa0, 0, 0, 0, 0x87, 0x10, 4, 0xff, 0x86, 0xff,][..])
        );
    }

    #[test]
    fn application_id_continuations_cannot_escape_their_field() {
        for output in [
            "Application ID:\nA0:00:00:00:87:10:02\n",
            "Application ID:\n\n  A0:00:00:00:87:10:02\n",
            "Application ID:\n  Application state: 'ready'\n  A0:00:00:00:87:10:02\n",
            "Application ID:\n  a0000000871002f\n",
            "Application ID:\n  a0000000871002ffffffffffffffffffff00\n",
            "Application ID:\n  'a0000000871002\n",
        ] {
            assert_eq!(
                parse_uicc_applications(output),
                UiccApplications::default(),
                "{output}"
            );
        }
        let applications =
            parse_uicc_applications("Application ID:\n  Application ID: 'a0000000871004'\n");
        assert!(applications.usim_aid.is_none());
        assert_eq!(applications.isim_aid.as_deref(), Some(ISIM_AID_PREFIX));
    }

    #[test]
    fn uicc_applications_are_scoped_to_the_requested_one_based_slot() {
        let output = concat!(
            "Provisioning applications:\n  Primary GW: slot '1', application '1'\n",
            "Slot [1]:\n  Card state: 'present'\n  Application ID:\n    A0:00:00:00:87:10:02:01\n",
            "Slot [2]:\n  Card state: 'present'\n  Application ID:\n    A0:00:00:00:87:10:02:02:02:02\n",
            "  Application ID: 'a0000000871004ff'\n",
        );
        let first = parse_uicc_applications_for_slot(output, 1);
        assert_eq!(
            first.usim_aid.as_deref(),
            Some(&[0xa0, 0, 0, 0, 0x87, 0x10, 2, 1][..])
        );
        assert!(first.isim_aid.is_none());
        let second = parse_uicc_applications_for_slot(output, 2);
        assert_eq!(second.usim_aid.as_ref().unwrap().last(), Some(&2));
        assert!(second.isim_aid.is_some());
        for slot in [0, 3] {
            assert_eq!(
                parse_uicc_applications_for_slot(output, slot),
                UiccApplications::default()
            );
        }
    }

    #[test]
    fn uicc_slot_parser_rejects_unscoped_duplicate_and_malformed_sections() {
        for output in [
            "Application ID: 'a0000000871002'\n",
            "Card [0]:\n  Application ID: 'a0000000871002'\n",
            "Slot [1]:\n  Application ID: 'a0000000871002'\nSlot [1]:\n  Application ID: 'a0000000871002ff'\n",
            "Slot [1]:\nSlot [invalid]:\n  Application ID: 'a0000000871002'\n",
        ] {
            assert_eq!(parse_uicc_applications_for_slot(output, 1), UiccApplications::default(), "{output}");
        }
    }

    #[test]
    fn crsm_ef_ad_requires_success_and_exact_read_binary_payload() {
        assert_eq!(
            parse_crsm_ef_ad_mnc_length("+CRSM: 144,0,\"00000182\"\r\nOK"),
            Some(2)
        );
        assert_eq!(
            parse_crsm_ef_ad_mnc_length("response: '+CRSM: 144,0,\"00000183\"'"),
            Some(3)
        );
        for output in [
            "+CRSM: 106,130,\"00000183\"",
            "+CRSM: 144,1,\"00000183\"",
            "+CRSM: 144,0",
            "+CRSM: 144,0,00000183",
            "+CRSM: 144,0,\"000083\"",
            "+CRSM: 144,0,\"0000018300\"",
            "+CRSM: 144,0,\"0000018z\"",
            "+CRSM: 144,0,\"000001ff\"",
            "unrelated +CRSM: 144,0,\"00000183\"",
            "+CRSM: 144,0,\"00000182\"\n+CRSM: 144,0,\"00000183\"",
        ] {
            assert_eq!(parse_crsm_ef_ad_mnc_length(output), None, "{output}");
        }
    }

    #[tokio::test]
    async fn at_mnc_fallback_is_read_only_and_requires_stable_imsi() {
        let imsi = "310260123456789";
        for (before, response, after, expected, queries) in [
            (imsi, Ok("+CRSM: 144,0,\"00000183\""), imsi, Some(3), 3),
            (
                "310260987654321",
                Ok("+CRSM: 144,0,\"00000183\""),
                imsi,
                None,
                1,
            ),
            (
                imsi,
                Ok("+CRSM: 144,0,\"00000183\""),
                "310260987654321",
                None,
                3,
            ),
            (imsi, Ok("+CRSM: 106,130"), imsi, None, 2),
            (imsi, Err("unavailable"), imsi, None, 2),
        ] {
            let mut replies = std::collections::VecDeque::from([
                Ok(before.to_string()),
                response.map(str::to_string).map_err(str::to_string),
                Ok(after.to_string()),
            ]);
            let mut commands = Vec::new();
            let result = read_mnc_length_via_at(imsi, |command| {
                commands.push(command);
                std::future::ready(replies.pop_front().unwrap())
            })
            .await;
            assert_eq!(result, expected);
            assert_eq!(commands.len(), queries);
            assert!(commands
                .iter()
                .all(|command| matches!(*command, "AT+CIMI" | "AT+CRSM=176,28589,0,0,4")));
        }
    }

    #[test]
    fn ef_ad_low_nibble_and_explicit_mnc_length_are_validated() {
        assert_eq!(
            parse_ef_ad_mnc_length("Read result: '00:00:01:83'"),
            Some(3)
        );
        assert_eq!(parse_ef_ad_mnc_length("Read result: '00:00:01:ff'"), None);
        for length in [0, 1, 4, usize::MAX] {
            assert!(split_imsi("310260123456789", length).is_err());
        }
        assert!(resolve_home_plmn("310260123456789", Some("45507"), None).is_err());
    }
}
