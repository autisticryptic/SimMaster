//! USB PC/SC SIM-reader device family through the OpenSC command-line client.
//!
//! Keeping this adapter process-based avoids linking the main static/musl
//! binary to libpcsclite. `pcscd`, a CCID driver and `opensc-tool` remain host
//! dependencies installed by the deployment script.

use std::{
    collections::HashMap,
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use serde::Serialize;

use crate::connectivity::modems::ims::vowifi::qmi_uim::{
    build_get_response_apdu, build_usim_authenticate_apdu, parse_ef_epdg_id,
    parse_ef_epdg_selection, parse_fcp_file_size, parse_usim_authenticate_response_reason,
    UimApduResponse, UsimAkaApduResult, UsimEpdgConfig, EF_EPDG_ID, EF_EPDG_SELECTION,
};

const COMMAND_TIMEOUT_SECS: u64 = 8;
const IDENTITY_CACHE_TTL: Duration = Duration::from_secs(5);
const MAX_OPTIONAL_EF_BYTES: usize = 4096;
const OPTIONAL_EF_READ_CHUNK_BYTES: usize = 255;
const USIM_AID_PREFIX: &[u8] = &[0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x02];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PcscReaderInfo {
    pub index: u16,
    pub name: String,
    pub card_present: bool,
    pub selector: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcscIdentity {
    pub iccid: String,
    pub imsi: String,
    pub mnc_length: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ApduResponse {
    data: Vec<u8>,
    sw1: u8,
    sw2: u8,
}

static IDENTITY_CACHE: OnceLock<Mutex<HashMap<String, (Instant, PcscIdentity)>>> = OnceLock::new();

fn identity_cache() -> &'static Mutex<HashMap<String, (Instant, PcscIdentity)>> {
    IDENTITY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn selector_from_path(path: &str) -> Option<String> {
    path.trim()
        .strip_prefix("pcsc://")
        .map(str::trim)
        .filter(|selector| !selector.is_empty())
        .map(str::to_string)
}

pub async fn discover_readers() -> Result<Vec<PcscReaderInfo>, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(COMMAND_TIMEOUT_SECS),
        tokio::process::Command::new("opensc-tool")
            .arg("--list-readers")
            .output(),
    )
    .await
    .map_err(|_| "pcsc_reader_discovery_timeout".to_string())?
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "pcsc_opensc_tool_missing".to_string()
        } else {
            "pcsc_reader_discovery_failed".to_string()
        }
    })?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let readers = parse_reader_list(&text);
    if !output.status.success() && readers.is_empty() {
        return Err("pcsc_service_unavailable".to_string());
    }
    if readers.is_empty() && usb_ccid_interface_present() {
        return Err("pcsc_ccid_driver_or_service_unavailable".to_string());
    }
    Ok(readers)
}

pub fn read_identity(path: &str) -> Result<PcscIdentity, &'static str> {
    let reader = resolve_reader_blocking(path)?;
    let select_usim = select_application_apdu(USIM_AID_PREFIX)?;
    let responses = run_apdus(
        reader.index,
        &[
            hex("00A40004023F0000")?,
            hex("00A40004022FE200")?,
            hex("00B000000A")?,
            select_usim.clone(),
            hex("00A40004026F0700")?,
            hex("00B0000009")?,
            select_usim,
            hex("00A40004026FAD00")?,
            hex("00B0000004")?,
        ],
    )?;
    if responses.len() != 9 {
        return Err("pcsc_apdu_response_count_invalid");
    }
    require_success(&responses[0])?;
    require_success(&responses[1])?;
    require_success(&responses[2])?;
    require_success(&responses[3])?;
    require_success(&responses[4])?;
    require_success(&responses[5])?;

    let iccid = decode_swapped_bcd(&responses[2].data, false)
        .filter(|value| (18..=22).contains(&value.len()))
        .ok_or("pcsc_iccid_invalid")?;
    let imsi = decode_imsi(&responses[5].data)?;
    let mnc_length = if require_success(&responses[6]).is_ok()
        && require_success(&responses[7]).is_ok()
        && require_success(&responses[8]).is_ok()
        && responses[8].data.len() >= 4
    {
        match responses[8].data[3] & 0x0f {
            2 => Some(2),
            3 => Some(3),
            _ => None,
        }
    } else {
        None
    };
    Ok(PcscIdentity {
        iccid,
        imsi,
        mnc_length,
    })
}

/// Read optional non-emergency ePDG configuration without coupling it to the
/// mandatory identity path. Each optional EF is selected and read independently,
/// so one absent, malformed or unsupported file cannot hide a usable sibling.
/// Transparent files are bounded to 4096 bytes and read in short-APDU chunks.
pub fn read_epdg_config(path: &str) -> Result<UsimEpdgConfig, &'static str> {
    let reader = resolve_reader_blocking(path)?;

    let home_identifiers = match read_optional_transparent_ef(reader.index, EF_EPDG_ID) {
        Ok(Some(data)) => match parse_ef_epdg_id(&data) {
            Ok(values) => values,
            Err(_) => {
                tracing::warn!(
                    reader = %reader.selector,
                    file_id = format_args!("{EF_EPDG_ID:04X}"),
                    "Ignoring malformed optional USIM ePDG identifier file"
                );
                Vec::new()
            }
        },
        Ok(None) => Vec::new(),
        Err(reason) => {
            tracing::warn!(
                reader = %reader.selector,
                file_id = format_args!("{EF_EPDG_ID:04X}"),
                reason,
                "Optional USIM ePDG identifier file could not be read"
            );
            Vec::new()
        }
    };
    let selection = match read_optional_transparent_ef(reader.index, EF_EPDG_SELECTION) {
        Ok(Some(data)) => match parse_ef_epdg_selection(&data) {
            Ok(values) => values,
            Err(_) => {
                tracing::warn!(
                    reader = %reader.selector,
                    file_id = format_args!("{EF_EPDG_SELECTION:04X}"),
                    "Ignoring malformed optional USIM ePDG selection file"
                );
                Vec::new()
            }
        },
        Ok(None) => Vec::new(),
        Err(reason) => {
            tracing::warn!(
                reader = %reader.selector,
                file_id = format_args!("{EF_EPDG_SELECTION:04X}"),
                reason,
                "Optional USIM ePDG selection file could not be read"
            );
            Vec::new()
        }
    };
    Ok(UsimEpdgConfig {
        home_identifiers,
        selection,
    })
}

fn read_optional_transparent_ef(
    reader_index: u16,
    file_id: u16,
) -> Result<Option<Vec<u8>>, &'static str> {
    let selected_responses = run_usim_file_apdus(reader_index, file_id, &[])?;
    let application = selected_responses
        .first()
        .ok_or("pcsc_apdu_response_count_invalid")?;
    require_success(application)?;
    let selected = selected_responses
        .get(1)
        .ok_or("pcsc_apdu_response_count_invalid")?;
    if matches!((selected.sw1, selected.sw2), (0x6a, 0x82 | 0x83)) {
        return Ok(None);
    }
    if !matches!(selected.sw1, 0x90 | 0x61 | 0x9f) {
        return Err("pcsc_optional_ef_select_rejected");
    }

    let mut fcp = selected.data.clone();
    if matches!(selected.sw1, 0x61 | 0x9f) {
        let get_response = build_get_response_apdu(selected.sw2);
        if let Ok(responses) = run_usim_file_apdus(reader_index, file_id, &[get_response]) {
            if let Some(response) = responses.get(2) {
                if (response.sw1, response.sw2) == (0x90, 0x00) {
                    fcp = response.data.clone();
                }
            }
        }
    }
    let file_size = parse_fcp_file_size(&fcp);
    read_optional_ef_contents(file_size, |offset, requested| {
        read_transparent_ef_chunk(reader_index, file_id, offset, requested)
    })
    .map(Some)
}

/// Read one already-selected transparent optional EF through a transport
/// callback. Keeping the bounded/chunked state machine independent from
/// `opensc-tool` lets us verify EOF and corrected-length handling without a
/// physical reader in the test environment.
fn read_optional_ef_contents<F>(
    file_size: Option<usize>,
    mut read_chunk: F,
) -> Result<Vec<u8>, &'static str>
where
    F: FnMut(usize, usize) -> Result<ApduResponse, &'static str>,
{
    if file_size.is_some_and(|size| size > MAX_OPTIONAL_EF_BYTES) {
        return Err("pcsc_optional_ef_too_large");
    }

    let mut data = Vec::new();
    loop {
        if let Some(size) = file_size {
            if data.len() >= size {
                data.truncate(size);
                return Ok(data);
            }
        }
        if data.len() >= MAX_OPTIONAL_EF_BYTES || data.len() > 0x7fff {
            return Err("pcsc_optional_ef_too_large");
        }
        let requested = file_size
            .map(|size| size.saturating_sub(data.len()))
            .unwrap_or(OPTIONAL_EF_READ_CHUNK_BYTES)
            .min(OPTIONAL_EF_READ_CHUNK_BYTES);
        if requested == 0 {
            return Ok(data);
        }

        let offset = data.len();
        let mut response = read_chunk(offset, requested)?;
        let mut effective_requested = requested;
        if response.sw1 == 0x6c {
            effective_requested = decode_short_apdu_le(response.sw2);
            response = read_chunk(offset, effective_requested)?;
        }
        let eof = (response.sw1, response.sw2) == (0x62, 0x82);
        if (response.sw1, response.sw2) != (0x90, 0x00) && !eof {
            if file_size.is_none()
                && !data.is_empty()
                && matches!((response.sw1, response.sw2), (0x6b, 0x00) | (0x6a, 0x86))
            {
                return Ok(data);
            }
            return Err("pcsc_optional_ef_read_rejected");
        }
        let read_len = response.data.len();
        if data.len().saturating_add(read_len) > MAX_OPTIONAL_EF_BYTES {
            return Err("pcsc_optional_ef_too_large");
        }
        data.extend_from_slice(&response.data);
        if eof || read_len < effective_requested || read_len == 0 {
            if let Some(size) = file_size {
                if data.len() < size {
                    return Err("pcsc_optional_ef_truncated");
                }
                data.truncate(size);
            }
            return Ok(data);
        }
    }
}

fn decode_short_apdu_le(le: u8) -> usize {
    if le == 0 {
        256
    } else {
        usize::from(le)
    }
}

fn run_usim_file_apdus(
    reader_index: u16,
    file_id: u16,
    trailing: &[Vec<u8>],
) -> Result<Vec<ApduResponse>, &'static str> {
    let [file_high, file_low] = file_id.to_be_bytes();
    let mut apdus = vec![
        select_application_apdu(USIM_AID_PREFIX)?,
        vec![0x00, 0xa4, 0x00, 0x04, 0x02, file_high, file_low, 0x00],
    ];
    apdus.extend_from_slice(trailing);
    run_apdus(reader_index, &apdus)
}

fn build_read_binary_apdu(offset: usize, le: usize) -> Result<Vec<u8>, &'static str> {
    if offset > 0x7fff {
        return Err("pcsc_optional_ef_offset_invalid");
    }
    if !(1..=256).contains(&le) {
        return Err("pcsc_optional_ef_length_invalid");
    }
    Ok(vec![
        0x00,
        0xb0,
        ((offset >> 8) & 0x7f) as u8,
        (offset & 0xff) as u8,
        if le == 256 { 0 } else { le as u8 },
    ])
}

fn read_transparent_ef_chunk(
    reader_index: u16,
    file_id: u16,
    offset: usize,
    le: usize,
) -> Result<ApduResponse, &'static str> {
    let read = build_read_binary_apdu(offset, le)?;
    let responses = run_usim_file_apdus(reader_index, file_id, &[read])?;
    require_success(
        responses
            .first()
            .ok_or("pcsc_apdu_response_count_invalid")?,
    )?;
    let selected = responses.get(1).ok_or("pcsc_apdu_response_count_invalid")?;
    if !matches!(selected.sw1, 0x90 | 0x61 | 0x9f) {
        return Err("pcsc_optional_ef_select_rejected");
    }
    responses
        .get(2)
        .cloned()
        .ok_or("pcsc_apdu_response_count_invalid")
}

/// Read a PC/SC card identity without blocking the async runtime. OpenSC is a
/// synchronous process adapter, so reader discovery and APDU traffic must run
/// on the blocking pool when they are used by the line registry and APIs.
pub async fn read_identity_async(path: &str) -> Result<PcscIdentity, String> {
    let path = path.trim().to_string();
    if selector_from_path(&path).is_none() {
        return Err("pcsc_reader_selector_invalid".to_string());
    }
    if let Some(identity) = identity_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&path)
        .filter(|(captured_at, _)| captured_at.elapsed() <= IDENTITY_CACHE_TTL)
        .map(|(_, identity)| identity.clone())
    {
        return Ok(identity);
    }
    let cache_key = path.clone();
    let identity = tokio::time::timeout(
        Duration::from_secs(COMMAND_TIMEOUT_SECS + 2),
        tokio::task::spawn_blocking(move || read_identity(&path)),
    )
    .await
    .map_err(|_| "pcsc_identity_timeout".to_string())?
    .map_err(|_| "pcsc_identity_task_failed".to_string())?
    .map_err(str::to_string)?;
    identity_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(cache_key, (Instant::now(), identity.clone()));
    Ok(identity)
}

pub fn verify_usim(path: &str) -> Result<(), &'static str> {
    let reader = resolve_reader_blocking(path)?;
    let responses = run_apdus(reader.index, &[select_application_apdu(USIM_AID_PREFIX)?])?;
    responses
        .first()
        .ok_or("pcsc_apdu_response_missing")
        .and_then(require_success)
}

pub fn authenticate(
    path: &str,
    rand: &[u8],
    autn: &[u8],
) -> Result<UsimAkaApduResult, &'static str> {
    let reader = resolve_reader_blocking(path)?;
    let authenticate =
        build_usim_authenticate_apdu(rand, autn).map_err(|_| "pcsc_aka_apdu_build_failed")?;
    let responses = run_apdus(
        reader.index,
        &[select_application_apdu(USIM_AID_PREFIX)?, authenticate],
    )?;
    let select = responses.first().ok_or("pcsc_apdu_response_missing")?;
    require_success(select)?;
    let response = responses.get(1).ok_or("pcsc_apdu_response_missing")?;
    parse_usim_authenticate_response_reason(&UimApduResponse {
        data: response.data.clone(),
        sw1: response.sw1,
        sw2: response.sw2,
    })
}

fn resolve_reader_blocking(path: &str) -> Result<PcscReaderInfo, &'static str> {
    let selector = selector_from_path(path).ok_or("pcsc_reader_selector_invalid")?;
    let output = run_opensc(&["--list-readers".to_string()])?;
    let readers = parse_reader_list(&output);
    resolve_reader(&readers, &selector)
        .cloned()
        .ok_or("pcsc_reader_not_found")
        .and_then(|reader| {
            if reader.card_present {
                Ok(reader)
            } else {
                Err("pcsc_card_not_present")
            }
        })
}

fn resolve_reader<'a>(readers: &'a [PcscReaderInfo], selector: &str) -> Option<&'a PcscReaderInfo> {
    if let Ok(index) = selector.parse::<u16>() {
        return readers.iter().find(|reader| reader.index == index);
    }
    readers.iter().find(|reader| reader.name == selector)
}

fn run_apdus(reader_index: u16, apdus: &[Vec<u8>]) -> Result<Vec<ApduResponse>, &'static str> {
    let mut args = vec![
        "--reader".to_string(),
        reader_index.to_string(),
        "--card-driver".to_string(),
        "default".to_string(),
    ];
    for apdu in apdus {
        args.push("--send-apdu".to_string());
        args.push(encode_hex(apdu));
    }
    parse_apdu_responses(&run_opensc(&args)?)
}

fn run_opensc(args: &[String]) -> Result<String, &'static str> {
    #[cfg(unix)]
    let output = Command::new("timeout")
        .arg(format!("{COMMAND_TIMEOUT_SECS}s"))
        .arg("opensc-tool")
        .args(args)
        .output();

    #[cfg(not(unix))]
    let output = Command::new("opensc-tool").args(args).output();

    let output = output.map_err(|_| "pcsc_opensc_tool_missing")?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        Ok(text)
    } else if output.status.code() == Some(124) {
        Err("pcsc_command_timeout")
    } else if text.to_ascii_lowercase().contains("no smart card readers") {
        Err("pcsc_reader_not_found")
    } else {
        Err("pcsc_command_failed")
    }
}

fn parse_reader_list(output: &str) -> Vec<PcscReaderInfo> {
    let mut readers = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        let mut parts = line.split_whitespace();
        let Some(index) = parts.next().and_then(|value| value.parse::<u16>().ok()) else {
            continue;
        };
        let Some(card) = parts.next() else {
            continue;
        };
        if !matches!(card.to_ascii_lowercase().as_str(), "yes" | "no") {
            continue;
        }
        let mut name_parts = parts.collect::<Vec<_>>();
        if name_parts.len() >= 2
            && name_parts[0].eq_ignore_ascii_case("pin")
            && name_parts[1].eq_ignore_ascii_case("pad")
        {
            name_parts.drain(..2);
        }
        let name = name_parts.join(" ").trim().to_string();
        if name.is_empty() {
            continue;
        }
        readers.push(PcscReaderInfo {
            index,
            selector: format!("pcsc://{name}"),
            name,
            card_present: card.eq_ignore_ascii_case("yes"),
        });
    }
    readers
}

fn parse_apdu_responses(output: &str) -> Result<Vec<ApduResponse>, &'static str> {
    let lines = output.lines().collect::<Vec<_>>();
    let mut responses = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        if !line.starts_with("Received") {
            index += 1;
            continue;
        }
        let sw1 = parse_status_octet(line, "SW1=0x").ok_or("pcsc_status_parse_failed")?;
        let sw2 = parse_status_octet(line, "SW2=0x").ok_or("pcsc_status_parse_failed")?;
        let inline = line
            .split_once(": ")
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty());
        let mut data = inline.map(hex).transpose()?.unwrap_or_default();
        let mut consumed = 1;
        if inline.is_none() {
            while let Some(next) = lines.get(index + consumed) {
                let next = next.trim();
                if next.starts_with("Sending:") || next.starts_with("Received") {
                    break;
                }
                let dump = parse_opensc_hex_dump_line(next);
                if dump.is_empty() {
                    break;
                }
                data.extend(dump);
                consumed += 1;
            }
        }
        responses.push(ApduResponse { data, sw1, sw2 });
        index += consumed;
    }
    if responses.is_empty() {
        Err("pcsc_apdu_response_missing")
    } else {
        Ok(responses)
    }
}

fn parse_opensc_hex_dump_line(line: &str) -> Vec<u8> {
    let bytes = line.as_bytes();
    let mut parsed = Vec::new();
    let mut offset = 0;
    while parsed.len() < 16 && offset + 2 < bytes.len() {
        if !bytes[offset].is_ascii_hexdigit()
            || !bytes[offset + 1].is_ascii_hexdigit()
            || bytes[offset + 2] != b' '
        {
            break;
        }
        let Ok(value) = u8::from_str_radix(&line[offset..offset + 2], 16) else {
            break;
        };
        parsed.push(value);
        offset += 3;
    }
    parsed
}

#[cfg(target_os = "linux")]
fn usb_ccid_interface_present() -> bool {
    std::fs::read_dir("/sys/bus/usb/devices")
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            std::fs::read_to_string(entry.path().join("bInterfaceClass"))
                .is_ok_and(|value| value.trim().eq_ignore_ascii_case("0b"))
        })
}

#[cfg(not(target_os = "linux"))]
fn usb_ccid_interface_present() -> bool {
    false
}

fn parse_status_octet(line: &str, marker: &str) -> Option<u8> {
    let tail = line.split_once(marker)?.1;
    u8::from_str_radix(tail.get(..2)?, 16).ok()
}

fn select_application_apdu(aid: &[u8]) -> Result<Vec<u8>, &'static str> {
    if aid.is_empty() || aid.len() > u8::MAX as usize {
        return Err("pcsc_usim_aid_invalid");
    }
    let mut apdu = vec![0x00, 0xa4, 0x04, 0x04, aid.len() as u8];
    apdu.extend_from_slice(aid);
    apdu.push(0x00);
    Ok(apdu)
}

fn require_success(response: &ApduResponse) -> Result<(), &'static str> {
    match (response.sw1, response.sw2) {
        (0x90, 0x00) => Ok(()),
        (0x69, 0x82) | (0x98, 0x04) => Err("pcsc_sim_pin_required"),
        (0x6a, 0x82) => Err("pcsc_usim_application_missing"),
        _ => Err("pcsc_apdu_rejected"),
    }
}

fn decode_imsi(data: &[u8]) -> Result<String, &'static str> {
    let length = data.first().copied().ok_or("pcsc_imsi_invalid")? as usize;
    if length == 0 || data.len() < 1 + length {
        return Err("pcsc_imsi_invalid");
    }
    let imsi = decode_swapped_bcd(&data[1..1 + length], true).ok_or("pcsc_imsi_invalid")?;
    if (10..=18).contains(&imsi.len()) {
        Ok(imsi)
    } else {
        Err("pcsc_imsi_invalid")
    }
}

fn decode_swapped_bcd(data: &[u8], mut drop_first_nibble: bool) -> Option<String> {
    let mut result = String::new();
    for byte in data {
        for nibble in [byte & 0x0f, byte >> 4] {
            if drop_first_nibble {
                drop_first_nibble = false;
                continue;
            }
            if nibble == 0x0f {
                return Some(result);
            }
            if nibble > 9 {
                return None;
            }
            result.push(char::from(b'0' + nibble));
        }
    }
    Some(result)
}

fn encode_hex(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn is_hex_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() % 2 == 0
        && value.chars().all(|character| character.is_ascii_hexdigit())
}

fn hex(value: &str) -> Result<Vec<u8>, &'static str> {
    let value = value.trim();
    if !is_hex_text(value) {
        return Err("pcsc_hex_invalid");
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16).map_err(|_| "pcsc_hex_invalid")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_opensc_reader_inventory() {
        let readers = parse_reader_list(
            "# Detected readers (pcsc)\nNr.  Card  Features  Name\n0    Yes   PIN pad   ACS ACR38U 00 00\n1    No              Generic CCID 00 00\n",
        );
        assert_eq!(readers.len(), 2);
        assert_eq!(readers[0].selector, "pcsc://ACS ACR38U 00 00");
        assert!(readers[0].card_present);
        assert!(!readers[1].card_present);
    }

    #[test]
    fn parses_apdu_output_with_inline_and_following_data() {
        let responses = parse_apdu_responses(
            "Received (SW1=0x90, SW2=0x00): 981234\nSending: 00 B0 00 00 09\nReceived (SW1=0x90, SW2=0x00):\n08 19 32 54 76 98 10 32 54 .2Tv..2T\n",
        )
        .unwrap();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0].data, vec![0x98, 0x12, 0x34]);
        assert_eq!(responses[1].data.len(), 9);
    }

    #[test]
    fn decodes_standard_sim_identifiers() {
        assert_eq!(
            decode_swapped_bcd(&hex("986800214365870921F3").unwrap(), false).unwrap(),
            "8986001234567890123"
        );
        assert_eq!(
            decode_imsi(&hex("081932547698103254").unwrap()).unwrap(),
            "123456789012345"
        );
    }

    #[test]
    fn reader_paths_are_explicit_and_never_implicit() {
        assert_eq!(
            selector_from_path("pcsc://ACS ACR38U 00 00").as_deref(),
            Some("ACS ACR38U 00 00")
        );
        assert!(selector_from_path("/dev/cdc-wdm0").is_none());
        assert!(selector_from_path("pcsc://").is_none());
    }

    #[test]
    fn optional_ef_fcp_size_accepts_standard_80_and_81_tags() {
        assert_eq!(
            parse_fcp_file_size(&[0x62, 0x04, 0x80, 0x02, 0x01, 0x2c]),
            Some(300)
        );
        assert_eq!(
            parse_fcp_file_size(&[0x62, 0x05, 0x81, 0x03, 0x00, 0x10, 0x00]),
            Some(MAX_OPTIONAL_EF_BYTES)
        );
    }

    #[test]
    fn read_binary_apdu_encodes_offset_and_short_apdu_lengths() {
        assert_eq!(
            build_read_binary_apdu(0x1234, 255).unwrap(),
            vec![0x00, 0xb0, 0x12, 0x34, 0xff]
        );
        assert_eq!(
            build_read_binary_apdu(0x7fff, 256).unwrap(),
            vec![0x00, 0xb0, 0x7f, 0xff, 0x00]
        );
        assert_eq!(decode_short_apdu_le(0), 256);
        assert_eq!(decode_short_apdu_le(17), 17);
        assert_eq!(
            build_read_binary_apdu(0x8000, 1),
            Err("pcsc_optional_ef_offset_invalid")
        );
        assert_eq!(
            build_read_binary_apdu(0, 0),
            Err("pcsc_optional_ef_length_invalid")
        );
        assert_eq!(
            build_read_binary_apdu(0, 257),
            Err("pcsc_optional_ef_length_invalid")
        );
    }

    #[test]
    fn optional_ef_reader_honors_6c00_and_6282_eof() {
        let mut calls = Vec::new();
        let mut round = 0usize;
        let data = read_optional_ef_contents(None, |offset, requested| {
            calls.push((offset, requested));
            round += 1;
            Ok(match round {
                1 => ApduResponse {
                    data: Vec::new(),
                    sw1: 0x6c,
                    sw2: 0x00,
                },
                2 => ApduResponse {
                    data: vec![0xa5; 256],
                    sw1: 0x62,
                    sw2: 0x82,
                },
                _ => panic!("unexpected extra READ BINARY"),
            })
        })
        .expect("corrected read ending at EOF");

        assert_eq!(calls, vec![(0, 255), (0, 256)]);
        assert_eq!(data, vec![0xa5; 256]);
    }

    #[test]
    fn optional_ef_reader_enforces_the_4096_byte_boundary() {
        assert_eq!(
            read_optional_ef_contents(Some(MAX_OPTIONAL_EF_BYTES + 1), |_, _| {
                panic!("oversized FCP must fail before any READ BINARY")
            }),
            Err("pcsc_optional_ef_too_large")
        );

        let exact = read_optional_ef_contents(Some(MAX_OPTIONAL_EF_BYTES), |_, requested| {
            Ok(ApduResponse {
                data: vec![0x5a; requested],
                sw1: 0x90,
                sw2: 0x00,
            })
        })
        .expect("4096-byte EF remains within the safety bound");
        assert_eq!(exact.len(), MAX_OPTIONAL_EF_BYTES);

        let mut reads = 0usize;
        let oversized = read_optional_ef_contents(None, |_, _| {
            reads += 1;
            Ok(ApduResponse {
                data: vec![0x33; if reads <= 16 { 255 } else { 17 }],
                sw1: 0x90,
                sw2: 0x00,
            })
        });
        assert_eq!(oversized, Err("pcsc_optional_ef_too_large"));
        assert_eq!(reads, 17);
    }

    #[test]
    fn optional_ef_reader_reports_known_size_truncation_at_eof() {
        let result = read_optional_ef_contents(Some(3), |_, _| {
            Ok(ApduResponse {
                data: vec![0x01, 0x02],
                sw1: 0x62,
                sw2: 0x82,
            })
        });
        assert_eq!(result, Err("pcsc_optional_ef_truncated"));
    }
}
