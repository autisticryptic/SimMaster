#![allow(dead_code)]

use std::{
    fmt, io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::Duration,
};

#[cfg(unix)]
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
};

const QMUX_CTL_SERVICE: u8 = 0x00;
const QMUX_UIM_SERVICE: u8 = 0x0b;
const QMI_CTL_ALLOCATE_CID: u16 = 0x0022;
const QMI_CTL_RELEASE_CID: u16 = 0x0023;
const QMI_PROXY_OPEN: u16 = 0xff00;
const QMI_UIM_SEND_APDU: u16 = 0x003b;
const QMI_UIM_OPEN_LOGICAL_CHANNEL: u16 = 0x0042;
const QMI_UIM_LOGICAL_CHANNEL: u16 = 0x003f;

const TLV_RESULT: u8 = 0x02;
const TLV_PROXY_DEVICE_PATH: u8 = 0x01;
const TLV_CTL_SERVICE: u8 = 0x01;
const TLV_CTL_ALLOCATION_INFO: u8 = 0x01;
const TLV_UIM_SLOT: u8 = 0x01;
const TLV_UIM_APDU: u8 = 0x02;
const TLV_UIM_CHANNEL_ID: u8 = 0x10;
const TLV_UIM_PROCEDURE_BYTES: u8 = 0x11;
const TLV_UIM_OPEN_AID: u8 = 0x10;
const TLV_UIM_OPEN_FCI: u8 = 0x11;
const TLV_UIM_APDU_RESPONSE: u8 = 0x10;

pub const USIM_AID_PREFIX: &[u8] = &[0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x02];
pub const USIM_AUTHENTICATE_CLA: u8 = 0x00;
pub const USIM_AUTHENTICATE_INS: u8 = 0x88;
pub const USIM_AUTHENTICATE_P2_3G: u8 = 0x81;
pub const ISO_GET_RESPONSE_INS: u8 = 0xc0;
pub const EF_EPDG_ID: u16 = 0x6ff3;
pub const EF_EPDG_SELECTION: u16 = 0x6ff4;
const MAX_OPTIONAL_EF_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QmiMessage {
    pub service: u8,
    pub client_id: u8,
    pub transaction_id: u16,
    pub message_id: u16,
    pub tlvs: Vec<QmiTlv>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QmiTlv {
    pub tlv_type: u8,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QmiResult {
    pub success: bool,
    pub error_code: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalChannelOpened {
    pub channel_id: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UimApduResponse {
    pub data: Vec<u8>,
    pub sw1: u8,
    pub sw2: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsimAkaApduResult {
    pub res: Vec<u8>,
    pub ck: Vec<u8>,
    pub ik: Vec<u8>,
    pub auts: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsimIdentity {
    pub imsi: String,
    pub mnc_length: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsimEpdgAddress {
    Fqdn(String),
    Ip(IpAddr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpdgFqdnFormat {
    OperatorIdentifier,
    LocationBased,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsimEpdgSelectionEntry {
    /// MCC/MNC digits with `D` as the TS 31.102 wildcard. A two-digit MNC is
    /// represented by five characters; `DDDDDD` is Any_PLMN.
    pub plmn_pattern: String,
    pub priority: u16,
    pub fqdn_format: EpdgFqdnFormat,
}

impl UsimEpdgSelectionEntry {
    /// Match a PLMN using the TS 31.102 digit/wildcard representation.
    ///
    /// The EF stores a two-digit MNC as a five-character PLMN pattern, while
    /// DNS Operator Identifier names always encode that MNC as three digits
    /// with a leading zero. Accept both forms so UICC selection continues to
    /// work when the serving snapshot came from DNS/NAPTR rather than the
    /// modem's original MCC/MNC fields.
    pub fn matches_plmn(&self, plmn: &str) -> bool {
        epdg_plmn_pattern_matches(&self.plmn_pattern, plmn)
    }

    pub fn is_any_plmn(&self) -> bool {
        self.plmn_pattern == "DDDDDD"
    }
}

/// Match an EF ePDG-selection PLMN pattern against a five- or six-digit PLMN.
///
/// For a five-digit PLMN, the two-digit MNC occupies the last two positions
/// in the pattern. The canonical six-digit form used by 3GPP DNS names is
/// `MCC + 0 + MNC2`, so the comparison maps pattern positions 3/4 to
/// canonical positions 4/5 instead of treating the inserted zero as a real
/// operator digit.
pub fn epdg_plmn_pattern_matches(pattern: &str, plmn: &str) -> bool {
    let pattern = pattern.trim().to_ascii_uppercase();
    let plmn = plmn.trim();
    if !matches!(pattern.len(), 5 | 6)
        || !matches!(plmn.len(), 5 | 6)
        || !plmn.bytes().all(|byte| byte.is_ascii_digit())
        || !pattern
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'D')
    {
        return false;
    }

    if pattern == "DDDDDD" {
        return true;
    }

    let canonical = if plmn.len() == 6 {
        plmn.to_string()
    } else {
        format!("{}0{}", &plmn[..3], &plmn[3..])
    };
    let canonical = canonical.as_bytes();
    let positions: &[usize] = if pattern.len() == 6 {
        &[0, 1, 2, 3, 4, 5]
    } else {
        &[0, 1, 2, 4, 5]
    };
    pattern
        .bytes()
        .zip(positions.iter().copied())
        .all(|(pattern_digit, position)| {
            pattern_digit == b'D' || pattern_digit == canonical[position]
        })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsimEpdgConfig {
    pub home_identifiers: Vec<UsimEpdgAddress>,
    pub selection: Vec<UsimEpdgSelectionEntry>,
}

#[derive(Debug)]
pub enum QmiUimError {
    Io(io::Error),
    FrameTooShort,
    InvalidFrame,
    MessageTooLarge,
    MissingTlv(&'static str),
    ResultFailure(u16),
    InvalidApduResponse,
    InvalidAkaResponse,
    InvalidIdentityResponse,
    InvalidEpdgConfig,
}

impl fmt::Display for QmiUimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::FrameTooShort => write!(f, "QMI frame too short"),
            Self::InvalidFrame => write!(f, "invalid QMI frame"),
            Self::MessageTooLarge => write!(f, "QMI message too large"),
            Self::MissingTlv(name) => write!(f, "QMI response missing {name} TLV"),
            Self::ResultFailure(code) => write!(f, "QMI operation failed with code {code}"),
            Self::InvalidApduResponse => write!(f, "invalid UIM APDU response"),
            Self::InvalidAkaResponse => write!(f, "invalid USIM AKA response"),
            Self::InvalidIdentityResponse => write!(f, "invalid USIM identity response"),
            Self::InvalidEpdgConfig => write!(f, "invalid USIM ePDG configuration"),
        }
    }
}

impl std::error::Error for QmiUimError {}

impl From<io::Error> for QmiUimError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn build_proxy_open_frame(path: &str, transaction_id: u16) -> Result<Vec<u8>, QmiUimError> {
    encode_qmi_message(&QmiMessage {
        service: QMUX_CTL_SERVICE,
        client_id: 0,
        transaction_id,
        message_id: QMI_PROXY_OPEN,
        tlvs: vec![tlv(TLV_PROXY_DEVICE_PATH, path.as_bytes().to_vec())],
    })
}

pub fn build_allocate_uim_cid_frame(transaction_id: u16) -> Result<Vec<u8>, QmiUimError> {
    encode_qmi_message(&QmiMessage {
        service: QMUX_CTL_SERVICE,
        client_id: 0,
        transaction_id,
        message_id: QMI_CTL_ALLOCATE_CID,
        tlvs: vec![tlv(TLV_CTL_SERVICE, vec![QMUX_UIM_SERVICE])],
    })
}

pub fn build_release_uim_cid_frame(
    client_id: u8,
    transaction_id: u16,
) -> Result<Vec<u8>, QmiUimError> {
    encode_qmi_message(&QmiMessage {
        service: QMUX_CTL_SERVICE,
        client_id: 0,
        transaction_id,
        message_id: QMI_CTL_RELEASE_CID,
        tlvs: vec![tlv(TLV_CTL_SERVICE, vec![QMUX_UIM_SERVICE, client_id])],
    })
}

pub fn parse_allocated_cid(message: &QmiMessage) -> Result<u8, QmiUimError> {
    ensure_success(message)?;
    let value = find_tlv(message, TLV_CTL_ALLOCATION_INFO)
        .ok_or(QmiUimError::MissingTlv("allocation_info"))?;
    if value.len() < 2 || value[0] != QMUX_UIM_SERVICE {
        return Err(QmiUimError::InvalidFrame);
    }
    Ok(value[1])
}

pub fn build_open_logical_channel_frame(
    client_id: u8,
    transaction_id: u16,
    slot: u8,
    aid: &[u8],
) -> Result<Vec<u8>, QmiUimError> {
    let mut aid_value = Vec::with_capacity(1 + aid.len());
    aid_value.push(aid.len() as u8);
    aid_value.extend_from_slice(aid);
    encode_qmi_message(&QmiMessage {
        service: QMUX_UIM_SERVICE,
        client_id,
        transaction_id,
        message_id: QMI_UIM_OPEN_LOGICAL_CHANNEL,
        tlvs: vec![
            tlv(TLV_UIM_SLOT, vec![slot]),
            tlv(TLV_UIM_OPEN_AID, aid_value),
            tlv(TLV_UIM_OPEN_FCI, vec![0x00]),
        ],
    })
}

pub fn build_close_logical_channel_frame(
    client_id: u8,
    transaction_id: u16,
    slot: u8,
    channel_id: u8,
) -> Result<Vec<u8>, QmiUimError> {
    encode_qmi_message(&QmiMessage {
        service: QMUX_UIM_SERVICE,
        client_id,
        transaction_id,
        message_id: QMI_UIM_LOGICAL_CHANNEL,
        tlvs: vec![
            tlv(TLV_UIM_SLOT, vec![slot]),
            tlv(TLV_UIM_CHANNEL_ID, vec![channel_id]),
            tlv(0x13, vec![0x01]),
        ],
    })
}

pub fn parse_open_logical_channel(
    message: &QmiMessage,
) -> Result<LogicalChannelOpened, QmiUimError> {
    ensure_success(message)?;
    let value =
        find_tlv(message, TLV_UIM_CHANNEL_ID).ok_or(QmiUimError::MissingTlv("channel_id"))?;
    let channel_id = *value.first().ok_or(QmiUimError::InvalidFrame)?;
    Ok(LogicalChannelOpened { channel_id })
}

pub fn build_send_apdu_frame(
    client_id: u8,
    transaction_id: u16,
    slot: u8,
    channel_id: u8,
    apdu: &[u8],
) -> Result<Vec<u8>, QmiUimError> {
    let mut apdu_value = Vec::with_capacity(2 + apdu.len());
    apdu_value.extend_from_slice(&(apdu.len() as u16).to_le_bytes());
    apdu_value.extend_from_slice(apdu);
    encode_qmi_message(&QmiMessage {
        service: QMUX_UIM_SERVICE,
        client_id,
        transaction_id,
        message_id: QMI_UIM_SEND_APDU,
        tlvs: vec![
            tlv(TLV_UIM_SLOT, vec![slot]),
            tlv(TLV_UIM_APDU, apdu_value),
            tlv(TLV_UIM_CHANNEL_ID, vec![channel_id]),
            tlv(TLV_UIM_PROCEDURE_BYTES, vec![0x00]),
        ],
    })
}

pub fn build_usim_authenticate_apdu(rand: &[u8], autn: &[u8]) -> Result<Vec<u8>, QmiUimError> {
    if rand.len() > u8::MAX as usize || autn.len() > u8::MAX as usize {
        return Err(QmiUimError::MessageTooLarge);
    }
    let mut data = Vec::with_capacity(2 + rand.len() + autn.len());
    data.push(rand.len() as u8);
    data.extend_from_slice(rand);
    data.push(autn.len() as u8);
    data.extend_from_slice(autn);
    if data.len() > u8::MAX as usize {
        return Err(QmiUimError::MessageTooLarge);
    }
    Ok(vec![
        USIM_AUTHENTICATE_CLA,
        USIM_AUTHENTICATE_INS,
        0x00,
        USIM_AUTHENTICATE_P2_3G,
        data.len() as u8,
    ]
    .into_iter()
    .chain(data)
    .chain([0x00])
    .collect())
}

pub fn build_get_response_apdu(length: u8) -> Vec<u8> {
    vec![
        USIM_AUTHENTICATE_CLA,
        ISO_GET_RESPONSE_INS,
        0x00,
        0x00,
        length,
    ]
}

pub fn parse_send_apdu_response(message: &QmiMessage) -> Result<UimApduResponse, QmiUimError> {
    ensure_success(message)?;
    let value =
        find_tlv(message, TLV_UIM_APDU_RESPONSE).ok_or(QmiUimError::MissingTlv("apdu_response"))?;
    if value.len() < 4 {
        return Err(QmiUimError::InvalidApduResponse);
    }
    let len = u16::from_le_bytes([value[0], value[1]]) as usize;
    if value.len() < 2 + len || len < 2 {
        return Err(QmiUimError::InvalidApduResponse);
    }
    let apdu = &value[2..2 + len];
    let (body, status) = apdu.split_at(apdu.len() - 2);
    Ok(UimApduResponse {
        data: body.to_vec(),
        sw1: status[0],
        sw2: status[1],
    })
}

pub fn parse_usim_authenticate_response(
    response: &UimApduResponse,
) -> Result<UsimAkaApduResult, QmiUimError> {
    if response.sw1 != 0x90 || response.sw2 != 0x00 || response.data.is_empty() {
        return Err(QmiUimError::InvalidAkaResponse);
    }
    let data = unwrap_authenticate_response_data(&response.data)?;
    match data[0] {
        0xdb => parse_successful_auth_response(&data[1..]),
        0xdc => {
            let (auts, rest) = take_lv(&data[1..])?;
            if !rest.is_empty() {
                return Err(QmiUimError::InvalidAkaResponse);
            }
            Ok(UsimAkaApduResult {
                res: Vec::new(),
                ck: Vec::new(),
                ik: Vec::new(),
                auts: Some(auts.to_vec()),
            })
        }
        _ => Err(QmiUimError::InvalidAkaResponse),
    }
}

pub fn parse_usim_authenticate_response_reason(
    response: &UimApduResponse,
) -> Result<UsimAkaApduResult, &'static str> {
    parse_usim_authenticate_response(response)
        .map_err(|_| classify_usim_authenticate_response(response))
}

pub fn decode_ef_imsi(data: &[u8]) -> Result<String, QmiUimError> {
    let length = data
        .first()
        .copied()
        .map(usize::from)
        .ok_or(QmiUimError::InvalidIdentityResponse)?;
    if length == 0 || data.len() < 1 + length {
        return Err(QmiUimError::InvalidIdentityResponse);
    }

    let mut digits = String::new();
    let mut skip_identity_nibble = true;
    for byte in &data[1..1 + length] {
        for nibble in [byte & 0x0f, byte >> 4] {
            if skip_identity_nibble {
                skip_identity_nibble = false;
                continue;
            }
            if nibble == 0x0f {
                break;
            }
            if nibble > 9 {
                return Err(QmiUimError::InvalidIdentityResponse);
            }
            digits.push(char::from(b'0' + nibble));
        }
    }
    if (10..=16).contains(&digits.len()) {
        Ok(digits)
    } else {
        Err(QmiUimError::InvalidIdentityResponse)
    }
}

pub fn parse_ef_ad_mnc_length(data: &[u8]) -> Option<u8> {
    data.get(3)
        .map(|value| *value & 0x0f)
        .filter(|length| matches!(*length, 2 | 3))
}

/// Validate and canonicalize an ASCII ePDG FQDN read from a UICC or line
/// setting. URI syntax, ports, control characters and IDNA are intentionally
/// rejected: the DNS codec currently accepts DNS labels, not arbitrary URLs.
pub fn normalize_epdg_fqdn(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 253
        || !value.is_ascii()
        || !value.contains('.')
        || value.starts_with("sos.")
    {
        return None;
    }
    for label in value.split('.') {
        if label.is_empty()
            || label.len() > 63
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || !label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
        {
            return None;
        }
    }
    Some(value)
}

/// Parse TS 31.102 EFePDGId (6FF3). Reserved address types are ignored, while
/// malformed recognized values reject the file so an attacker-controlled UICC
/// cannot smuggle a URI or host-header fragment into DNS/IKE processing.
pub fn parse_ef_epdg_id(data: &[u8]) -> Result<Vec<UsimEpdgAddress>, QmiUimError> {
    let mut cursor = 0usize;
    let mut addresses = Vec::new();
    while cursor < data.len() {
        if data[cursor] == 0xff {
            if data[cursor..].iter().all(|byte| *byte == 0xff) {
                break;
            }
            return Err(QmiUimError::InvalidEpdgConfig);
        }
        if data[cursor] != 0x80 || cursor + 2 > data.len() {
            return Err(QmiUimError::InvalidEpdgConfig);
        }
        let length = usize::from(data[cursor + 1]);
        cursor += 2;
        if length == 0 || cursor + length > data.len() {
            return Err(QmiUimError::InvalidEpdgConfig);
        }
        let value = &data[cursor..cursor + length];
        cursor += length;
        let address = match value[0] {
            0x00 => {
                let fqdn = std::str::from_utf8(&value[1..])
                    .ok()
                    .and_then(normalize_epdg_fqdn)
                    .ok_or(QmiUimError::InvalidEpdgConfig)?;
                Some(UsimEpdgAddress::Fqdn(fqdn))
            }
            0x01 if value.len() == 5 => Some(UsimEpdgAddress::Ip(IpAddr::V4(Ipv4Addr::new(
                value[1], value[2], value[3], value[4],
            )))),
            0x02 if value.len() == 17 => {
                let octets: [u8; 16] = value[1..]
                    .try_into()
                    .map_err(|_| QmiUimError::InvalidEpdgConfig)?;
                Some(UsimEpdgAddress::Ip(IpAddr::V6(Ipv6Addr::from(octets))))
            }
            // TS 31.102 reserves every other address type. It may be defined by
            // a future release, so skip it rather than misinterpreting bytes.
            _ if !matches!(value[0], 0x00..=0x02) => None,
            _ => return Err(QmiUimError::InvalidEpdgConfig),
        };
        if let Some(address) = address {
            if !addresses.contains(&address) {
                addresses.push(address);
            }
        }
    }
    Ok(addresses)
}

/// Parse TS 31.102 EFePDGSelection (6FF4).
pub fn parse_ef_epdg_selection(data: &[u8]) -> Result<Vec<UsimEpdgSelectionEntry>, QmiUimError> {
    if data.is_empty() || data[0] != 0x80 {
        return Err(QmiUimError::InvalidEpdgConfig);
    }
    let (length, length_bytes) = parse_ber_length(&data[1..])?;
    let start = 1usize
        .checked_add(length_bytes)
        .ok_or(QmiUimError::InvalidEpdgConfig)?;
    let end = start
        .checked_add(length)
        .filter(|end| *end <= data.len())
        .ok_or(QmiUimError::InvalidEpdgConfig)?;
    if data[end..].iter().any(|byte| *byte != 0xff) || length % 6 != 0 {
        return Err(QmiUimError::InvalidEpdgConfig);
    }

    data[start..end]
        .chunks_exact(6)
        .map(|entry| {
            let plmn_pattern = decode_epdg_selection_plmn(&entry[..3])?;
            let priority = u16::from_be_bytes([entry[3], entry[4]]);
            let fqdn_format = match entry[5] {
                0x00 => EpdgFqdnFormat::OperatorIdentifier,
                0x01 => EpdgFqdnFormat::LocationBased,
                _ => return Err(QmiUimError::InvalidEpdgConfig),
            };
            Ok(UsimEpdgSelectionEntry {
                plmn_pattern,
                priority,
                fqdn_format,
            })
        })
        .collect()
}

fn parse_ber_length(data: &[u8]) -> Result<(usize, usize), QmiUimError> {
    let first = data
        .first()
        .copied()
        .ok_or(QmiUimError::InvalidEpdgConfig)?;
    match first {
        0x00..=0x7f => Ok((usize::from(first), 1)),
        0x81 if data.len() >= 2 => Ok((usize::from(data[1]), 2)),
        0x82 if data.len() >= 3 => Ok((usize::from(u16::from_be_bytes([data[1], data[2]])), 3)),
        // Indefinite and wider lengths are unnecessary for these small EFs and
        // would make bounds validation needlessly ambiguous.
        _ => Err(QmiUimError::InvalidEpdgConfig),
    }
}

fn decode_epdg_selection_plmn(data: &[u8]) -> Result<String, QmiUimError> {
    if data.len() != 3 {
        return Err(QmiUimError::InvalidEpdgConfig);
    }
    let digits = [
        data[0] & 0x0f,
        data[0] >> 4,
        data[1] & 0x0f,
        data[2] & 0x0f,
        data[2] >> 4,
        data[1] >> 4,
    ];
    if digits[..5]
        .iter()
        .any(|digit| !matches!(*digit, 0..=9 | 0x0d))
        || !matches!(digits[5], 0..=9 | 0x0d | 0x0f)
    {
        return Err(QmiUimError::InvalidEpdgConfig);
    }
    let length = if digits[5] == 0x0f { 5 } else { 6 };
    Ok(digits[..length]
        .iter()
        .map(|digit| {
            if *digit == 0x0d {
                'D'
            } else {
                char::from(b'0' + *digit)
            }
        })
        .collect())
}

/// Read optional TS 31.102 ePDG configuration from the exact QMI/UIM slot.
/// Missing 6FF3/6FF4 files are a normal empty result and never affect IMSI or
/// AKA access.
pub fn read_usim_epdg_config_via_proxy_reason(
    proxy_socket: &str,
    device_path: &str,
    slot: u8,
    aid: &[u8],
    timeout: Duration,
) -> Result<UsimEpdgConfig, &'static str> {
    #[cfg(not(unix))]
    {
        let _ = (proxy_socket, device_path, slot, aid, timeout);
        Err("sim_epdg_config_platform_unsupported")
    }

    #[cfg(unix)]
    {
        let mut conn = QmiProxyConnection::connect(proxy_socket, timeout)
            .map_err(|_| "sim_epdg_config_proxy_connect_failed")?;
        conn.proxy_open(device_path)
            .map_err(|_| "sim_epdg_config_proxy_open_failed")?;
        let client_id = conn
            .allocate_uim_cid()
            .map_err(|_| "sim_epdg_config_uim_client_failed")?;
        let channel = match conn.open_logical_channel(client_id, slot, aid) {
            Ok(channel) => channel,
            Err(_) => {
                let _ = conn.release_uim_cid(client_id);
                return Err("sim_epdg_config_logical_channel_failed");
            }
        };

        let result = (|| {
            // 6FF3 and 6FF4 are independent optional files. Preserve a usable
            // sibling when one file is absent, malformed or rejected instead
            // of discarding the entire UICC contribution.
            let home_identifiers = match read_optional_transparent_ef(
                &mut conn,
                client_id,
                slot,
                channel.channel_id,
                EF_EPDG_ID,
            ) {
                Ok(Some(data)) => match parse_ef_epdg_id(&data) {
                    Ok(values) => values,
                    Err(error) => {
                        tracing::warn!(
                            device_path,
                            slot,
                            error = %error,
                            "Ignoring malformed optional UICC ePDG identifier file"
                        );
                        Vec::new()
                    }
                },
                Ok(None) => Vec::new(),
                Err(error) => {
                    tracing::warn!(
                        device_path,
                        slot,
                        error = %error,
                        "Optional UICC ePDG identifier file could not be read"
                    );
                    Vec::new()
                }
            };
            let selection = match read_optional_transparent_ef(
                &mut conn,
                client_id,
                slot,
                channel.channel_id,
                EF_EPDG_SELECTION,
            ) {
                Ok(Some(data)) => match parse_ef_epdg_selection(&data) {
                    Ok(values) => values,
                    Err(error) => {
                        tracing::warn!(
                            device_path,
                            slot,
                            error = %error,
                            "Ignoring malformed optional UICC ePDG selection file"
                        );
                        Vec::new()
                    }
                },
                Ok(None) => Vec::new(),
                Err(error) => {
                    tracing::warn!(
                        device_path,
                        slot,
                        error = %error,
                        "Optional UICC ePDG selection file could not be read"
                    );
                    Vec::new()
                }
            };
            Ok(UsimEpdgConfig {
                home_identifiers,
                selection,
            })
        })();

        let _ = conn.close_logical_channel(client_id, slot, channel.channel_id);
        let _ = conn.release_uim_cid(client_id);
        result
    }
}

#[cfg(unix)]
fn read_optional_transparent_ef(
    conn: &mut QmiProxyConnection,
    client_id: u8,
    slot: u8,
    channel_id: u8,
    file_id: u16,
) -> Result<Option<Vec<u8>>, QmiUimError> {
    let [fid_high, fid_low] = file_id.to_be_bytes();
    let selected = conn.send_apdu(
        client_id,
        slot,
        channel_id,
        &[0x00, 0xa4, 0x00, 0x04, 0x02, fid_high, fid_low, 0x00],
    )?;
    if matches!((selected.sw1, selected.sw2), (0x6a, 0x82 | 0x83)) {
        return Ok(None);
    }
    if !matches!(selected.sw1, 0x90 | 0x61 | 0x9f) {
        return Err(QmiUimError::InvalidApduResponse);
    }
    let fcp = if matches!(selected.sw1, 0x61 | 0x9f) {
        conn.send_apdu(
            client_id,
            slot,
            channel_id,
            &build_get_response_apdu(selected.sw2),
        )
        .ok()
        .filter(|response| (response.sw1, response.sw2) == (0x90, 0x00))
        .map(|response| response.data)
        .unwrap_or_default()
    } else {
        selected.data
    };
    let file_size = parse_fcp_file_size(&fcp);
    if file_size.is_some_and(|size| size > MAX_OPTIONAL_EF_BYTES) {
        return Err(QmiUimError::MessageTooLarge);
    }

    let mut data = Vec::new();
    loop {
        if file_size.is_some_and(|size| data.len() >= size) {
            data.truncate(file_size.unwrap_or_default());
            return Ok(Some(data));
        }
        if data.len() >= MAX_OPTIONAL_EF_BYTES || data.len() > 0x7fff {
            return Err(QmiUimError::MessageTooLarge);
        }
        let remaining = file_size
            .map(|size| size.saturating_sub(data.len()))
            .unwrap_or(255);
        let requested = remaining.min(255);
        if requested == 0 {
            return Ok(Some(data));
        }
        let offset = data.len();
        let apdu = [
            0x00,
            0xb0,
            ((offset >> 8) & 0x7f) as u8,
            (offset & 0xff) as u8,
            requested as u8,
        ];
        let mut response = conn.send_apdu(client_id, slot, channel_id, &apdu)?;
        if response.sw1 == 0x6c {
            let mut adjusted = apdu;
            adjusted[4] = response.sw2;
            response = conn.send_apdu(client_id, slot, channel_id, &adjusted)?;
        }
        let eof = (response.sw1, response.sw2) == (0x62, 0x82);
        if (response.sw1, response.sw2) != (0x90, 0x00) && !eof {
            // When FCP was absent, a read immediately beyond an exact chunked
            // file may report an invalid offset. The bytes already read are a
            // complete bounded file in that case.
            if file_size.is_none()
                && !data.is_empty()
                && matches!((response.sw1, response.sw2), (0x6b, 0x00) | (0x6a, 0x86))
            {
                return Ok(Some(data));
            }
            return Err(QmiUimError::InvalidApduResponse);
        }
        let read_len = response.data.len();
        data.extend_from_slice(&response.data);
        if eof || read_len < requested || read_len == 0 {
            if let Some(size) = file_size {
                if data.len() < size {
                    return Err(QmiUimError::InvalidApduResponse);
                }
                data.truncate(size);
            }
            return Ok(Some(data));
        }
    }
}

pub(crate) fn parse_fcp_file_size(data: &[u8]) -> Option<usize> {
    let content = if data.first().copied() == Some(0x62) {
        let (length, length_bytes) = parse_ber_length(&data[1..]).ok()?;
        let start = 1 + length_bytes;
        data.get(start..start.checked_add(length)?)?
    } else {
        data
    };
    let mut cursor = 0usize;
    while cursor < content.len() {
        let tag = *content.get(cursor)?;
        cursor += 1;
        let (length, length_bytes) = parse_ber_length(content.get(cursor..)?).ok()?;
        cursor = cursor.checked_add(length_bytes)?;
        let value = content.get(cursor..cursor.checked_add(length)?)?;
        cursor += length;
        if matches!(tag, 0x80 | 0x81) && matches!(value.len(), 1..=4) {
            let size = value
                .iter()
                .fold(0usize, |size, byte| (size << 8) | usize::from(*byte));
            return Some(size);
        }
    }
    None
}

pub fn read_usim_identity_via_proxy_reason(
    proxy_socket: &str,
    device_path: &str,
    slot: u8,
    aid: &[u8],
    timeout: Duration,
) -> Result<UsimIdentity, &'static str> {
    #[cfg(not(unix))]
    {
        let _ = (proxy_socket, device_path, slot, aid, timeout);
        Err("sim_identity_platform_unsupported")
    }

    #[cfg(unix)]
    {
        let mut conn = QmiProxyConnection::connect(proxy_socket, timeout)
            .map_err(|_| "sim_identity_proxy_connect_failed")?;
        conn.proxy_open(device_path)
            .map_err(|_| "sim_identity_proxy_open_failed")?;
        let client_id = conn
            .allocate_uim_cid()
            .map_err(|_| "sim_identity_uim_client_failed")?;
        let channel = match conn.open_logical_channel(client_id, slot, aid) {
            Ok(channel) => channel,
            Err(_) => {
                let _ = conn.release_uim_cid(client_id);
                return Err("sim_identity_logical_channel_failed");
            }
        };

        let result = (|| {
            let selected = conn
                .send_apdu(
                    client_id,
                    slot,
                    channel.channel_id,
                    &[0x00, 0xa4, 0x00, 0x04, 0x02, 0x6f, 0x07, 0x00],
                )
                .map_err(|_| "sim_identity_imsi_select_failed")?;
            if !matches!(selected.sw1, 0x90 | 0x61 | 0x9f) {
                return Err("sim_identity_imsi_select_rejected");
            }
            let imsi_response = conn
                .send_apdu(
                    client_id,
                    slot,
                    channel.channel_id,
                    &[0x00, 0xb0, 0x00, 0x00, 0x09],
                )
                .map_err(|_| "sim_identity_imsi_read_failed")?;
            if (imsi_response.sw1, imsi_response.sw2) != (0x90, 0x00) {
                return Err("sim_identity_imsi_read_rejected");
            }
            let imsi = decode_ef_imsi(&imsi_response.data)
                .map_err(|_| "sim_identity_imsi_decode_failed")?;

            let mnc_length = conn
                .send_apdu(
                    client_id,
                    slot,
                    channel.channel_id,
                    &[0x00, 0xa4, 0x00, 0x04, 0x02, 0x6f, 0xad, 0x00],
                )
                .ok()
                .filter(|response| matches!(response.sw1, 0x90 | 0x61 | 0x9f))
                .and_then(|_| {
                    conn.send_apdu(
                        client_id,
                        slot,
                        channel.channel_id,
                        &[0x00, 0xb0, 0x00, 0x00, 0x04],
                    )
                    .ok()
                })
                .filter(|response| (response.sw1, response.sw2) == (0x90, 0x00))
                .and_then(|response| parse_ef_ad_mnc_length(&response.data));

            Ok(UsimIdentity { imsi, mnc_length })
        })();

        let _ = conn.close_logical_channel(client_id, slot, channel.channel_id);
        let _ = conn.release_uim_cid(client_id);
        result
    }
}

pub fn classify_usim_authenticate_response(response: &UimApduResponse) -> &'static str {
    match (response.sw1, response.sw2) {
        (0x90, 0x00) => match response.data.first().copied() {
            Some(0xdb) => "sim_auth_aka_success_parse_failed",
            Some(0xdc) => "sim_auth_aka_sync_failure_parse_failed",
            Some(_) => "sim_auth_aka_response_unknown_tag",
            None => "sim_auth_aka_response_empty",
        },
        (0x61, _) => "sim_auth_apdu_more_data_unhandled",
        (0x6c, _) => "sim_auth_apdu_wrong_length_unhandled",
        (0x67, 0x00) => "sim_auth_apdu_wrong_length",
        (0x69, 0x82) | (0x69, 0x85) => "sim_auth_apdu_security_status",
        (0x6a, 0x80) | (0x6a, 0x86) => "sim_auth_apdu_parameter_rejected",
        (0x6d, 0x00) => "sim_auth_apdu_instruction_not_supported",
        (0x6e, 0x00) => "sim_auth_apdu_class_not_supported",
        _ => "sim_auth_aka_response_parse_failed",
    }
}

pub fn execute_usim_authenticate_via_proxy(
    proxy_socket: &str,
    device_path: &str,
    slot: u8,
    aid: &[u8],
    rand: &[u8],
    autn: &[u8],
    timeout: Duration,
) -> Result<UsimAkaApduResult, QmiUimError> {
    #[cfg(not(unix))]
    {
        let _ = (proxy_socket, device_path, slot, aid, rand, autn, timeout);
        Err(QmiUimError::InvalidFrame)
    }

    #[cfg(unix)]
    {
        let mut conn = QmiProxyConnection::connect(proxy_socket, timeout)?;
        conn.proxy_open(device_path)?;
        let client_id = conn.allocate_uim_cid()?;
        let channel = conn.open_logical_channel(client_id, slot, aid)?;
        let apdu = build_usim_authenticate_apdu(rand, autn)?;
        let response = conn.send_apdu(client_id, slot, channel.channel_id, &apdu);
        let _ = conn.close_logical_channel(client_id, slot, channel.channel_id);
        let _ = conn.release_uim_cid(client_id);
        parse_usim_authenticate_response(&response?)
    }
}

pub fn execute_usim_authenticate_via_proxy_reason(
    proxy_socket: &str,
    device_path: &str,
    slot: u8,
    aid: &[u8],
    rand: &[u8],
    autn: &[u8],
    timeout: Duration,
) -> Result<UsimAkaApduResult, &'static str> {
    #[cfg(not(unix))]
    {
        let _ = (proxy_socket, device_path, slot, aid, rand, autn, timeout);
        Err("sim_auth_platform_unsupported")
    }

    #[cfg(unix)]
    {
        let mut conn = QmiProxyConnection::connect(proxy_socket, timeout)
            .map_err(|_| "sim_auth_proxy_connect_failed")?;
        conn.proxy_open(device_path)
            .map_err(|_| "sim_auth_proxy_open_failed")?;
        let client_id = conn
            .allocate_uim_cid()
            .map_err(|_| "sim_auth_uim_client_failed")?;
        let channel = match conn.open_logical_channel(client_id, slot, aid) {
            Ok(channel) => channel,
            Err(_) => {
                let _ = conn.release_uim_cid(client_id);
                return Err("sim_auth_logical_channel_failed");
            }
        };
        let apdu = match build_usim_authenticate_apdu(rand, autn) {
            Ok(apdu) => apdu,
            Err(_) => {
                let _ = conn.close_logical_channel(client_id, slot, channel.channel_id);
                let _ = conn.release_uim_cid(client_id);
                return Err("sim_auth_apdu_build_failed");
            }
        };
        let mut response = conn.send_apdu(client_id, slot, channel.channel_id, &apdu);
        if matches!(response.as_ref().map(|r| r.sw1), Ok(0x61)) {
            let len = response.as_ref().map(|r| r.sw2).unwrap_or(0);
            response = conn.send_apdu(
                client_id,
                slot,
                channel.channel_id,
                &build_get_response_apdu(len),
            );
        } else if matches!(response.as_ref().map(|r| r.sw1), Ok(0x6c)) {
            let le = response.as_ref().map(|r| r.sw2).unwrap_or(0);
            let mut adjusted = apdu.clone();
            if let Some(last) = adjusted.last_mut() {
                *last = le;
            }
            response = conn.send_apdu(client_id, slot, channel.channel_id, &adjusted);
        }
        let _ = conn.close_logical_channel(client_id, slot, channel.channel_id);
        let _ = conn.release_uim_cid(client_id);
        let response = response.map_err(|_| "sim_auth_apdu_exchange_failed")?;
        parse_usim_authenticate_response_reason(&response)
    }
}

// The public adapter mirrors the QMI transaction contract; keeping each retry
// and APDU input explicit is safer than a loosely scoped mutable context.
#[allow(clippy::too_many_arguments)]
pub fn execute_usim_authenticate_via_proxy_reason_with_retry(
    proxy_socket: &str,
    device_path: &str,
    slot: u8,
    aid: &[u8],
    rand: &[u8],
    autn: &[u8],
    attempts: usize,
    timeout: Duration,
    retry_delay: Duration,
) -> Result<UsimAkaApduResult, &'static str> {
    let attempts = attempts.max(1);
    let mut last_reason = "sim_auth_retry_not_attempted";
    for attempt in 1..=attempts {
        match execute_usim_authenticate_via_proxy_reason(
            proxy_socket,
            device_path,
            slot,
            aid,
            rand,
            autn,
            timeout,
        ) {
            Ok(result) => return Ok(result),
            Err(reason) => {
                last_reason = reason;
                if attempt == attempts || !sim_auth_reason_is_retryable(reason) {
                    return Err(reason);
                }
                std::thread::sleep(retry_delay);
            }
        }
    }
    Err(last_reason)
}

pub fn verify_usim_application_via_proxy_reason(
    proxy_socket: &str,
    device_path: &str,
    slot: u8,
    aid: &[u8],
    timeout: Duration,
) -> Result<(), &'static str> {
    #[cfg(not(unix))]
    {
        let _ = (proxy_socket, device_path, slot, aid, timeout);
        Err("sim_auth_platform_unsupported")
    }

    #[cfg(unix)]
    {
        let mut conn = QmiProxyConnection::connect(proxy_socket, timeout)
            .map_err(|_| "sim_auth_proxy_connect_failed")?;
        conn.proxy_open(device_path)
            .map_err(|_| "sim_auth_proxy_open_failed")?;
        let client_id = conn
            .allocate_uim_cid()
            .map_err(|_| "sim_auth_uim_client_failed")?;
        let channel = match conn.open_logical_channel(client_id, slot, aid) {
            Ok(channel) => channel,
            Err(_) => {
                let _ = conn.release_uim_cid(client_id);
                return Err("sim_auth_logical_channel_failed");
            }
        };
        let _ = conn.close_logical_channel(client_id, slot, channel.channel_id);
        let _ = conn.release_uim_cid(client_id);
        Ok(())
    }
}

pub fn verify_usim_application_via_proxy_reason_with_retry(
    proxy_socket: &str,
    device_path: &str,
    slot: u8,
    aid: &[u8],
    attempts: usize,
    timeout: Duration,
    retry_delay: Duration,
) -> Result<(), &'static str> {
    let attempts = attempts.max(1);
    let mut last_reason = "sim_auth_gate_not_attempted";
    for attempt in 1..=attempts {
        match verify_usim_application_via_proxy_reason(
            proxy_socket,
            device_path,
            slot,
            aid,
            timeout,
        ) {
            Ok(()) => return Ok(()),
            Err(reason) => {
                last_reason = reason;
                if attempt == attempts || !sim_auth_reason_is_retryable(reason) {
                    return Err(reason);
                }
                std::thread::sleep(retry_delay);
            }
        }
    }
    Err(last_reason)
}

pub fn sim_auth_reason_is_retryable(reason: &str) -> bool {
    matches!(
        reason,
        "sim_auth_proxy_connect_failed"
            | "sim_auth_proxy_open_failed"
            | "sim_auth_uim_client_failed"
            | "sim_auth_logical_channel_failed"
            | "sim_auth_logical_channel_close_failed"
            | "sim_auth_apdu_exchange_failed"
            | "sim_auth_apdu_security_status"
            | "sim_auth_aka_response_parse_failed"
    )
}

#[cfg(unix)]
struct QmiProxyConnection {
    stream: UnixStream,
    next_ctl_transaction: u16,
    next_service_transaction: u16,
}

#[cfg(unix)]
impl QmiProxyConnection {
    fn connect(proxy_socket: &str, timeout: Duration) -> Result<Self, QmiUimError> {
        let stream = if let Some(name) = proxy_socket.strip_prefix('@') {
            connect_abstract_socket(name)?
        } else {
            UnixStream::connect(proxy_socket)?
        };
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        Ok(Self {
            stream,
            next_ctl_transaction: 1,
            next_service_transaction: 1,
        })
    }

    fn proxy_open(&mut self, device_path: &str) -> Result<(), QmiUimError> {
        let tx = self.take_ctl_transaction();
        let frame = build_proxy_open_frame(device_path, tx)?;
        let response = self.send_and_recv(&frame)?;
        if response.message_id != QMI_PROXY_OPEN {
            return Err(QmiUimError::InvalidFrame);
        }
        ensure_success(&response)?;
        Ok(())
    }

    fn allocate_uim_cid(&mut self) -> Result<u8, QmiUimError> {
        let tx = self.take_ctl_transaction();
        let frame = build_allocate_uim_cid_frame(tx)?;
        let response = self.send_and_recv(&frame)?;
        if response.message_id != QMI_CTL_ALLOCATE_CID {
            return Err(QmiUimError::InvalidFrame);
        }
        parse_allocated_cid(&response)
    }

    fn release_uim_cid(&mut self, client_id: u8) -> Result<(), QmiUimError> {
        let tx = self.take_ctl_transaction();
        let frame = build_release_uim_cid_frame(client_id, tx)?;
        let response = self.send_and_recv(&frame)?;
        if response.message_id != QMI_CTL_RELEASE_CID {
            return Err(QmiUimError::InvalidFrame);
        }
        ensure_success(&response)?;
        Ok(())
    }

    fn open_logical_channel(
        &mut self,
        client_id: u8,
        slot: u8,
        aid: &[u8],
    ) -> Result<LogicalChannelOpened, QmiUimError> {
        let tx = self.take_service_transaction();
        let frame = build_open_logical_channel_frame(client_id, tx, slot, aid)?;
        let response = self.send_and_recv(&frame)?;
        if response.message_id != QMI_UIM_OPEN_LOGICAL_CHANNEL {
            return Err(QmiUimError::InvalidFrame);
        }
        parse_open_logical_channel(&response)
    }

    fn close_logical_channel(
        &mut self,
        client_id: u8,
        slot: u8,
        channel_id: u8,
    ) -> Result<(), QmiUimError> {
        let tx = self.take_service_transaction();
        let frame = build_close_logical_channel_frame(client_id, tx, slot, channel_id)?;
        let response = self.send_and_recv(&frame)?;
        if response.message_id != QMI_UIM_LOGICAL_CHANNEL {
            return Err(QmiUimError::InvalidFrame);
        }
        ensure_success(&response)?;
        Ok(())
    }

    fn send_apdu(
        &mut self,
        client_id: u8,
        slot: u8,
        channel_id: u8,
        apdu: &[u8],
    ) -> Result<UimApduResponse, QmiUimError> {
        let tx = self.take_service_transaction();
        let frame = build_send_apdu_frame(client_id, tx, slot, channel_id, apdu)?;
        let response = self.send_and_recv(&frame)?;
        if response.message_id != QMI_UIM_SEND_APDU {
            return Err(QmiUimError::InvalidFrame);
        }
        parse_send_apdu_response(&response)
    }

    fn send_and_recv(&mut self, frame: &[u8]) -> Result<QmiMessage, QmiUimError> {
        self.stream.write_all(frame)?;
        self.stream.flush()?;
        read_qmi_message(&mut self.stream)
    }

    fn take_ctl_transaction(&mut self) -> u16 {
        let current = self.next_ctl_transaction;
        self.next_ctl_transaction = if current == u8::MAX as u16 {
            1
        } else {
            current + 1
        };
        current
    }

    fn take_service_transaction(&mut self) -> u16 {
        let current = self.next_service_transaction;
        self.next_service_transaction = if current == u16::MAX { 1 } else { current + 1 };
        current
    }
}

#[cfg(unix)]
fn connect_abstract_socket(name: &str) -> io::Result<UnixStream> {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::SocketAddr;
    let addr = SocketAddr::from_abstract_name(name.as_bytes())?;
    UnixStream::connect_addr(&addr)
}

#[cfg(unix)]
fn read_qmi_message(stream: &mut UnixStream) -> Result<QmiMessage, QmiUimError> {
    let mut header = [0u8; 3];
    stream.read_exact(&mut header)?;
    if header[0] != 0x01 {
        return Err(QmiUimError::InvalidFrame);
    }
    let qmux_len = u16::from_le_bytes([header[1], header[2]]) as usize;
    let mut frame = Vec::with_capacity(1 + qmux_len);
    frame.extend_from_slice(&header);
    frame.resize(1 + qmux_len, 0);
    stream.read_exact(&mut frame[3..])?;
    decode_qmi_frame(&frame)
}

pub fn decode_qmi_frame(frame: &[u8]) -> Result<QmiMessage, QmiUimError> {
    if frame.len() < 12 {
        return Err(QmiUimError::FrameTooShort);
    }
    if frame[0] != 0x01 {
        return Err(QmiUimError::InvalidFrame);
    }
    let qmux_len = u16::from_le_bytes([frame[1], frame[2]]) as usize;
    if qmux_len + 1 != frame.len() || frame[3] != 0x80 && frame[3] != 0x00 {
        return Err(QmiUimError::InvalidFrame);
    }
    let service = frame[4];
    let client_id = frame[5];
    let (transaction_id, message_offset) = if service == QMUX_CTL_SERVICE {
        (u16::from(frame[7]), 8usize)
    } else {
        (u16::from_le_bytes([frame[7], frame[8]]), 9usize)
    };
    if frame.len() < message_offset + 4 {
        return Err(QmiUimError::FrameTooShort);
    }
    let message_id = u16::from_le_bytes([frame[message_offset], frame[message_offset + 1]]);
    let tlv_len =
        u16::from_le_bytes([frame[message_offset + 2], frame[message_offset + 3]]) as usize;
    let tlv_start = message_offset + 4;
    let tlv_end = tlv_start
        .checked_add(tlv_len)
        .ok_or(QmiUimError::InvalidFrame)?;
    if tlv_end != frame.len() {
        return Err(QmiUimError::InvalidFrame);
    }
    let tlvs = decode_tlvs(&frame[tlv_start..tlv_end])?;
    Ok(QmiMessage {
        service,
        client_id,
        transaction_id,
        message_id,
        tlvs,
    })
}

pub fn encode_qmi_message(message: &QmiMessage) -> Result<Vec<u8>, QmiUimError> {
    let mut tlv_bytes = Vec::new();
    for item in &message.tlvs {
        if item.value.len() > u16::MAX as usize {
            return Err(QmiUimError::MessageTooLarge);
        }
        tlv_bytes.push(item.tlv_type);
        tlv_bytes.extend_from_slice(&(item.value.len() as u16).to_le_bytes());
        tlv_bytes.extend_from_slice(&item.value);
    }
    if tlv_bytes.len() > u16::MAX as usize {
        return Err(QmiUimError::MessageTooLarge);
    }

    let mut qmi = Vec::new();
    qmi.push(0x00);
    if message.service == QMUX_CTL_SERVICE {
        if message.transaction_id > u8::MAX as u16 {
            return Err(QmiUimError::MessageTooLarge);
        }
        qmi.push(message.transaction_id as u8);
    } else {
        qmi.extend_from_slice(&message.transaction_id.to_le_bytes());
    }
    qmi.extend_from_slice(&message.message_id.to_le_bytes());
    qmi.extend_from_slice(&(tlv_bytes.len() as u16).to_le_bytes());
    qmi.extend_from_slice(&tlv_bytes);

    let qmux_len = 5usize
        .checked_add(qmi.len())
        .ok_or(QmiUimError::MessageTooLarge)?;
    if qmux_len > u16::MAX as usize {
        return Err(QmiUimError::MessageTooLarge);
    }
    let mut frame = Vec::with_capacity(1 + qmux_len);
    frame.push(0x01);
    frame.extend_from_slice(&(qmux_len as u16).to_le_bytes());
    frame.push(0x00);
    frame.push(message.service);
    frame.push(message.client_id);
    frame.extend_from_slice(&qmi);
    Ok(frame)
}

fn parse_successful_auth_response(input: &[u8]) -> Result<UsimAkaApduResult, QmiUimError> {
    let (res, rest) = take_lv(input)?;
    let (ck, rest) = take_lv(rest)?;
    let (ik, rest) = take_lv(rest)?;
    consume_optional_auth_tail(rest)?;
    Ok(UsimAkaApduResult {
        res: res.to_vec(),
        ck: ck.to_vec(),
        ik: ik.to_vec(),
        auts: None,
    })
}

fn consume_optional_auth_tail(mut input: &[u8]) -> Result<(), QmiUimError> {
    while !input.is_empty() {
        let (_value, rest) = take_lv(input)?;
        input = rest;
    }
    Ok(())
}

fn unwrap_authenticate_response_data(input: &[u8]) -> Result<&[u8], QmiUimError> {
    if matches!(input.first(), Some(0xdb | 0xdc)) {
        return Ok(input);
    }
    if input.len() >= 2 {
        let len = usize::from(input[1]);
        if input.len() == 2 + len && matches!(input[2], 0xdb | 0xdc) {
            return Ok(&input[2..]);
        }
    }
    if input.len() >= 3 && input[1] == 0x81 {
        let len = usize::from(input[2]);
        if input.len() == 3 + len && matches!(input[3], 0xdb | 0xdc) {
            return Ok(&input[3..]);
        }
    }
    Err(QmiUimError::InvalidAkaResponse)
}

fn take_lv(input: &[u8]) -> Result<(&[u8], &[u8]), QmiUimError> {
    let (&len, rest) = input.split_first().ok_or(QmiUimError::InvalidAkaResponse)?;
    let len = usize::from(len);
    if rest.len() < len {
        return Err(QmiUimError::InvalidAkaResponse);
    }
    Ok(rest.split_at(len))
}

fn decode_tlvs(mut input: &[u8]) -> Result<Vec<QmiTlv>, QmiUimError> {
    let mut tlvs = Vec::new();
    while !input.is_empty() {
        if input.len() < 3 {
            return Err(QmiUimError::InvalidFrame);
        }
        let tlv_type = input[0];
        let len = u16::from_le_bytes([input[1], input[2]]) as usize;
        if input.len() < 3 + len {
            return Err(QmiUimError::InvalidFrame);
        }
        tlvs.push(tlv(tlv_type, input[3..3 + len].to_vec()));
        input = &input[3 + len..];
    }
    Ok(tlvs)
}

fn ensure_success(message: &QmiMessage) -> Result<QmiResult, QmiUimError> {
    let value = find_tlv(message, TLV_RESULT).ok_or(QmiUimError::MissingTlv("result"))?;
    if value.len() < 4 {
        return Err(QmiUimError::InvalidFrame);
    }
    let result = u16::from_le_bytes([value[0], value[1]]);
    let error = u16::from_le_bytes([value[2], value[3]]);
    if result == 0 {
        Ok(QmiResult {
            success: true,
            error_code: None,
        })
    } else {
        Err(QmiUimError::ResultFailure(error))
    }
}

fn find_tlv(message: &QmiMessage, tlv_type: u8) -> Option<&[u8]> {
    message
        .tlvs
        .iter()
        .find(|item| item.tlv_type == tlv_type)
        .map(|item| item.value.as_slice())
}

fn tlv(tlv_type: u8, value: Vec<u8>) -> QmiTlv {
    QmiTlv { tlv_type, value }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_ef_imsi_and_ef_ad_mnc_length() {
        assert_eq!(
            decode_ef_imsi(&[0x08, 0x19, 0x32, 0x54, 0x76, 0x98, 0x10, 0x32, 0x54]).unwrap(),
            "123456789012345"
        );
        assert_eq!(parse_ef_ad_mnc_length(&[0x00, 0x00, 0x00, 0x02]), Some(2));
        assert_eq!(parse_ef_ad_mnc_length(&[0x00, 0x00, 0x00, 0x03]), Some(3));
        assert_eq!(parse_ef_ad_mnc_length(&[0x00, 0x00, 0x00, 0x04]), None);
    }

    #[test]
    fn parses_uicc_epdg_identifiers_and_normalizes_fqdns() {
        let fqdn = b"EPDG.Example.ORG.";
        let mut data = vec![0x80, (fqdn.len() + 1) as u8, 0x00];
        data.extend_from_slice(fqdn);
        data.extend_from_slice(&[0x80, 0x05, 0x01, 192, 0, 2, 10]);
        data.extend_from_slice(&[0x80, 0x11, 0x02]);
        data.extend_from_slice(&Ipv6Addr::LOCALHOST.octets());
        data.extend_from_slice(&[0x80, 0x02, 0x7f, 0x00]);
        data.extend_from_slice(&[0xff, 0xff]);

        assert_eq!(
            parse_ef_epdg_id(&data).unwrap(),
            vec![
                UsimEpdgAddress::Fqdn("epdg.example.org".to_string()),
                UsimEpdgAddress::Ip("192.0.2.10".parse().unwrap()),
                UsimEpdgAddress::Ip(IpAddr::V6(Ipv6Addr::LOCALHOST)),
            ]
        );
    }

    #[test]
    fn rejects_unsafe_uicc_epdg_fqdns() {
        for value in [
            "https://epdg.example.org",
            "epdg.example.org:500",
            "epdg..example.org",
            "-epdg.example.org",
            "epdg_.example.org",
            "sos.epdg.example.org",
            "epdg.example.org\r\nignored",
        ] {
            assert!(normalize_epdg_fqdn(value).is_none(), "accepted {value:?}");
        }
        assert!(parse_ef_epdg_id(&[0x80, 0x03, 0x00, 0xff, 0xfe]).is_err());
    }

    #[test]
    fn parses_uicc_epdg_selection_ber_length_plmn_and_priority() {
        let data = [
            0x80, 0x81, 0x0c, // top-level BER TLV, two entries
            0x43, 0xf5, 0x21, 0x00, 0x10, 0x01, // 345-12, priority 16, TAI
            0x13, 0x00, 0x62, 0x00, 0x01, 0x00, // 310-260, priority 1, OI
            0xff, 0xff,
        ];

        let entries = parse_ef_epdg_selection(&data).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].plmn_pattern, "34512");
        assert_eq!(entries[0].priority, 16);
        assert_eq!(entries[0].fqdn_format, EpdgFqdnFormat::LocationBased);
        assert!(entries[0].matches_plmn("34512"));
        assert_eq!(entries[1].plmn_pattern, "310260");
        assert_eq!(entries[1].priority, 1);
        assert_eq!(entries[1].fqdn_format, EpdgFqdnFormat::OperatorIdentifier);
    }

    #[test]
    fn parses_any_plmn_wildcard_and_fcp_file_size() {
        let entries =
            parse_ef_epdg_selection(&[0x80, 0x06, 0xdd, 0xdd, 0xdd, 0x00, 0x02, 0x00]).unwrap();
        assert!(entries[0].is_any_plmn());
        assert!(entries[0].matches_plmn("310260"));
        assert_eq!(
            parse_fcp_file_size(&[0x62, 0x04, 0x80, 0x02, 0x01, 0x2c]),
            Some(300)
        );
    }

    #[test]
    fn matches_epdg_selection_wildcards_and_both_mnc_lengths() {
        assert!(epdg_plmn_pattern_matches("34512", "34512"));
        assert!(epdg_plmn_pattern_matches("34512", "345012"));
        assert!(epdg_plmn_pattern_matches("310260", "310260"));
        assert!(!epdg_plmn_pattern_matches("310260", "31026"));

        // A six-digit wildcard pattern compares against the canonical six-digit
        // PLMN form, while D matches an arbitrary BCD digit.
        assert!(epdg_plmn_pattern_matches("34D12D", "345120"));
        assert!(!epdg_plmn_pattern_matches("34D12D", "345012"));
        assert!(epdg_plmn_pattern_matches("DDDDD", "31026"));
        assert!(epdg_plmn_pattern_matches("DDDDDD", "310260"));
        assert!(epdg_plmn_pattern_matches("DDDDDD", "31026"));

        for (pattern, plmn) in [
            ("", "310260"),
            ("1234", "310260"),
            ("1234567", "310260"),
            ("31X260", "310260"),
            ("310260", "31"),
            ("310260", "31026x"),
        ] {
            assert!(
                !epdg_plmn_pattern_matches(pattern, plmn),
                "accepted invalid PLMN pair {pattern:?} / {plmn:?}"
            );
        }
    }

    #[test]
    fn rejects_invalid_epdg_selection_plmn_nibbles_and_tail() {
        // Invalid BCD nibble in the MCC/MNC area.
        assert!(
            parse_ef_epdg_selection(&[0x80, 0x06, 0x4a, 0xf5, 0x21, 0x00, 0x01, 0x00]).is_err()
        );
        // A non-FF trailing byte is not an allowed transparent-EF tail.
        assert!(
            parse_ef_epdg_selection(&[0x80, 0x06, 0x43, 0xf5, 0x21, 0x00, 0x01, 0x00, 0x00])
                .is_err()
        );
    }
    #[test]
    fn encodes_ctl_proxy_and_allocate_cid_frames() {
        let proxy = build_proxy_open_frame("/dev/wwan0qmi0", 1).expect("proxy frame");
        assert_eq!(&proxy[..4], &[1, 28, 0, 0]);
        assert_eq!(proxy[4], QMUX_CTL_SERVICE);

        let alloc = build_allocate_uim_cid_frame(2).expect("alloc frame");
        let decoded = decode_qmi_frame(&{
            let mut response = alloc.clone();
            response[3] = 0x80;
            response
        })
        .expect("decode");
        assert_eq!(decoded.message_id, QMI_CTL_ALLOCATE_CID);
    }

    #[test]
    fn builds_usim_authenticate_apdu_without_serializing_values() {
        let apdu = build_usim_authenticate_apdu(&[0x11; 16], &[0x22; 16]).expect("apdu");

        assert_eq!(&apdu[..5], &[0x00, 0x88, 0x00, 0x81, 34]);
        assert_eq!(apdu[5], 16);
        assert_eq!(apdu[22], 16);
        assert_eq!(*apdu.last().expect("le"), 0);
    }

    #[test]
    fn parses_usim_authenticate_success_response() {
        let response = UimApduResponse {
            data: [
                vec![0xdb, 8],
                vec![0x11; 8],
                vec![16],
                vec![0x22; 16],
                vec![16],
                vec![0x33; 16],
            ]
            .concat(),
            sw1: 0x90,
            sw2: 0x00,
        };

        let parsed = parse_usim_authenticate_response(&response).expect("aka");

        assert_eq!(parsed.res.len(), 8);
        assert_eq!(parsed.ck.len(), 16);
        assert_eq!(parsed.ik.len(), 16);
        assert_eq!(parsed.auts, None);
    }

    #[test]
    fn parses_wrapped_usim_authenticate_success_response() {
        let inner = [
            vec![0xdb, 4],
            vec![0x11; 4],
            vec![16],
            vec![0x22; 16],
            vec![16],
            vec![0x33; 16],
        ]
        .concat();
        let response = UimApduResponse {
            data: [vec![0x80, inner.len() as u8], inner].concat(),
            sw1: 0x90,
            sw2: 0x00,
        };

        let parsed = parse_usim_authenticate_response(&response).expect("wrapped aka");

        assert_eq!(parsed.res.len(), 4);
        assert_eq!(parsed.ck.len(), 16);
        assert_eq!(parsed.ik.len(), 16);
    }

    #[test]
    fn accepts_optional_tail_after_usim_aka_ck_ik() {
        let response = UimApduResponse {
            data: [
                vec![0xdb, 8],
                vec![0x11; 8],
                vec![16],
                vec![0x22; 16],
                vec![16],
                vec![0x33; 16],
                vec![8],
                vec![0x44; 8],
            ]
            .concat(),
            sw1: 0x90,
            sw2: 0x00,
        };

        let parsed = parse_usim_authenticate_response(&response).expect("aka with optional tail");

        assert_eq!(parsed.res.len(), 8);
        assert_eq!(parsed.ck.len(), 16);
        assert_eq!(parsed.ik.len(), 16);
    }

    #[test]
    fn classifies_apdu_status_without_values() {
        let response = UimApduResponse {
            data: Vec::new(),
            sw1: 0x6a,
            sw2: 0x86,
        };

        assert_eq!(
            parse_usim_authenticate_response_reason(&response).unwrap_err(),
            "sim_auth_apdu_parameter_rejected"
        );
    }

    #[test]
    fn classifies_retryable_sim_auth_reasons() {
        assert!(sim_auth_reason_is_retryable(
            "sim_auth_logical_channel_failed"
        ));
        assert!(sim_auth_reason_is_retryable(
            "sim_auth_apdu_exchange_failed"
        ));
        assert!(!sim_auth_reason_is_retryable(
            "sim_auth_apdu_parameter_rejected"
        ));
        assert!(!sim_auth_reason_is_retryable(
            "sim_auth_platform_unsupported"
        ));
    }

    #[test]
    fn parses_send_apdu_response_tlv() {
        let mut value = Vec::new();
        value.extend_from_slice(&4u16.to_le_bytes());
        value.extend_from_slice(&[0xdb, 0x00, 0x90, 0x00]);
        let message = QmiMessage {
            service: QMUX_UIM_SERVICE,
            client_id: 3,
            transaction_id: 7,
            message_id: QMI_UIM_SEND_APDU,
            tlvs: vec![
                tlv(TLV_RESULT, vec![0, 0, 0, 0]),
                tlv(TLV_UIM_APDU_RESPONSE, value),
            ],
        };

        let response = parse_send_apdu_response(&message).expect("apdu response");

        assert_eq!(response.data, vec![0xdb, 0x00]);
        assert_eq!(response.sw1, 0x90);
        assert_eq!(response.sw2, 0x00);
    }
}
