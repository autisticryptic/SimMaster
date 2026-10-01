//! Extra read-only proof for an owned IPv6 profile alongside disjoint contexts.
//! Only target CID P-CSCF is consumed; other contexts are exclusion evidence.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
struct OtherRow {
    bearer_id: u8,
    apn: String,
    local: IpAddr,
    prefix: Option<u8>,
    remaining: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Evidence {
    assigned: IpAddr,
    other: BTreeMap<u8, Vec<OtherRow>>,
}

fn literal(value: &str) -> Option<IpAddr> {
    if let Ok(ip) = value.parse::<IpAddr>() {
        return Some(ip);
    }
    let octets: Vec<_> = value.split('.').collect();
    if octets.len() != 16
        || octets.iter().any(|part| {
            part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit())
        })
    {
        return None;
    }
    let bytes: Vec<u8> = octets
        .into_iter()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    Some(IpAddr::V6(std::net::Ipv6Addr::from(
        <[u8; 16]>::try_from(bytes).ok()?,
    )))
}

fn assigned_address(output: &str, target: u8, expected: IpAddr) -> Result<IpAddr, ImsBearerError> {
    let lines = data_lines(output, &format!("AT+CGPADDR={target}"), "+CGPADDR:")?;
    let [line] = lines.as_slice() else {
        return Err(unavailable("assigned_address_ambiguous"));
    };
    let fields = line.split(',').map(field).collect::<Result<Vec<_>, _>>()?;
    if fields.len() != 2 || cid(fields[0])? != target {
        return Err(unavailable("assigned_address_ambiguous"));
    }
    let address = literal(fields[1])
        .filter(|address| address.is_ipv6() && usable(*address))
        .ok_or_else(|| unavailable("assigned_address_invalid"))?;
    if address != expected {
        return Err(unavailable("assigned_address_mismatch"));
    }
    Ok(address)
}

fn other_rows(
    output: &str,
    target: u8,
    definition: &Definition,
    ims_apn: &str,
) -> Result<Vec<OtherRow>, ImsBearerError> {
    if !matches!(definition.pdp_type.as_str(), "IP" | "IPV6" | "IPV4V6")
        || definition.apn.eq_ignore_ascii_case(ims_apn)
    {
        return Err(unavailable("other_context_not_disjoint"));
    }
    let mut rows: Vec<OtherRow> = Vec::new();
    for line in data_lines(output, &format!("AT+CGCONTRDP={target}"), "+CGCONTRDP:")? {
        let values = line.split(',').map(field).collect::<Result<Vec<_>, _>>()?;
        if values.len() < 7
            || cid(values[0])? != target
            || values[2].is_empty()
            || values[2].eq_ignore_ascii_case(ims_apn)
            || (!definition.apn.is_empty() && !values[2].eq_ignore_ascii_case(&definition.apn))
        {
            return Err(unavailable("other_context_row_mismatch"));
        }
        let bearer_id = values[1]
            .parse::<u8>()
            .ok()
            .filter(|id| *id != 0)
            .ok_or_else(|| unavailable("other_context_bearer_invalid"))?;
        let (local, prefix) = parse_cgcontrdp_addr_and_mask(values[3])
            .filter(|(ip, _)| usable(*ip))
            .ok_or_else(|| unavailable("other_context_address_invalid"))?;
        if rows.iter().any(|row| {
            row.local.is_ipv4() == local.is_ipv4()
                || row.bearer_id != bearer_id
                || !row.apn.eq_ignore_ascii_case(values[2])
        }) {
            return Err(unavailable("other_context_rows_ambiguous"));
        }
        if !values[4].is_empty()
            && literal(values[4]).is_none_or(|gateway| gateway.is_ipv4() != local.is_ipv4())
        {
            return Err(unavailable("other_context_layout_unsupported"));
        }
        rows.push(OtherRow {
            bearer_id,
            apn: values[2].to_ascii_lowercase(),
            local,
            prefix,
            remaining: values[4..].iter().map(|value| value.to_string()).collect(),
        });
    }
    let has_v6 = rows.iter().any(|row| row.local.is_ipv6());
    // An IPv6-capable context without an observable IPv6 address is UNKNOWN,
    // not proof that it cannot overlap the target. Empty negotiated APN is also
    // unknown. A stored empty APN may legitimately negotiate INTERNET.
    if rows.is_empty()
        || (definition.pdp_type == "IP" && has_v6)
        || (definition.pdp_type != "IP" && !has_v6)
        || (definition.pdp_type == "IPV6" && rows.iter().any(|row| row.local.is_ipv4()))
    {
        return Err(unavailable("other_context_family_unverified"));
    }
    Ok(rows)
}

pub(super) async fn observe<At, AtFuture>(
    target: u8,
    target_row: &ContextRow,
    ims_apn: &str,
    state: &ContextState,
    at: &mut At,
) -> Result<Evidence, ImsBearerError>
where
    At: FnMut(String) -> AtFuture,
    AtFuture: Future<Output = Result<String, ImsBearerError>>,
{
    let target_local = target_row.local;
    let IpAddr::V6(target_ip) = target_local else {
        return Err(unavailable("separated_context_requires_ipv6"));
    };
    if state.active.len() < 2
        || !state.active.contains(&target)
        || !state
            .definitions
            .get(&target)
            .is_some_and(|d| d.pdp_type == "IPV6" && d.apn.eq_ignore_ascii_case(ims_apn))
    {
        return Err(unavailable("separated_context_not_admitted"));
    }
    let assigned = assigned_address(
        &at(format!("AT+CGPADDR={target}")).await?,
        target,
        target_local,
    )?;
    let mut other = BTreeMap::new();
    for cid in state.active.iter().copied().filter(|cid| *cid != target) {
        let definition = state
            .definitions
            .get(&cid)
            .ok_or_else(|| unavailable("active_definition_missing"))?;
        let rows = other_rows(
            &at(format!("AT+CGCONTRDP={cid}")).await?,
            cid,
            definition,
            ims_apn,
        )?;
        for row in &rows {
            if row.bearer_id == target_row.bearer_id
                || other.values().any(|previous: &Vec<OtherRow>| {
                    previous
                        .iter()
                        .any(|prior| prior.bearer_id == row.bearer_id)
                })
            {
                return Err(unavailable("other_context_bearer_ambiguous"));
            }
            if let IpAddr::V6(address) = row.local {
                let bytes = address.octets();
                // Only a disjoint 3GPP /64 can exclude another active PDN.
                // An explicitly different prefix, zero IID or overlap cannot.
                if row.prefix.is_some_and(|prefix| prefix != 64)
                    || bytes[8..] == [0; 8]
                    || bytes[..8] == target_ip.octets()[..8]
                {
                    return Err(unavailable("other_context_prefix_ambiguous"));
                }
            }
        }
        other.insert(cid, rows);
    }
    Ok(Evidence { assigned, other })
}

#[cfg(test)]
#[path = "primary_ims_pcscf_separated_tests.rs"]
mod tests;
