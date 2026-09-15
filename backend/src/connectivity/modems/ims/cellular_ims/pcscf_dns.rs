//! Bounded DNS parsing for the UE-local P-CSCF resolver.
//!
//! Only the answer to our exact IN question (or its answer-section CNAME
//! chain) may supply an endpoint. Authority/additional glue is never a SIP
//! destination. This validates association and framing, not DNS authenticity.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use super::{errors::code, CellularImsError};

const IN: u16 = 1;
const A: u16 = 1;
const AAAA: u16 = 28;
const CNAME: u16 = 5;
const SRV: u16 = 33;
const MAX_CNAME_HOPS: usize = 8;

fn invalid(reason: &str) -> CellularImsError {
    CellularImsError::with_detail(code::RUNTIME_ALL_PCSCF_FAILED, format!("dns_{reason}"))
}

/// Labels stay separate: a wire label containing '.' is not two DNS labels.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Name(Vec<Vec<u8>>);

impl Name {
    fn host(value: &str) -> Result<Self, CellularImsError> {
        let value = value.strip_suffix('.').unwrap_or(value);
        let labels = value
            .split('.')
            .map(|label| {
                if label.is_empty()
                    || label.len() > 63
                    || !label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                {
                    return Err(invalid("query_name"));
                }
                Ok(label.as_bytes().to_ascii_lowercase())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if 1 + labels.iter().map(|label| label.len() + 1).sum::<usize>() > 255 {
            return Err(invalid("name_too_long"));
        }
        Ok(Self(labels))
    }

    fn host_text(&self) -> Option<String> {
        if self.0.is_empty()
            || self.0.iter().any(|label| {
                !label
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
            })
        {
            return None;
        }
        Some(
            self.0
                .iter()
                .map(|label| String::from_utf8_lossy(label))
                .collect::<Vec<_>>()
                .join("."),
        )
    }

    fn append_wire(&self, target: &mut Vec<u8>) {
        for label in &self.0 {
            target.push(label.len() as u8);
            target.extend_from_slice(label);
        }
        target.push(0);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SrvTarget {
    pub target: String,
    pub port: u16,
    pub priority: u16,
    pub weight: u16,
}

impl SrvTarget {
    pub fn endpoint(&self, address: IpAddr) -> SocketAddr {
        SocketAddr::new(address, self.port)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct DnsRecords {
    pub addresses: Vec<IpAddr>,
    pub srv_targets: Vec<SrvTarget>,
}

pub(super) fn build_dns_query(
    id: u16,
    name: &str,
    record_type: u16,
) -> Result<Vec<u8>, CellularImsError> {
    if !matches!(record_type, A | AAAA | SRV) {
        return Err(invalid("query_type"));
    }
    let name = Name::host(name)?;
    let mut query = Vec::with_capacity(272);
    for value in [id, 0x0100, 1, 0, 0, 0] {
        query.extend_from_slice(&value.to_be_bytes());
    }
    name.append_wire(&mut query);
    query.extend_from_slice(&record_type.to_be_bytes());
    query.extend_from_slice(&IN.to_be_bytes());
    Ok(query)
}

fn word(packet: &[u8], offset: usize) -> Result<u16, CellularImsError> {
    let bytes = packet
        .get(offset..offset + 2)
        .ok_or_else(|| invalid("truncated"))?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

/// Returns the end of the encoded name, not the end of its compression target.
fn read_name(packet: &[u8], start: usize) -> Result<(Name, usize), CellularImsError> {
    let mut labels = Vec::new();
    let mut offset = start;
    let mut encoded_end = None;
    let mut expanded_bytes = 1usize;
    let mut visited = Vec::new();
    for _ in 0..128 {
        if visited.contains(&offset) {
            return Err(invalid("compression_loop"));
        }
        visited.push(offset);
        let length = *packet
            .get(offset)
            .ok_or_else(|| invalid("truncated_name"))?;
        if length == 0 {
            return Ok((Name(labels), encoded_end.unwrap_or(offset + 1)));
        }
        match length & 0xc0 {
            0xc0 => {
                let low = *packet
                    .get(offset + 1)
                    .ok_or_else(|| invalid("truncated_pointer"))?;
                let target = (usize::from(length & 0x3f) << 8) | usize::from(low);
                // RFC 1035 compression points to a prior occurrence, never the
                // header, future RDATA, or itself.
                if target < 12 || target >= offset {
                    return Err(invalid("compression_target"));
                }
                encoded_end.get_or_insert(offset + 2);
                offset = target;
            }
            0 => {
                let end = offset + 1 + usize::from(length);
                let label = packet
                    .get(offset + 1..end)
                    .ok_or_else(|| invalid("truncated_label"))?;
                expanded_bytes += label.len() + 1;
                if expanded_bytes > 255 {
                    return Err(invalid("name_too_long"));
                }
                labels.push(label.to_ascii_lowercase());
                offset = end;
            }
            _ => return Err(invalid("label_encoding")),
        }
    }
    Err(invalid("name_indirection_limit"))
}

#[derive(Debug)]
enum Data {
    Address(IpAddr),
    Alias(Name),
    Service(Option<SrvTarget>),
    Other,
}

struct Answer {
    owner: Name,
    kind: u16,
    data: Data,
}

pub(super) fn parse_dns_response(
    id: u16,
    expected_name: &str,
    expected_type: u16,
    packet: &[u8],
) -> Result<DnsRecords, CellularImsError> {
    if packet.len() < 12 || packet.len() > 65535 || word(packet, 0)? != id {
        return Err(invalid("header"));
    }
    if !matches!(expected_type, A | AAAA | SRV) {
        return Err(invalid("query_type"));
    }
    let flags = word(packet, 2)?;
    // QR, standard opcode, no truncation, reserved Z=0, successful RCODE.
    if flags & 0x8000 == 0 || flags & (0x7800 | 0x0200 | 0x0040 | 0x000f) != 0 {
        return Err(invalid("response_flags"));
    }
    if word(packet, 4)? != 1 {
        return Err(invalid("question_count"));
    }
    let expected = Name::host(expected_name)?;
    let (question, question_end) = read_name(packet, 12)?;
    if question != expected
        || word(packet, question_end)? != expected_type
        || word(packet, question_end + 2)? != IN
    {
        return Err(invalid("question_mismatch"));
    }
    let mut offset = question_end + 4;
    let mut answers = Vec::new();
    let mut seen_opt = false;
    for section in 0..3 {
        let count = word(packet, 6 + section * 2)?;
        for _ in 0..count {
            let (owner, header) = read_name(packet, offset)?;
            let kind = word(packet, header)?;
            let class = word(packet, header + 2)?;
            let length = usize::from(word(packet, header + 8)?);
            let start = header + 10;
            let end = start
                .checked_add(length)
                .filter(|end| *end <= packet.len())
                .ok_or_else(|| invalid("rdata_length"))?;
            let data = match kind {
                A => {
                    let octets: [u8; 4] = packet[start..end]
                        .try_into()
                        .map_err(|_| invalid("a_length"))?;
                    Data::Address(IpAddr::V4(Ipv4Addr::from(octets)))
                }
                AAAA => {
                    let octets: [u8; 16] = packet[start..end]
                        .try_into()
                        .map_err(|_| invalid("aaaa_length"))?;
                    Data::Address(IpAddr::V6(Ipv6Addr::from(octets)))
                }
                CNAME => {
                    let (target, consumed) = read_name(packet, start)?;
                    if consumed != end || target.0.is_empty() {
                        return Err(invalid("cname_rdata"));
                    }
                    Data::Alias(target)
                }
                SRV => {
                    if length < 7 {
                        return Err(invalid("srv_length"));
                    }
                    let (target, consumed) = read_name(packet, start + 6)?;
                    if consumed != end {
                        return Err(invalid("srv_rdata"));
                    }
                    let port = word(packet, start + 4)?;
                    // Root target means service unavailable; port zero is not
                    // a usable SIP service. Neither may become port 5060.
                    let target = target
                        .host_text()
                        .filter(|_| port != 0)
                        .map(|target| SrvTarget {
                            target,
                            port,
                            priority: word(packet, start).unwrap(),
                            weight: word(packet, start + 2).unwrap(),
                        });
                    Data::Service(target)
                }
                41 => {
                    if section != 2 || seen_opt || !owner.0.is_empty() || packet[header + 4] != 0 {
                        return Err(invalid("opt_extended_rcode"));
                    }
                    seen_opt = true;
                    Data::Other
                }
                _ => Data::Other,
            };
            if section == 0 && class == IN {
                answers.push(Answer { owner, kind, data });
            }
            offset = end;
        }
    }
    if offset != packet.len() {
        return Err(invalid("trailing_data"));
    }

    let mut terminal = expected;
    let mut visited = vec![terminal.clone()];
    for hop in 0..=MAX_CNAME_HOPS {
        let mut next: Option<&Name> = None;
        for answer in answers.iter().filter(|answer| answer.owner == terminal) {
            if let Data::Alias(target) = &answer.data {
                if next.is_some_and(|previous| previous != target) {
                    return Err(invalid("cname_conflict"));
                }
                next = Some(target);
            }
        }
        let Some(target) = next else { break };
        if hop == MAX_CNAME_HOPS || visited.contains(target) {
            return Err(invalid("cname_chain_limit"));
        }
        if answers.iter().any(|answer| {
            answer.owner == terminal && matches!(&answer.data, Data::Address(_) | Data::Service(_))
        }) {
            return Err(invalid("cname_data_conflict"));
        }
        terminal = target.clone();
        visited.push(terminal.clone());
    }
    let mut result = DnsRecords::default();
    for answer in answers
        .into_iter()
        .filter(|answer| answer.owner == terminal && answer.kind == expected_type)
    {
        match answer.data {
            Data::Address(address) if !result.addresses.contains(&address) => {
                result.addresses.push(address)
            }
            Data::Service(Some(target)) if !result.srv_targets.contains(&target) => {
                result.srv_targets.push(target)
            }
            _ => {}
        }
    }
    // Keep response order within a priority. Full weighted SRV scheduling is
    // separate from this bounded, single-result P-CSCF fallback.
    result.srv_targets.sort_by_key(|target| target.priority);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: u16 = 0x1234;
    const HOST: &str = "pcscf.ims.example";

    fn wire(name: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        if name == "." {
            bytes.push(0);
        } else {
            Name::host(name).unwrap().append_wire(&mut bytes);
        }
        bytes
    }

    fn response(name: &str, kind: u16, counts: [u16; 3]) -> Vec<u8> {
        let mut packet = build_dns_query(ID, name, kind).unwrap();
        packet[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
        for (index, count) in counts.into_iter().enumerate() {
            packet[6 + 2 * index..8 + 2 * index].copy_from_slice(&count.to_be_bytes());
        }
        packet
    }

    fn record(packet: &mut Vec<u8>, owner: &[u8], kind: u16, class: u16, bytes: &[u8]) {
        packet.extend_from_slice(owner);
        packet.extend_from_slice(&kind.to_be_bytes());
        packet.extend_from_slice(&class.to_be_bytes());
        packet.extend_from_slice(&60u32.to_be_bytes());
        packet.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        packet.extend_from_slice(bytes);
    }

    fn parse(packet: &[u8]) -> Result<DnsRecords, CellularImsError> {
        parse_dns_response(ID, HOST, AAAA, packet)
    }

    #[test]
    fn accepts_only_the_exact_question_answer_and_deduplicates() {
        let mut packet = response(HOST, AAAA, [3, 0, 0]);
        let ip = "2001:db8::10".parse::<Ipv6Addr>().unwrap();
        record(&mut packet, &[0xc0, 12], AAAA, IN, &ip.octets());
        record(
            &mut packet,
            &wire("unrelated.example"),
            AAAA,
            IN,
            &Ipv6Addr::LOCALHOST.octets(),
        );
        record(&mut packet, &wire(HOST), AAAA, IN, &ip.octets());
        assert_eq!(parse(&packet).unwrap().addresses, vec![IpAddr::V6(ip)]);
        assert_eq!(
            parse_dns_response(ID, "PCSCF.IMS.EXAMPLE.", AAAA, &packet)
                .unwrap()
                .addresses,
            vec![IpAddr::V6(ip)]
        );
    }

    #[test]
    fn authority_and_additional_glue_cannot_become_sip_destinations() {
        let mut packet = response(HOST, AAAA, [0, 1, 2]);
        record(
            &mut packet,
            &wire("ims.example"),
            2,
            IN,
            &wire("ns.ims.example"),
        );
        record(
            &mut packet,
            &wire("ns.ims.example"),
            AAAA,
            IN,
            &Ipv6Addr::LOCALHOST.octets(),
        );
        // Even matching-owner additional data is not an answer to our question.
        record(
            &mut packet,
            &wire(HOST),
            AAAA,
            IN,
            &Ipv6Addr::LOCALHOST.octets(),
        );
        assert_eq!(parse(&packet).unwrap(), DnsRecords::default());
    }

    #[test]
    fn rejects_header_and_question_mismatches() {
        let packet = response(HOST, AAAA, [0, 0, 0]);
        assert!(parse_dns_response(ID + 1, HOST, AAAA, &packet).is_err());
        assert!(parse_dns_response(ID, "other.example", AAAA, &packet).is_err());
        assert!(parse_dns_response(ID, HOST, A, &packet).is_err());
        for flags in [0x0180u16, 0x8980, 0x8380, 0x8183, 0x81c0] {
            let mut bad = packet.clone();
            bad[2..4].copy_from_slice(&flags.to_be_bytes());
            assert!(parse(&bad).is_err(), "flags {flags:x}");
        }
        for count in [0u16, 2] {
            let mut bad = packet.clone();
            bad[4..6].copy_from_slice(&count.to_be_bytes());
            assert!(parse(&bad).is_err());
        }
        let mut bad = packet.clone();
        let end = bad.len();
        bad[end - 1] = 3;
        assert!(parse(&bad).is_err());
    }

    #[test]
    fn follows_out_of_order_bounded_answer_cnames_only() {
        let mut packet = response(HOST, AAAA, [5, 0, 0]);
        record(
            &mut packet,
            &wire("b.example"),
            AAAA,
            IN,
            &Ipv6Addr::LOCALHOST.octets(),
        );
        record(
            &mut packet,
            &wire("a.example"),
            CNAME,
            IN,
            &wire("b.example"),
        );
        record(&mut packet, &wire(HOST), CNAME, IN, &wire("a.example"));
        record(
            &mut packet,
            &wire("unrelated.example"),
            CNAME,
            IN,
            &wire("evil.example"),
        );
        record(&mut packet, &wire("evil.example"), AAAA, IN, &[0xff; 16]);
        assert_eq!(
            parse(&packet).unwrap().addresses,
            vec![IpAddr::V6(Ipv6Addr::LOCALHOST)]
        );
    }

    #[test]
    fn rejects_cname_loops_conflicts_and_excessive_chains() {
        let mut packet = response(HOST, AAAA, [2, 0, 0]);
        record(&mut packet, &wire(HOST), CNAME, IN, &wire("a.example"));
        record(&mut packet, &wire("a.example"), CNAME, IN, &wire(HOST));
        assert!(parse(&packet).is_err());
        let mut conflict = response(HOST, AAAA, [2, 0, 0]);
        record(&mut conflict, &wire(HOST), CNAME, IN, &wire("a.example"));
        record(&mut conflict, &wire(HOST), CNAME, IN, &wire("b.example"));
        assert!(parse(&conflict).is_err());
        let mut data_conflict = response(HOST, AAAA, [2, 0, 0]);
        record(
            &mut data_conflict,
            &wire(HOST),
            CNAME,
            IN,
            &wire("a.example"),
        );
        record(&mut data_conflict, &wire(HOST), AAAA, IN, &[0; 16]);
        assert!(parse(&data_conflict).is_err());
        for hops in [8, 9] {
            let mut chain = response(HOST, AAAA, [hops + 1, 0, 0]);
            let mut owner = HOST.to_string();
            for hop in 0..hops {
                let target = format!("alias{hop}.example");
                record(&mut chain, &wire(&owner), CNAME, IN, &wire(&target));
                owner = target;
            }
            record(
                &mut chain,
                &wire(&owner),
                AAAA,
                IN,
                &Ipv6Addr::LOCALHOST.octets(),
            );
            assert_eq!(parse(&chain).is_ok(), hops == 8);
        }
    }

    #[test]
    fn ignores_wrong_class_family_and_unrelated_cname_data() {
        let mut packet = response(HOST, AAAA, [3, 0, 0]);
        record(&mut packet, &wire(HOST), AAAA, 3, &[0; 16]);
        record(&mut packet, &wire(HOST), A, IN, &[192, 0, 2, 1]);
        record(&mut packet, &wire("other.example"), CNAME, IN, &wire(HOST));
        assert!(parse(&packet).unwrap().addresses.is_empty());
    }

    #[test]
    fn name_label_boundaries_are_not_flattened_for_comparison() {
        let mut packet = response(HOST, AAAA, [0, 0, 0]);
        let replacement = [
            vec![9],
            b"pcscf.ims".to_vec(),
            vec![7],
            b"example".to_vec(),
            vec![0],
        ]
        .concat();
        packet.splice(12..12 + wire(HOST).len(), replacement);
        assert!(parse(&packet).is_err());
    }

    #[test]
    fn compression_is_bounded_and_rdata_cannot_borrow_the_next_record() {
        let mut valid = response(HOST, AAAA, [2, 0, 0]);
        let target_offset = valid.len();
        record(
            &mut valid,
            &wire("target.example"),
            AAAA,
            IN,
            &Ipv6Addr::LOCALHOST.octets(),
        );
        let pointer = (0xc000u16 | target_offset as u16).to_be_bytes();
        record(&mut valid, &wire(HOST), CNAME, IN, &pointer);
        assert_eq!(parse(&valid).unwrap().addresses.len(), 1);
        let mut bad = response(HOST, AAAA, [1, 0, 0]);
        record(&mut bad, &wire(HOST), CNAME, IN, &[0xc0]);
        bad.push(12);
        assert!(parse(&bad).is_err());
        let mut self_pointer = response(HOST, AAAA, [1, 0, 0]);
        let offset = self_pointer.len();
        record(
            &mut self_pointer,
            &(0xc000u16 | offset as u16).to_be_bytes(),
            AAAA,
            IN,
            &[0; 16],
        );
        assert!(parse(&self_pointer).is_err());
        let mut out_of_bounds = response(HOST, AAAA, [1, 0, 0]);
        record(&mut out_of_bounds, &[0xff, 0xff], AAAA, IN, &[0; 16]);
        assert!(parse(&out_of_bounds).is_err());
    }

    #[test]
    fn rejects_truncation_invalid_address_lengths_and_oversized_names() {
        let packet = response(HOST, AAAA, [0, 0, 0]);
        for length in 0..packet.len() {
            assert!(parse(&packet[..length]).is_err());
        }
        for (kind, length) in [(A, 3), (AAAA, 15)] {
            let mut bad = response(HOST, AAAA, [1, 0, 0]);
            record(&mut bad, &wire(HOST), kind, IN, &vec![0; length]);
            assert!(parse(&bad).is_err());
        }
        assert!(build_dns_query(ID, &"x".repeat(64), AAAA).is_err());
        assert!(build_dns_query(ID, &format!("{0}.{0}.{0}.{0}", "x".repeat(63)), AAAA).is_err());
        let mut expanded = vec![0; 12];
        for _ in 0..4 {
            expanded.push(63);
            expanded.extend_from_slice(&[b'x'; 63]);
        }
        expanded.push(0);
        assert!(read_name(&expanded, 12).is_err());
        assert!(build_dns_query(ID, "bad..example", AAAA).is_err());
        assert!(build_dns_query(ID, "example..", AAAA).is_err());
        assert!(build_dns_query(ID, HOST, CNAME).is_err());
    }

    fn srv(port: u16, target: &str, priority: u16) -> Vec<u8> {
        let mut bytes = Vec::new();
        for value in [priority, 0, port] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        bytes.extend(wire(target));
        bytes
    }

    #[test]
    fn srv_preserves_service_ports_and_priorities_and_ignores_glue() {
        let name = "_sip._udp.ims.example";
        let mut packet = response(name, SRV, [3, 0, 1]);
        record(
            &mut packet,
            &[0xc0, 12],
            SRV,
            IN,
            &srv(5078, "p1.ims.example", 20),
        );
        record(
            &mut packet,
            &[0xc0, 12],
            SRV,
            IN,
            &srv(5088, "p1.ims.example", 10),
        );
        record(
            &mut packet,
            &[0xc0, 12],
            SRV,
            IN,
            &srv(5078, "p1.ims.example", 20),
        );
        record(
            &mut packet,
            &wire("p1.ims.example"),
            A,
            IN,
            &[192, 0, 2, 10],
        );
        let result = parse_dns_response(ID, name, SRV, &packet).unwrap();
        assert!(result.addresses.is_empty());
        assert_eq!(
            result
                .srv_targets
                .iter()
                .map(|s| s.port)
                .collect::<Vec<_>>(),
            [5088, 5078]
        );
        assert_eq!(
            result.srv_targets[1]
                .endpoint("192.0.2.10".parse().unwrap())
                .to_string(),
            "192.0.2.10:5078"
        );
    }

    #[test]
    fn srv_root_zero_port_and_malformed_rdata_do_not_produce_endpoints() {
        let name = "_sip._udp.ims.example";
        let mut packet = response(name, SRV, [2, 0, 0]);
        record(&mut packet, &[0xc0, 12], SRV, IN, &srv(5060, ".", 0));
        record(&mut packet, &[0xc0, 12], SRV, IN, &srv(0, "p1.example", 0));
        assert!(parse_dns_response(ID, name, SRV, &packet)
            .unwrap()
            .srv_targets
            .is_empty());
        for bytes in [
            vec![0; 6],
            [vec![0, 0, 0, 0, 0x13, 0xc4], wire("p1.example"), vec![0]].concat(),
        ] {
            let mut malformed = response(name, SRV, [1, 0, 0]);
            record(&mut malformed, &[0xc0, 12], SRV, IN, &bytes);
            assert!(parse_dns_response(ID, name, SRV, &malformed).is_err());
        }
    }

    #[test]
    fn rejects_nonzero_extended_rcode_and_packet_trailing_bytes() {
        let mut packet = response(HOST, AAAA, [0, 0, 1]);
        let at = packet.len();
        record(&mut packet, &[0], 41, 4096, &[]);
        packet[at + 5] = 1;
        assert!(parse(&packet).is_err());
        let mut trailing = response(HOST, AAAA, [0, 0, 0]);
        trailing.push(0);
        assert!(parse(&trailing).is_err());
    }
}
