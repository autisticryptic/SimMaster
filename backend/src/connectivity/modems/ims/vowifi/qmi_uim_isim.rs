//! Read-only TS 31.103 ISIM EFs on an already selected ISIM ADF.
//!
//! The exchange adapter owns the channel and resolves 6C/61/9F follow-ups.
//! This layer never selects another application, derives an identity or runs AKA.

use super::UimApduResponse;
use std::{fmt, net::{IpAddr, Ipv4Addr, Ipv6Addr}};

pub const ISIM_AID_PREFIX: &[u8] = &[0xa0, 0, 0, 0, 0x87, 0x10, 0x04];
pub const EF_IMPI: u16 = 0x6f02;
pub const EF_DOMAIN: u16 = 0x6f03;
pub const EF_IMPU: u16 = 0x6f04;
pub const EF_PCSCF: u16 = 0x6f09;
const MAX_EF_BYTES: usize = 4096;
const MAX_RECORDS: usize = 32;
const MAX_FCP_BYTES: usize = 1024;

/// Validated, optional ISIM provisioning, without any IMSI-derived fallback.
/// Public fields permit callers to construct values; only reader/parser output
/// is guaranteed validated. Debug intentionally reports presence/counts only.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct IsimImsMaterial {
    pub impi: Option<String>,
    pub domain: Option<String>,
    pub impus: Vec<String>,
    pub pcscf: Vec<String>,
}

impl fmt::Debug for IsimImsMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IsimImsMaterial")
            .field("has_impi", &self.impi.is_some())
            .field("has_domain", &self.domain.is_some())
            .field("impu_count", &self.impus.len())
            .field("pcscf_count", &self.pcscf.len())
            .finish()
    }
}

/// Require the complete registered ISIM application prefix, not a partial AID
/// or a USIM AID. ISO application identifiers have at most 16 bytes.
pub fn validate_isim_aid(aid: &[u8]) -> Result<(), &'static str> {
    if aid.len() > 16 || !aid.starts_with(ISIM_AID_PREFIX) {
        return Err("isim_invalid_aid");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Transparent { size: usize },
    Records { size: usize, length: usize, count: usize },
}

// Definite BER lengths only. Long-form lengths (including non-minimal BER)
// are supported, but wider-than-u16 and indefinite lengths are not needed by
// these bounded EFs. Every addition and slice is checked before use.
fn take_tlv<'a>(input: &mut &'a [u8]) -> Result<(u32, &'a [u8]), &'static str> {
    let mut offset = 1usize;
    let first = *input.first().ok_or("isim_tlv_invalid")?;
    if matches!(first, 0 | 0xff) {
        return Err("isim_tlv_invalid");
    }
    let mut tag = u32::from(first);
    if first & 0x1f == 0x1f {
        let mut number = 0u32;
        loop {
            let byte = *input.get(offset).ok_or("isim_tlv_invalid")?;
            if offset > 2 || (offset == 1 && byte & 0x7f == 0) {
                return Err("isim_tlv_invalid");
            }
            tag = (tag << 8) | u32::from(byte);
            number = (number << 7) | u32::from(byte & 0x7f);
            offset += 1;
            if byte & 0x80 == 0 {
                if number < 31 { return Err("isim_tlv_invalid"); }
                break;
            }
        }
    }
    let first_length = *input.get(offset).ok_or("isim_tlv_invalid")?;
    offset += 1;
    let length = match first_length {
        0..=0x7f => usize::from(first_length),
        0x81 | 0x82 => {
            let width = usize::from(first_length & 0x7f);
            let bytes = input.get(offset..offset + width).ok_or("isim_tlv_invalid")?;
            offset += width;
            bytes.iter().fold(0usize, |n, b| (n << 8) | usize::from(*b))
        }
        _ => return Err("isim_tlv_invalid"),
    };
    let end = offset.checked_add(length).ok_or("isim_tlv_invalid")?;
    let value = input.get(offset..end).ok_or("isim_tlv_invalid")?;
    *input = input.get(end..).ok_or("isim_tlv_invalid")?;
    Ok((tag, value))
}

fn validate_constructed(mut content: &[u8], depth: usize) -> Result<(), &'static str> {
    if depth > 8 { return Err("isim_fcp_invalid"); }
    while !content.is_empty() {
        let constructed = content[0] & 0x20 != 0;
        let (_, value) = take_tlv(&mut content).map_err(|_| "isim_fcp_invalid")?;
        if constructed { validate_constructed(value, depth + 1)?; }
    }
    Ok(())
}

fn parse_fcp(data: &[u8], file: u16, records: bool) -> Result<Layout, &'static str> {
    if data.len() > MAX_FCP_BYTES { return Err("isim_fcp_invalid"); }
    let mut outer = data;
    let (tag, mut content) = take_tlv(&mut outer).map_err(|_| "isim_fcp_invalid")?;
    if tag != 0x62 || !outer.is_empty() { return Err("isim_fcp_invalid"); }
    validate_constructed(content, 0)?;
    let mut descriptor = None;
    let mut file_id = None;
    let mut size = None;
    while !content.is_empty() {
        let (tag, value) = take_tlv(&mut content).map_err(|_| "isim_fcp_invalid")?;
        let target = match tag {
            0x82 => &mut descriptor,
            0x83 => &mut file_id,
            0x80 => &mut size,
            // Other TS 102.221 objects (security attributes, lifecycle,
            // proprietary information, total size 81) are not EF data size.
            _ => continue,
        };
        if target.replace(value).is_some() { return Err("isim_fcp_invalid"); }
    }
    let descriptor = descriptor.ok_or("isim_fcp_invalid")?;
    if file_id != Some(file.to_be_bytes().as_slice()) { return Err("isim_fcp_invalid"); }
    let size = size.ok_or("isim_fcp_invalid")?;
    if size.len() != 2 { return Err("isim_fcp_invalid"); }
    let size = usize::from(u16::from_be_bytes([size[0], size[1]]));
    if size > MAX_EF_BYTES { return Err("isim_file_too_large"); }
    // TS 102.221 file descriptor: shareable bit is optional, working EF,
    // transparent=001 / linear fixed=010; data coding byte is 21.
    if descriptor.len() != if records { 5 } else { 2 }
        || descriptor[0] & !0x40 != if records { 0x02 } else { 0x01 }
        || descriptor[1] != 0x21
    {
        return Err("isim_fcp_invalid");
    }
    if !records { return Ok(Layout::Transparent { size }); }
    let length = usize::from(u16::from_be_bytes([descriptor[2], descriptor[3]]));
    let count = usize::from(descriptor[4]);
    if count > MAX_RECORDS { return Err("isim_record_limit"); }
    if length == 0 || count == 0 || length.checked_mul(count) != Some(size) {
        return Err("isim_fcp_invalid");
    }
    // READ RECORD uses a short Le (00 means 256); do not truncate a larger
    // FCP length or guess how to read it with an unsupported extended APDU.
    if length > 256 { return Err("isim_record_size_unsupported"); }
    Ok(Layout::Records { size, length, count })
}

/// One data object per transparent EF/record, followed only by FF padding.
/// An absent value is represented by an empty/all-FF file, not a bad TLV.
fn data_object(data: &[u8]) -> Result<Option<&[u8]>, &'static str> {
    if data.len() > MAX_EF_BYTES { return Err("isim_file_too_large"); }
    if data.iter().all(|b| *b == 0xff) { return Ok(None); }
    let mut rest = data;
    let (tag, value) = take_tlv(&mut rest)?;
    if tag != 0x80 || value.is_empty() || rest.iter().any(|b| *b != 0xff) {
        return Err("isim_tlv_invalid");
    }
    Ok(Some(value))
}

fn ascii(value: &[u8]) -> Result<&str, &'static str> {
    if value.is_empty() || value.len() > 512 || !value.iter().all(|b| (0x21..=0x7e).contains(b)) {
        return Err("isim_identity_invalid");
    }
    std::str::from_utf8(value).map_err(|_| "isim_identity_invalid")
}

fn fqdn(value: &str) -> bool {
    let value = value.strip_suffix('.').unwrap_or(value);
    !value.is_empty() && value.len() <= 253 && value.contains('.')
        && value.parse::<IpAddr>().is_err()
        && value.split('.').all(|label| {
            !label.is_empty() && label.len() <= 63
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
        })
        && value.rsplit('.').next().is_some_and(|label| label.bytes().any(|b| b.is_ascii_alphabetic()))
}

fn user_part(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~+!$&'()*=,".contains(&b))
}

fn sip_host(value: &str) -> bool {
    let valid_host = |host: &str| fqdn(host) || host.parse::<Ipv4Addr>().is_ok();
    if let Some(value) = value.strip_prefix('[') {
        let Some((host, rest)) = value.split_once(']') else { return false; };
        return host.parse::<Ipv6Addr>().is_ok()
            && (rest.is_empty() || rest.strip_prefix(':').is_some_and(valid_port));
    }
    if let Some((host, port)) = value.split_once(':') {
        valid_host(host) && valid_port(port)
    } else { valid_host(value) }
}

fn valid_port(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u16>().is_ok_and(|port| port != 0)
}

fn public_uri(value: &str) -> bool {
    // Safe supported RFC 3261 / RFC 3966 forms. Do not pass URI header
    // components, passwords, percent-encoded controls or arbitrary parameters
    // to registration. Unsupported URI extensions are rejected, not stripped.
    if let Some((scheme, rest)) = value.split_once(':') {
        if scheme.eq_ignore_ascii_case("sip") || scheme.eq_ignore_ascii_case("sips") {
            return rest.split_once('@').is_some_and(|(user, host)| user_part(user) && sip_host(host));
        }
        if scheme.eq_ignore_ascii_case("tel") {
            return rest.strip_prefix('+').is_some_and(|number| {
                !number.is_empty() && number.len() <= 15 && number.bytes().all(|b| b.is_ascii_digit())
            });
        }
    }
    false
}

pub fn parse_ef_impi(data: &[u8]) -> Result<Option<String>, &'static str> {
    let Some(value) = data_object(data)? else { return Ok(None); };
    let value = ascii(value)?;
    if !value.split_once('@').is_some_and(|(user, domain)| user_part(user) && fqdn(domain)) {
        return Err("isim_identity_invalid");
    }
    Ok(Some(value.to_owned()))
}

pub fn parse_ef_domain(data: &[u8]) -> Result<Option<String>, &'static str> {
    let Some(value) = data_object(data)? else { return Ok(None); };
    let value = ascii(value)?;
    if !fqdn(value) { return Err("isim_domain_invalid"); }
    Ok(Some(value.to_owned()))
}

pub fn parse_ef_impu_record(data: &[u8]) -> Result<Option<String>, &'static str> {
    let Some(value) = data_object(data)? else { return Ok(None); };
    let value = ascii(value)?;
    if !public_uri(value) { return Err("isim_identity_invalid"); }
    Ok(Some(value.to_owned()))
}

pub fn parse_ef_pcscf_record(data: &[u8]) -> Result<Option<String>, &'static str> {
    let Some(value) = data_object(data)? else { return Ok(None); };
    let address = match value {
        [0, text @ ..] => {
            let name = ascii(text).map_err(|_| "isim_pcscf_invalid")?;
            if !fqdn(name) { return Err("isim_pcscf_invalid"); }
            name.to_owned()
        }
        [1, a, b, c, d] => Ipv4Addr::new(*a, *b, *c, *d).to_string(),
        [2, bytes @ ..] if bytes.len() == 16 => {
            let octets: [u8; 16] = bytes.try_into().map_err(|_| "isim_pcscf_invalid")?;
            Ipv6Addr::from(octets).to_string()
        }
        _ => return Err("isim_pcscf_invalid"),
    };
    Ok(Some(address))
}

/// Validate public material constructed outside the EF reader as well.
pub fn validate_material(material: &IsimImsMaterial) -> Result<(), &'static str> {
    if let Some(value) = &material.impi {
        ascii(value.as_bytes())?;
        if !value.split_once('@').is_some_and(|(user, domain)| user_part(user) && fqdn(domain)) {
            return Err("isim_identity_invalid");
        }
    }
    if let Some(value) = &material.domain {
        ascii(value.as_bytes())?;
        if !fqdn(value) { return Err("isim_domain_invalid"); }
    }
    if material.impus.len() > MAX_RECORDS || material.pcscf.len() > MAX_RECORDS {
        return Err("isim_record_limit");
    }
    for value in &material.impus {
        ascii(value.as_bytes())?;
        if !public_uri(value) { return Err("isim_identity_invalid"); }
    }
    for value in &material.pcscf {
        if !fqdn(value) && value.parse::<IpAddr>().is_err() { return Err("isim_pcscf_invalid"); }
    }
    Ok(())
}

fn read_ef(
    exchange: &mut impl FnMut(&[u8]) -> Result<UimApduResponse, &'static str>,
    file: u16,
    records: bool,
) -> Result<Vec<Vec<u8>>, &'static str> {
    let [hi, lo] = file.to_be_bytes();
    let selected = exchange(&[0, 0xa4, 0, 4, 2, hi, lo, 0])?;
    // Only SELECT's explicit file-not-found is absence. A missing record,
    // access denial, read failure, or an unexpected SW remains an error.
    if (selected.sw1, selected.sw2) == (0x6a, 0x82) { return Ok(Vec::new()); }
    if (selected.sw1, selected.sw2) != (0x90, 0) { return Err("isim_select_failed"); }
    match parse_fcp(&selected.data, file, records)? {
        Layout::Transparent { size } => {
            let mut data = Vec::with_capacity(size);
            while data.len() < size {
                let offset = data.len();
                let length = (size - offset).min(256);
                let response = exchange(&[0, 0xb0, (offset >> 8) as u8, offset as u8, length as u8])?;
                validate_read(&response, length, offset + length == size)?;
                data.extend(response.data);
            }
            Ok(vec![data])
        }
        Layout::Records { length, count, .. } => {
            let mut result = Vec::with_capacity(count);
            for record in 1..=count {
                let response = exchange(&[0, 0xb2, record as u8, 4, length as u8])?;
                validate_read(&response, length, record == count)?;
                result.push(response.data);
            }
            Ok(result)
        }
    }
}

fn validate_read(response: &UimApduResponse, length: usize, last: bool) -> Result<(), &'static str> {
    let status = (response.sw1, response.sw2);
    if status != (0x90, 0) && !(last && status == (0x62, 0x82)) {
        return Err("isim_read_failed");
    }
    // EOF is legal only at the declared end, with all advertised bytes. Do
    // not silently turn a truncated/rejected read into an unconfigured field.
    if response.data.len() != length { return Err("isim_read_length_mismatch"); }
    Ok(())
}

/// Read all four optional EFs through the same selected ISIM channel.
/// Missing/FF siblings give a partial material; corrupt or unreadable files
/// return a stable reason, and adapter errors are preserved verbatim.
pub fn read_isim_material_with(
    mut exchange: impl FnMut(&[u8]) -> Result<UimApduResponse, &'static str>,
) -> Result<IsimImsMaterial, &'static str> {
    let mut result = IsimImsMaterial::default();
    for data in read_ef(&mut exchange, EF_IMPI, false)? {
        result.impi = parse_ef_impi(&data)?;
    }
    for data in read_ef(&mut exchange, EF_DOMAIN, false)? {
        result.domain = parse_ef_domain(&data)?;
    }
    for data in read_ef(&mut exchange, EF_IMPU, true)? {
        if let Some(value) = parse_ef_impu_record(&data)? {
            if !result.impus.contains(&value) { result.impus.push(value); }
        }
    }
    for data in read_ef(&mut exchange, EF_PCSCF, true)? {
        if let Some(value) = parse_ef_pcscf_record(&data)? {
            if !result.pcscf.contains(&value) { result.pcscf.push(value); }
        }
    }
    Ok(result)
}

/// Discover full application AIDs from MF/EF_DIR over an owned logical channel.
/// Only read-only SELECT/READ commands are issued; malformed directory data is
/// not interpreted as evidence that ISIM is absent.
pub fn read_application_aids_with(
    mut exchange: impl FnMut(&[u8]) -> Result<UimApduResponse, &'static str>,
) -> Result<Vec<Vec<u8>>, &'static str> {
    let mf = exchange(&[0, 0xa4, 0, 4, 2, 0x3f, 0, 0])?;
    if (mf.sw1, mf.sw2) != (0x90, 0) { return Err("isim_application_discovery_failed"); }
    let records = read_ef(&mut exchange, 0x2f00, true)?;
    if records.is_empty() { return Err("isim_application_discovery_failed"); }
    parse_application_directory(&records)
}

fn parse_application_directory(records: &[Vec<u8>]) -> Result<Vec<Vec<u8>>, &'static str> {
    let mut aids = Vec::new();
    for record in records {
        let mut rest = record.as_slice();
        while !rest.is_empty() && !rest.iter().all(|byte| *byte == 0xff) {
            let (tag, mut template) = take_tlv(&mut rest)?;
            if tag != 0x61 { return Err("isim_application_discovery_failed"); }
            let mut aid = None;
            while !template.is_empty() {
                let (tag, data) = take_tlv(&mut template)?;
                if tag == 0x4f {
                    if aid.is_some() || !(5..=16).contains(&data.len()) {
                        return Err("isim_application_discovery_failed");
                    }
                    aid = Some(data.to_vec());
                }
            }
            let aid = aid.ok_or("isim_application_discovery_failed")?;
            if !aids.contains(&aid) {
                if aids.len() >= MAX_RECORDS { return Err("isim_record_limit"); }
                aids.push(aid);
            }
        }
    }
    Ok(aids)
}

#[cfg(test)]
#[path = "qmi_uim_isim_tests.rs"]
mod tests;
