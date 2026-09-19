//! Read-only AT P-CSCF association for an owned QCA410/MM bearer.
//!
//! A 3GPP IPv6 PDN allocates a /64, while the modem's AT context and the
//! host-side WDS client may use different interface identifiers in that PDN
//! (RFC 6459, section 5.2). A shared prefix ALONE is not session ownership.
//! The narrowly scoped exception here additionally requires a verified MM
//! profile pin, exactly one active PDP context on the modem, that exact CID,
//! an IPv6-capable definition, and unchanged MM and AT snapshots. Other
//! providers and unpinned/multiple contexts retain exact-address matching.
//!
//! All IO is supplied by the retained session's unique-owner MM adapter. This
//! module never opens a QMI client, changes a profile, or activates a context.

use std::{collections::BTreeMap, future::Future, net::IpAddr, time::Duration};

use crate::hardware::{
    cellular::cgcontrdp::{
        parse_cgcontrdp_addr_and_mask, parse_cgcontrdp_addresses, CgcontrdpSettings,
    },
    devices::transport::{
        ImsBearerError, ImsBearerErrorKind, ImsBearerFailureHint, ImsPcscfDiscovery,
    },
};

pub(super) const BUDGET: Duration = Duration::from_secs(12);
pub(super) const READ_DELAY: Duration = Duration::from_secs(1);
const READ_ROUNDS: usize = 6;
const MAX_AT_BYTES: usize = 16 * 1024;

pub(super) fn unavailable(detail: &str) -> ImsBearerError {
    ImsBearerError {
        kind: ImsBearerErrorKind::PcscfUnavailable,
        hint: ImsBearerFailureHint::None,
        detail: format!("qca410_primary_mm_pcscf:{detail}"),
    }
}

pub(super) fn session_changed(detail: &str) -> ImsBearerError {
    ImsBearerError {
        kind: ImsBearerErrorKind::SessionLost,
        hint: ImsBearerFailureHint::None,
        detail: format!("qca410_primary_mm_pcscf_binding:{detail}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Definition {
    apn: String,
    pdp_type: String,
    /// Preserve the remaining definition fields for the before/after check;
    /// CID/APN/type alone must not hide a changed profile definition.
    remaining: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ContextState {
    active: Vec<u8>,
    definitions: BTreeMap<u8, Definition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ContextRow {
    bearer_id: u8,
    local: IpAddr,
    prefix: Option<u8>,
    gateway: Option<IpAddr>,
    candidates: Vec<IpAddr>,
}

fn usable(address: IpAddr) -> bool {
    !address.is_unspecified()
        && !address.is_loopback()
        && !address.is_multicast()
        && match address {
            IpAddr::V4(value) => !value.is_broadcast() && !value.is_link_local(),
            IpAddr::V6(value) => !value.is_unicast_link_local() && value.to_ipv4_mapped().is_none(),
        }
}

/// Accept only this command's data and optional echo/OK, not an error tail or
/// a partial mixed response. Values are never copied into diagnostic errors.
fn data_lines<'a>(
    output: &'a str,
    command: &str,
    prefix: &str,
) -> Result<Vec<&'a str>, ImsBearerError> {
    if output.len() > MAX_AT_BYTES {
        return Err(unavailable("at_output_limit"));
    }
    let mut rows = Vec::new();
    let mut terminal = false;
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if terminal {
            return Err(unavailable("at_trailing_data"));
        }
        if line == "OK" {
            terminal = true;
        } else if line == command && rows.is_empty() {
            continue;
        } else if let Some(values) = line.strip_prefix(prefix) {
            rows.push(values.trim());
        } else {
            return Err(unavailable("at_response_invalid"));
        }
    }
    Ok(rows)
}

fn field(value: &str) -> Result<&str, ImsBearerError> {
    let value = value.trim();
    let value = if value.starts_with('"') {
        value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .ok_or_else(|| unavailable("at_field_invalid"))?
    } else {
        value
    };
    if value.contains(['"', '\'', '\r', '\n']) {
        return Err(unavailable("at_field_invalid"));
    }
    Ok(value)
}

fn cid(value: &str) -> Result<u8, ImsBearerError> {
    field(value)?
        .parse::<u8>()
        .ok()
        .filter(|value| (1..=16).contains(value))
        .ok_or_else(|| unavailable("at_cid_invalid"))
}

fn context_state(activity: &str, definitions: &str) -> Result<ContextState, ImsBearerError> {
    let mut states = BTreeMap::new();
    for values in data_lines(activity, "AT+CGACT?", "+CGACT:")? {
        let fields: Vec<_> = values.split(',').collect();
        if fields.len() != 2 {
            return Err(unavailable("activity_ambiguous"));
        }
        let active = match field(fields[1])? {
            "0" => false,
            "1" => true,
            _ => return Err(unavailable("activity_ambiguous")),
        };
        if states.insert(cid(fields[0])?, active).is_some() {
            return Err(unavailable("activity_ambiguous"));
        }
    }
    let mut contexts = BTreeMap::new();
    for values in data_lines(definitions, "AT+CGDCONT?", "+CGDCONT:")? {
        let fields: Vec<_> = values.split(',').collect();
        if fields.len() < 3 {
            return Err(unavailable("definition_ambiguous"));
        }
        let definition = Definition {
            pdp_type: field(fields[1])?.to_ascii_uppercase(),
            apn: field(fields[2])?.to_ascii_lowercase(),
            remaining: fields[3..]
                .iter()
                .map(|value| field(value).map(str::to_string))
                .collect::<Result<_, _>>()?,
        };
        if contexts.insert(cid(fields[0])?, definition).is_some() {
            return Err(unavailable("definition_ambiguous"));
        }
    }
    let active: Vec<_> = states
        .into_iter()
        .filter_map(|(cid, active)| active.then_some(cid))
        .collect();
    if active.iter().any(|cid| !contexts.contains_key(cid)) {
        return Err(unavailable("active_definition_missing"));
    }
    Ok(ContextState {
        active,
        definitions: contexts,
    })
}

fn context_rows(
    output: &str,
    expected_cid: u8,
    apn: &str,
) -> Result<Vec<ContextRow>, ImsBearerError> {
    let mut rows: Vec<ContextRow> = Vec::new();
    for values in data_lines(
        output,
        &format!("AT+CGCONTRDP={expected_cid}"),
        "+CGCONTRDP:",
    )? {
        let fields: Vec<_> = values.split(',').map(field).collect::<Result<_, _>>()?;
        if fields.len() < 7
            || cid(fields[0])? != expected_cid
            || !fields[2].eq_ignore_ascii_case(apn)
        {
            return Err(unavailable("context_row_mismatch"));
        }
        let bearer_id = fields[1]
            .parse::<u8>()
            .ok()
            .filter(|value| *value != 0)
            .ok_or_else(|| unavailable("context_bearer_id_invalid"))?;
        let (local, prefix) = parse_cgcontrdp_addr_and_mask(fields[3])
            .filter(|(address, _)| usable(*address))
            .ok_or_else(|| unavailable("context_local_invalid"))?;
        if rows
            .iter()
            .any(|row| row.local.is_ipv4() == local.is_ipv4() || row.bearer_id != bearer_id)
        {
            // Do not merge a local address from one row with another row's
            // P-CSCF, even when the CID/APN text is identical.
            return Err(unavailable("context_rows_ambiguous"));
        }
        let gateway = if fields[4].is_empty() {
            None
        } else {
            let gateway = parse_cgcontrdp_addresses(fields[4]);
            if fields[4].split_whitespace().count() != 1
                || gateway.len() != 1
                || gateway[0].is_ipv4() != local.is_ipv4()
            {
                // The compact dual-local-column format requires independent
                // CGPADDR proof. It is not a standard row and is not guessed.
                return Err(unavailable("context_layout_unsupported"));
            }
            Some(gateway[0])
        };
        let mut candidates = Vec::new();
        for token in fields
            .iter()
            .skip(7)
            .take(2)
            .flat_map(|value| value.split_whitespace())
        {
            let parsed = parse_cgcontrdp_addresses(token);
            if parsed.len() != 1 {
                return Err(unavailable("context_pcscf_invalid"));
            }
            let address = parsed[0];
            if address.is_unspecified() {
                continue;
            }
            if !usable(address) || address.is_ipv4() != local.is_ipv4() {
                return Err(unavailable("context_pcscf_family_invalid"));
            }
            if !candidates.contains(&address) {
                candidates.push(address);
            }
        }
        rows.push(ContextRow {
            bearer_id,
            local,
            prefix,
            gateway,
            candidates,
        });
    }
    Ok(rows)
}

fn association(
    row: &ContextRow,
    cid: u8,
    state: &ContextState,
    profile_id: Option<u32>,
    expected: &CgcontrdpSettings,
) -> Option<&'static str> {
    let local = if row.local.is_ipv4() {
        expected.ipv4_address
    } else {
        expected.ipv6_address
    }?;
    if row.local == local {
        return Some("mm_owned_at_exact_address");
    }
    if profile_id != Some(u32::from(cid))
        || state.active.as_slice() != [cid]
        || expected.ipv6_prefix != Some(64)
        || row.prefix.is_some_and(|prefix| prefix != 64)
        || !matches!(
            state.definitions.get(&cid)?.pdp_type.as_str(),
            "IPV6" | "IPV4V6"
        )
    {
        return None;
    }
    let (IpAddr::V6(at), IpAddr::V6(mm)) = (row.local, local) else {
        return None;
    };
    // No IPv4-subnet approximation, link-local bootstrap, mapped address, zero
    // IID or guessed prefix. The MM snapshot is the authoritative /64 grant.
    let at = at.octets();
    let mm = mm.octets();
    (at[..8] == mm[..8] && at[8..] != [0; 8] && mm[8..] != [0; 8])
        .then_some("mm_owned_at_sole_pinned_ipv6_prefix")
}

/// The adapter must bind both callbacks to the same unique MM owner and modem;
/// `read` additionally validates the actual bearer profile, exclusive netdev,
/// and current endpoint. Caller holds the lease activity guard and deadline.
pub(super) async fn discover_with<Read, ReadFuture, At, AtFuture>(
    expected: &CgcontrdpSettings,
    profile_id: Option<u32>,
    apn: &str,
    delay: Duration,
    mut read: Read,
    mut at: At,
) -> Result<ImsPcscfDiscovery, ImsBearerError>
where
    Read: FnMut() -> ReadFuture,
    ReadFuture: Future<Output = Result<CgcontrdpSettings, ImsBearerError>>,
    At: FnMut(String) -> AtFuture,
    AtFuture: Future<Output = Result<String, ImsBearerError>>,
{
    if expected.ipv4_address.is_none() && expected.ipv6_address.is_none()
        || expected
            .ipv4_address
            .is_some_and(|address| !address.is_ipv4() || !usable(address))
        || expected
            .ipv6_address
            .is_some_and(|address| !address.is_ipv6() || !usable(address))
    {
        return Err(session_changed("local_address_invalid"));
    }
    if read().await? != *expected {
        return Err(session_changed("ip_config_changed"));
    }
    let mut pinned: Option<ContextState> = None;
    let mut saw_unassociated = false;
    for round in 0..READ_ROUNDS {
        if round > 0 {
            tokio::time::sleep(delay).await;
        }
        let state = context_state(
            &at("AT+CGACT?".into()).await?,
            &at("AT+CGDCONT?".into()).await?,
        )?;
        if pinned.as_ref().is_some_and(|previous| previous != &state) {
            return Err(unavailable("context_changed"));
        }
        if state.active.is_empty() {
            continue;
        }
        pinned.get_or_insert_with(|| state.clone());
        for cid in state.active.iter().copied().filter(|cid| {
            state.definitions[cid].apn.eq_ignore_ascii_case(apn)
                && profile_id.is_none_or(|profile| profile == u32::from(*cid))
        }) {
            let command = format!("AT+CGCONTRDP={cid}");
            let rows = context_rows(&at(command.clone()).await?, cid, apn)?;
            let mut candidates = Vec::new();
            let mut source = "mm_owned_at_exact_address";
            for row in &rows {
                let Some(associated) = association(row, cid, &state, profile_id, expected) else {
                    saw_unassociated |= !row.candidates.is_empty();
                    continue;
                };
                if associated == "mm_owned_at_sole_pinned_ipv6_prefix" && !row.candidates.is_empty()
                {
                    source = associated;
                }
                for address in &row.candidates {
                    if !candidates.contains(address) {
                        candidates.push(*address);
                    }
                }
            }
            if candidates.is_empty() {
                continue;
            }
            // Revalidate both control-plane views, not just APN/CGACT. A
            // deactivated/rebound context cannot lend its old PCO after await.
            let after = context_state(
                &at("AT+CGACT?".into()).await?,
                &at("AT+CGDCONT?".into()).await?,
            )?;
            if after != state || context_rows(&at(command).await?, cid, apn)? != rows {
                return Err(unavailable("context_changed"));
            }
            if read().await? != *expected {
                return Err(session_changed("ip_config_changed"));
            }
            return Ok(ImsPcscfDiscovery {
                candidates,
                context_id: Some(cid),
                source,
            });
        }
    }
    // A failed observation must not hide a disappeared/replaced MM bearer.
    if read().await? != *expected {
        return Err(session_changed("ip_config_changed"));
    }
    Err(unavailable(if saw_unassociated {
        "context_address_unassociated"
    } else {
        "context_pcscf_absent"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        future::ready,
    };

    const ACTIVITY: &str = "+CGACT: 1,0\n+CGACT: 2,1";
    const DEFINITIONS: &str = "+CGDCONT: 1,\"IPV4V6\",\"internet\"\n+CGDCONT: 2,\"IPV4V6\",\"ims\"";
    const READY: &str =
        "+CGCONTRDP: 2,5,ims,2001:db8:1::a,2001:db8:1::b,,,2001:db8:2::10,2001:db8:2::11";

    fn settings() -> CgcontrdpSettings {
        CgcontrdpSettings {
            ipv6_address: Some("2001:db8:1::2".parse().unwrap()),
            ipv6_prefix: Some(64),
            ipv6_gateway: Some("2001:db8:1::1".parse().unwrap()),
            ..Default::default()
        }
    }

    fn response(command: &str, activity: &str, definitions: &str, row: &str) -> String {
        match command {
            "AT+CGACT?" => activity.to_string(),
            "AT+CGDCONT?" => definitions.to_string(),
            "AT+CGCONTRDP=2" => row.to_string(),
            other => panic!("unexpected/non-read-only command: {other}"),
        }
    }

    async fn observe(
        expected: &CgcontrdpSettings,
        pin: Option<u32>,
        activity: &str,
        definitions: &str,
        row: &str,
    ) -> Result<ImsPcscfDiscovery, ImsBearerError> {
        discover_with(
            expected,
            pin,
            "ims",
            Duration::ZERO,
            || ready(Ok(expected.clone())),
            |command| ready(Ok(response(&command, activity, definitions, row))),
        )
        .await
    }

    #[tokio::test]
    async fn sole_pinned_ipv6_context_accepts_different_iids_not_at_addressing() {
        let expected = settings();
        let discovery = observe(&expected, Some(2), ACTIVITY, DEFINITIONS, READY)
            .await
            .unwrap();
        assert_eq!(discovery.source, "mm_owned_at_sole_pinned_ipv6_prefix");
        assert_eq!(discovery.context_id, Some(2));
        assert_eq!(
            discovery.candidates,
            ["2001:db8:2::10", "2001:db8:2::11"].map(|ip| ip.parse::<IpAddr>().unwrap())
        );
        assert_eq!(
            expected,
            settings(),
            "AT must not replace the owned IP/gateway/DNS"
        );
    }

    #[tokio::test]
    async fn matching_full_address_keeps_the_existing_unpinned_path() {
        let row = READY.replace("2001:db8:1::a", "2001:db8:1::2");
        let discovery = observe(&settings(), None, ACTIVITY, DEFINITIONS, &row)
            .await
            .unwrap();
        assert_eq!(discovery.source, "mm_owned_at_exact_address");
    }

    #[tokio::test]
    async fn an_unpinned_prefix_does_not_prove_context_ownership() {
        let error = observe(&settings(), None, ACTIVITY, DEFINITIONS, READY)
            .await
            .unwrap_err();
        assert_eq!(error.kind, ImsBearerErrorKind::PcscfUnavailable);
        assert!(error.detail.ends_with("context_address_unassociated"));
    }

    #[tokio::test]
    async fn requested_profile_three_cannot_borrow_active_context_two() {
        let error = observe(&settings(), Some(3), ACTIVITY, DEFINITIONS, READY)
            .await
            .unwrap_err();
        assert!(error.detail.ends_with("context_pcscf_absent"));
    }

    #[tokio::test]
    async fn another_active_context_blocks_prefix_but_not_exact_match() {
        let activity = "+CGACT: 1,1\n+CGACT: 2,1";
        assert!(observe(&settings(), Some(2), activity, DEFINITIONS, READY)
            .await
            .is_err());
        let exact = READY.replace("2001:db8:1::a", "2001:db8:1::2");
        assert_eq!(
            observe(&settings(), Some(2), activity, DEFINITIONS, &exact)
                .await
                .unwrap()
                .source,
            "mm_owned_at_exact_address"
        );
    }

    #[tokio::test]
    async fn prefix_association_requires_the_actual_64_bit_grant() {
        for prefix in [None, Some(0), Some(48), Some(63), Some(65), Some(128)] {
            let mut expected = settings();
            expected.ipv6_prefix = prefix;
            assert!(observe(&expected, Some(2), ACTIVITY, DEFINITIONS, READY)
                .await
                .is_err());
        }
        for address in [
            "2001:db8:3::a",
            "2001:db8:1::",
            "2001:db8:1::a/48",
            "2001:db8:1::a/129",
            "fe80::a",
            "::ffff:192.0.2.2",
        ] {
            let row = READY.replace("2001:db8:1::a", address);
            assert!(
                observe(&settings(), Some(2), ACTIVITY, DEFINITIONS, &row)
                    .await
                    .is_err(),
                "{address}"
            );
        }
        assert!(observe(
            &settings(),
            Some(2),
            ACTIVITY,
            DEFINITIONS,
            &READY.replace("2001:db8:1::a", "2001:db8:1::a/64")
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn ipv4_never_uses_subnet_similarity_as_an_association() {
        let expected = CgcontrdpSettings {
            ipv4_address: Some("192.0.2.2".parse().unwrap()),
            ipv4_prefix: Some(24),
            ..Default::default()
        };
        let row = "+CGCONTRDP: 2,5,ims,192.0.2.10/24,192.0.2.1,,,192.0.2.20";
        assert!(observe(&expected, Some(2), ACTIVITY, DEFINITIONS, row)
            .await
            .is_err());
        let exact = row.replace("192.0.2.10/24", "192.0.2.2/24");
        assert_eq!(
            observe(&expected, Some(2), ACTIVITY, DEFINITIONS, &exact)
                .await
                .unwrap()
                .source,
            "mm_owned_at_exact_address"
        );
    }

    #[tokio::test]
    async fn cid_apn_and_ipv6_capable_definition_are_all_required() {
        for definitions in [
            DEFINITIONS.replace("\"ims\"", "\"foreign\""),
            DEFINITIONS.replace("2,\"IPV4V6\"", "2,\"IP\""),
            format!("{DEFINITIONS}\n+CGDCONT: 2,\"IPV6\",\"ims\""),
        ] {
            assert!(observe(&settings(), Some(2), ACTIVITY, &definitions, READY)
                .await
                .is_err());
        }
        for row in [
            READY.replace("2,5,ims", "3,5,ims"),
            READY.replace(",ims,", ",foreign,"),
            READY.replace("2,5,ims", "2,0,ims"),
        ] {
            assert!(observe(&settings(), Some(2), ACTIVITY, DEFINITIONS, &row)
                .await
                .is_err());
        }
    }

    #[test]
    fn duplicate_rows_cannot_combine_an_owned_local_with_foreign_pcscf() {
        let first = "+CGCONTRDP: 2,5,ims,2001:db8:1::2,2001:db8:1::1,,,";
        for second in [
            READY.to_string(),
            first.to_string(),
            READY.replace("2001:db8:1::a", "2001:db8:9::2"),
        ] {
            assert!(context_rows(&format!("{first}\n{second}"), 2, "ims").is_err());
        }
        let dual = format!("{READY}\n+CGCONTRDP: 2,6,ims,192.0.2.2,192.0.2.1,,,192.0.2.20");
        assert!(
            context_rows(&dual, 2, "ims").is_err(),
            "contradictory bearer IDs"
        );
    }

    #[test]
    fn decimal_octets_are_parsed_without_guessing_compact_column_offsets() {
        let row = "+CGCONTRDP: 2,5,ims,32.1.13.184.0.1.0.0.0.0.0.0.0.0.0.10,2001:db8:1::1,,,32.1.13.184.0.2.0.0.0.0.0.0.0.0.0.16";
        let parsed = context_rows(row, 2, "ims").unwrap();
        assert_eq!(parsed[0].local, "2001:db8:1::a".parse::<IpAddr>().unwrap());
        assert_eq!(
            parsed[0].candidates,
            ["2001:db8:2::10".parse::<IpAddr>().unwrap()]
        );
        let compact = "+CGCONTRDP: 2,5,ims,192.0.2.2,2001:db8:1::a,fe80::1,,,192.0.2.20,2001:db8:2::10,192.0.2.21,2001:db8:2::11";
        assert!(context_rows(compact, 2, "ims").is_err());
        let compact_zero = "+CGCONTRDP: 2,5,ims,192.0.2.2,::,192.0.2.1,192.0.2.53,192.0.2.54,192.0.2.20,::,192.0.2.21,::";
        assert!(
            context_rows(compact_zero, 2, "ims").is_err(),
            "an unspecified second local must not shift DNS into the P-CSCF columns"
        );
    }

    #[test]
    fn candidates_must_be_valid_same_family_and_never_dns_columns() {
        for candidate in [
            "not-an-ip",
            "ff02::1",
            "fe80::1",
            "::1",
            "192.0.2.20",
            "2001:db8::1 garbage",
        ] {
            assert!(context_rows(&READY.replace("2001:db8:2::10", candidate), 2, "ims").is_err());
        }
        let dns_only = "+CGCONTRDP: 2,5,ims,2001:db8:1::2,2001:db8:1::1,2001:db8::53,,::,::";
        assert!(context_rows(dns_only, 2, "ims").unwrap()[0]
            .candidates
            .is_empty());
    }

    #[test]
    fn malformed_activity_and_error_tails_cannot_authorize_a_prefix() {
        for activity in [
            "+CGACT: 2,1\n+CGACT: 2,1",
            "+CGACT: 2,1\n+CGACT: 2,0",
            "+CGACT: 2,unknown",
            "+CGACT: 2,1\nERROR",
            "+CGACT: 2,1\nOK\n+CME ERROR: 3",
        ] {
            assert!(context_state(activity, DEFINITIONS).is_err());
        }
        for tail in ["ERROR", "+CME ERROR: 3", "OK\ntrailing", "+UNKNOWN: data"] {
            assert!(context_rows(&format!("{READY}\n{tail}"), 2, "ims").is_err());
        }
        assert!(context_rows(&"x".repeat(MAX_AT_BYTES + 1), 2, "ims").is_err());
    }

    #[tokio::test]
    async fn changed_mm_ip_or_owner_is_terminal_not_a_dns_fallback() {
        for lose_owner in [false, true] {
            let expected = settings();
            let reads = Cell::new(0);
            let error = discover_with(
                &expected,
                Some(2),
                "ims",
                Duration::ZERO,
                || {
                    let index = reads.get();
                    reads.set(index + 1);
                    let mut current = expected.clone();
                    if index > 0 {
                        current.ipv6_address = Some("2001:db8:1::3".parse().unwrap());
                    }
                    ready(if index > 0 && lose_owner {
                        Err(session_changed("owner_changed"))
                    } else {
                        Ok(current)
                    })
                },
                |command| ready(Ok(response(&command, ACTIVITY, DEFINITIONS, READY))),
            )
            .await
            .unwrap_err();
            assert_eq!(error.kind, ImsBearerErrorKind::SessionLost);
            assert_eq!(reads.get(), 2);
        }
    }

    #[tokio::test]
    async fn changed_context_or_pco_is_rejected_before_publication() {
        for change_definition in [false, true] {
            let expected = settings();
            let definitions = Cell::new(0);
            let contexts = Cell::new(0);
            let error = discover_with(
                &expected,
                Some(2),
                "ims",
                Duration::ZERO,
                || ready(Ok(expected.clone())),
                |command| {
                    let mut output = response(&command, ACTIVITY, DEFINITIONS, READY);
                    if command == "AT+CGDCONT?" {
                        definitions.set(definitions.get() + 1);
                        if definitions.get() == 2 && change_definition {
                            output = DEFINITIONS.replace("\"ims\"", "\"foreign\"");
                        }
                    }
                    if command == "AT+CGCONTRDP=2" {
                        contexts.set(contexts.get() + 1);
                        if contexts.get() == 2 {
                            output = READY.replace("2001:db8:2::10", "2001:db8:2::99");
                        }
                    }
                    ready(Ok(output))
                },
            )
            .await
            .unwrap_err();
            assert!(error.detail.ends_with("context_changed"));
        }
    }

    #[test]
    fn gateway_and_extended_profile_changes_are_not_hidden_by_stable_candidates() {
        let first = context_rows(READY, 2, "ims").unwrap();
        let changed =
            context_rows(&READY.replace("2001:db8:1::b", "2001:db8:1::c"), 2, "ims").unwrap();
        assert_ne!(first, changed);
        assert!(context_rows(
            &READY.replace("2001:db8:1::b", "2001:db8:1::b garbage"),
            2,
            "ims"
        )
        .is_err());
        assert_ne!(
            context_state(ACTIVITY, &format!("{DEFINITIONS},0,0")).unwrap(),
            context_state(ACTIVITY, &format!("{DEFINITIONS},0,1")).unwrap()
        );
    }

    #[tokio::test]
    async fn retry_budget_is_read_only_and_reports_missing_pcscf_separately() {
        let expected = settings();
        let calls = RefCell::new(Vec::new());
        let missing = "+CGCONTRDP: 2,5,ims,2001:db8:1::a,2001:db8:1::b,,";
        let error = discover_with(
            &expected,
            Some(2),
            "ims",
            Duration::ZERO,
            || ready(Ok(expected.clone())),
            |command| {
                calls.borrow_mut().push(command.clone());
                ready(Ok(response(&command, ACTIVITY, DEFINITIONS, missing)))
            },
        )
        .await
        .unwrap_err();
        assert!(error.detail.ends_with("context_pcscf_absent"));
        assert_eq!(
            calls
                .borrow()
                .iter()
                .filter(|command| *command == "AT+CGCONTRDP=2")
                .count(),
            READ_ROUNDS
        );
        assert_eq!(calls.borrow().len(), 3 * READ_ROUNDS);
    }

    #[tokio::test]
    async fn invalid_or_stale_grant_never_starts_at_discovery() {
        for missing in [false, true] {
            let expected = if missing {
                CgcontrdpSettings::default()
            } else {
                settings()
            };
            let error = discover_with(
                &expected,
                Some(2),
                "ims",
                Duration::ZERO,
                || ready(Ok(CgcontrdpSettings::default())),
                |_| ready(Err(unavailable("must_not_be_called"))),
            )
            .await
            .unwrap_err();
            assert_eq!(error.kind, ImsBearerErrorKind::SessionLost);
        }
    }
}
