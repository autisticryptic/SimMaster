//! Read-only WDS Get Current Settings on an already-owned, retained client.
//!
//! Wire contract: libqmi 1.36.0 data/qmi-service-wds.json, SHA-256
//! 62fdd54826eb48b5f36ba97448948cf699998a5ed89cb582748532d6fce0e721.
//! Request bits: src/libqmi-glib/qmi-enums-wds.h at the same tag, SHA-256
//! 2038edba06e5aa61d46c076008106360316e69352bed5cdebc74382381013635.
//! Both are under https://raw.githubusercontent.com/linux-mobile-broadband/libqmi/1.36.0/.
//! qmicli-wds.c (SHA-256 694abebc6ff7507125ee9a07828da19226083c4f5d5092c84e346e551d588590)
//! neither requests nor prints these lists. Generic Domains (0x2a) are NOT P-CSCF.
//! Result codes: src/libqmi-glib/qmi-errors.h at the same tag, SHA-256
//! 20338363406547f675c512f59640d8f2a70a20bd6d0dff63f17a2069854f9137.
//!
//! This adapter does not allocate/release a client, bind a data port, activate a
//! bearer, or change a profile/PCO policy. Dropping its local socket is its only
//! cleanup. The caller holds the device operation lock and proves CID ownership;
//! SystemNativeIo fences the exact configured endpoint and persistent proxy lease.

use std::net::{IpAddr, Ipv4Addr};

use super::NativeError;
use crate::connectivity::modems::ims::vowifi::qmi_uim::{
    decode_qmi_frame, encode_qmi_message, QmiMessage, QmiTlv,
};

/// Observed only on the requested retained WDS client. Empty lists are valid
/// absence, not proof that the modem supports a requested discovery policy.
/// The pinned 0x23 schema is IPv4-only; no IPv6 address TLV is invented here.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct RetainedWdsSettings {
    pub addresses: Vec<IpAddr>,
    pub domains: Vec<String>,
}

impl std::fmt::Debug for RetainedWdsSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetainedWdsSettings")
            .field("address_count", &self.addresses.len())
            .field("domain_count", &self.domains.len())
            .finish()
    }
}

const WDS: u8 = 1;
const GET_CURRENT_SETTINGS: u16 = 0x002d;
const PROXY_OPEN: u16 = 0xff00;
const REQUEST_MASK: u32 = (1 << 11) | (1 << 12);
const MAX_FRAME_BYTES: usize = 8192;
const MAX_TLVS: usize = 64;
const MAX_LIST_ITEMS: usize = 16;
#[cfg(unix)]
const MAX_MESSAGES: usize = 32;

fn invalid() -> NativeError {
    NativeError::Protocol("native_wds_pcscf_response_invalid".into())
}

pub(super) fn timeout_error() -> NativeError {
    NativeError::CommandFailed("native_wds_pcscf_timeout")
}

fn request(client_id: u8, transaction_id: u16) -> Result<QmiMessage, NativeError> {
    // Zero is unallocated; 255 is the broadcast indication CID.
    if matches!(client_id, 0 | 255) || transaction_id == 0 {
        return Err(NativeError::Protocol("native_wds_pcscf_client_invalid".into()));
    }
    Ok(QmiMessage {
        service: WDS,
        client_id,
        transaction_id,
        message_id: GET_CURRENT_SETTINGS,
        tlvs: vec![QmiTlv {
            tlv_type: 0x10,
            value: REQUEST_MASK.to_le_bytes().to_vec(),
        }],
    })
}

/// Inbound QMUX only; compound messages and reserved flags are not admitted.
/// Duplicate TLVs (including unknown ones) are ambiguous and fail closed.
fn decode_inbound(frame: &[u8]) -> Result<(QmiMessage, bool), NativeError> {
    if !(12..=MAX_FRAME_BYTES).contains(&frame.len()) || frame[3] != 0x80 {
        return Err(invalid());
    }
    let message = decode_qmi_frame(frame).map_err(|_| invalid())?;
    let response_flag = if message.service == 0 { 1 } else { 2 };
    let is_response = frame[6] == response_flag;
    if !is_response && frame[6] != response_flag * 2 {
        return Err(invalid());
    }
    if message.tlvs.len() > MAX_TLVS {
        return Err(invalid());
    }
    let mut seen = [false; 256];
    for field in &message.tlvs {
        if std::mem::replace(&mut seen[usize::from(field.tlv_type)], true) {
            return Err(invalid());
        }
    }
    if is_response {
        // Even unrelated responses must be structurally valid before ignoring.
        result_code(&message)?;
    }
    Ok((message, is_response))
}

fn result_code(message: &QmiMessage) -> Result<u16, NativeError> {
    let value = message.tlvs.iter().find(|t| t.tlv_type == 2)
        .map(|t| t.value.as_slice()).ok_or_else(invalid)?;
    if value.len() != 4 {
        return Err(invalid());
    }
    let status = u16::from_le_bytes([value[0], value[1]]);
    let code = u16::from_le_bytes([value[2], value[3]]);
    match (status, code) {
        (0, 0) => Ok(0),
        (1, 1..=u16::MAX) => Ok(code),
        _ => Err(invalid()),
    }
}

fn usable_address(address: Ipv4Addr) -> bool {
    !address.is_unspecified()
        && !address.is_loopback()
        && !address.is_link_local()
        && !address.is_multicast()
        && !address.is_broadcast()
        && address.octets()[0] != 0
        && address.octets()[0] < 240
}

fn pcscf_name(raw: &[u8]) -> Result<String, NativeError> {
    if raw.is_empty() || raw.len() > 254 || !raw.is_ascii() {
        return Err(invalid());
    }
    let name = std::str::from_utf8(raw).map_err(|_| invalid())?;
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.len() > 253 || !name.contains('.') || name.parse::<IpAddr>().is_ok() {
        return Err(invalid());
    }
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63
            || !label.as_bytes()[0].is_ascii_alphanumeric()
            || !label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
            || !label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(invalid());
        }
    }
    // A numeric final label can be interpreted as an IPv4 literal by resolvers.
    if name.rsplit('.').next().is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit())) {
        return Err(invalid());
    }
    Ok(name.to_ascii_lowercase())
}

fn settings(message: &QmiMessage) -> Result<RetainedWdsSettings, NativeError> {
    match result_code(message)? {
        0 => {}
        // Public QmiProtocolError values, not heuristics on helper output.
        25 | 71 | 94 => return Err(NativeError::Unsupported("native_wds_pcscf_unsupported")),
        code => return Err(NativeError::ProtocolRejected(code)),
    }
    let mut result = RetainedWdsSettings::default();
    for field in &message.tlvs {
        match field.tlv_type {
            0x23 => {
                let (&count, bytes) = field.value.split_first().ok_or_else(invalid)?;
                if usize::from(count) > MAX_LIST_ITEMS || bytes.len() != usize::from(count) * 4 {
                    return Err(invalid());
                }
                for bytes in bytes.chunks_exact(4) {
                    let address = Ipv4Addr::from(u32::from_le_bytes(bytes.try_into().map_err(|_| invalid())?));
                    let ip = IpAddr::V4(address);
                    if !usable_address(address) || result.addresses.contains(&ip) {
                        return Err(invalid());
                    }
                    result.addresses.push(ip);
                }
            }
            0x24 => {
                let (&count, mut bytes) = field.value.split_first().ok_or_else(invalid)?;
                if usize::from(count) > MAX_LIST_ITEMS {
                    return Err(invalid());
                }
                for _ in 0..count {
                    if bytes.len() < 2 {
                        return Err(invalid());
                    }
                    let length = usize::from(u16::from_le_bytes([bytes[0], bytes[1]]));
                    bytes = &bytes[2..];
                    let name = pcscf_name(bytes.get(..length).ok_or_else(invalid)?)?;
                    if result.domains.contains(&name) {
                        return Err(invalid());
                    }
                    result.domains.push(name);
                    bytes = &bytes[length..];
                }
                if !bytes.is_empty() {
                    return Err(invalid());
                }
            }
            _ => {} // In particular 0x2a is NOT a P-CSCF domain list.
        }
    }
    Ok(result)
}

#[cfg(unix)]
fn io_error(_: std::io::Error) -> NativeError {
    NativeError::CommandFailed("native_wds_pcscf_proxy_io_failed")
}

#[cfg(unix)]
async fn read_frame(stream: &mut tokio::net::UnixStream) -> Result<Vec<u8>, NativeError> {
    use tokio::io::AsyncReadExt;
    let mut header = [0; 3];
    stream.read_exact(&mut header).await.map_err(io_error)?;
    let length = usize::from(u16::from_le_bytes([header[1], header[2]])) + 1;
    if header[0] != 1 || !(12..=MAX_FRAME_BYTES).contains(&length) {
        return Err(invalid());
    }
    let mut frame = vec![0; length];
    frame[..3].copy_from_slice(&header);
    stream.read_exact(&mut frame[3..]).await.map_err(io_error)?;
    Ok(frame)
}

#[cfg(unix)]
async fn correlated_response(
    stream: &mut tokio::net::UnixStream,
    request: &QmiMessage,
    remaining_messages: &mut usize,
) -> Result<QmiMessage, NativeError> {
    while *remaining_messages > 0 {
        *remaining_messages -= 1;
        let (response, is_response) = decode_inbound(&read_frame(stream).await?)?;
        if is_response
            && response.service == request.service
            && response.client_id == request.client_id
            && response.transaction_id == request.transaction_id
            && response.message_id == request.message_id
        {
            return Ok(response);
        }
        // Valid unsolicited/unrelated traffic does not extend either budget.
    }
    Err(NativeError::CommandFailed("native_wds_pcscf_message_limit"))
}

/// Real stream exchange, independently testable without claiming a device.
/// The one absolute deadline includes open acknowledgement, writes, unrelated
/// messages, and all partial reads. No per-frame or per-byte timeout resets.
#[cfg(unix)]
async fn exchange(
    mut stream: tokio::net::UnixStream,
    device: &str,
    client_id: u8,
    transaction_id: u16,
    deadline: tokio::time::Instant,
) -> Result<RetainedWdsSettings, NativeError> {
    use tokio::io::AsyncWriteExt;
    use crate::connectivity::modems::ims::vowifi::qmi_uim::build_proxy_open_frame;
    if !super::config::valid_device_path(device) {
        return Err(NativeError::Protocol("native_wds_pcscf_device_invalid".into()));
    }
    let query = request(client_id, transaction_id)?;
    if tokio::time::Instant::now() >= deadline {
        return Err(timeout_error());
    }
    tokio::time::timeout_at(deadline, async {
        let mut remaining_messages = MAX_MESSAGES;
        // Identical logical-open framing to QmiProxyLease; never its claim-time
        // helper bootstrap or the allocating native_management_exchange.
        let open_frame = build_proxy_open_frame(device, 1).map_err(|_| invalid())?;
        let open = decode_qmi_frame(&open_frame).map_err(|_| invalid())?;
        if (open.service, open.client_id, open.transaction_id, open.message_id) != (0, 0, 1, PROXY_OPEN) {
            return Err(invalid());
        }
        stream.write_all(&open_frame).await.map_err(io_error)?;
        let opened = correlated_response(&mut stream, &open, &mut remaining_messages).await?;
        let code = result_code(&opened)?;
        if code != 0 {
            return Err(NativeError::ProtocolRejected(code));
        }
        let frame = encode_qmi_message(&query).map_err(|_| invalid())?;
        stream.write_all(&frame).await.map_err(io_error)?;
        let response = correlated_response(&mut stream, &query, &mut remaining_messages).await?;
        let result = settings(&response)?;
        if tokio::time::Instant::now() >= deadline {
            return Err(timeout_error());
        }
        Ok(result)
    }).await.map_err(|_| timeout_error())?
}

/// Only SystemNativeIo may call this, after verifying its live proxy lease.
#[cfg(unix)]
pub(super) async fn read_retained(
    device: &str,
    client_id: u8,
    deadline: tokio::time::Instant,
) -> Result<RetainedWdsSettings, NativeError> {
    #[cfg(target_os = "linux")]
    {
        use std::sync::atomic::{AtomicU16, Ordering};
        static NEXT_TRANSACTION: AtomicU16 = AtomicU16::new(1);
        let transaction_id = NEXT_TRANSACTION.fetch_update(Ordering::Relaxed, Ordering::Relaxed,
            |id| Some(if id == u16::MAX { 1 } else { id + 1 })).expect("infallible update");
        request(client_id, transaction_id)?;
        if tokio::time::Instant::now() >= deadline {
            return Err(timeout_error());
        }
        // Tokio recognizes the leading NUL as a Linux abstract Unix address.
        // Unlike std::UnixStream::connect_addr, the connect is nonblocking and
        // shares the query deadline. Never start/restart a missing proxy here.
        tokio::time::timeout_at(deadline, async {
            let stream = tokio::net::UnixStream::connect("\0qmi-proxy").await.map_err(io_error)?;
            exchange(stream, device, client_id, transaction_id, deadline).await
        }).await.map_err(|_| timeout_error())?
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (device, client_id, deadline);
        Err(NativeError::Unsupported("native_backend_requires_linux"))
    }
}

#[cfg(test)]
#[path = "wds_pcscf_tests.rs"]
mod tests;
