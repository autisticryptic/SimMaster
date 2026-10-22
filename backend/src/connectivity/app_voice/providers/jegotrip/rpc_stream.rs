//! The native SDK's type-0 binary stream, before compression/TLS/net framing.
//!
//! The leading StreamData byte is part of every nested stream. All sizes/counts
//! below are local resource policies, not claims about server-negotiated limits.
//! Strings are length-prefixed bytes: binary metadata must not be UTF-8-lossy
//! decoded or rejected merely because it contains NUL.
use std::{cmp::Ordering, fmt};

pub const MAX_STREAM_BYTES: usize = 128 * 1024;
pub const MAX_FIELD_BYTES: usize = 64 * 1024;
pub const MAX_MAP_ENTRIES: usize = 128;
pub const STREAM_DATA: u8 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecError {
    TooLarge,
    Truncated,
    UnsupportedStream,
    UnsupportedVersion,
    DuplicateKey,
    InvalidCallId,
    InvalidField,
    RemoteFailure,
    TrailingData,
}

impl CodecError {
    pub fn code(self) -> &'static str {
        match self {
            Self::TooLarge => "jegotrip_rpc_arguments_too_large",
            Self::Truncated => "jegotrip_rpc_truncated",
            Self::UnsupportedStream => "jegotrip_rpc_stream_unsupported",
            Self::UnsupportedVersion => "jegotrip_rpc_version_unsupported",
            Self::DuplicateKey => "jegotrip_rpc_duplicate_key",
            Self::InvalidCallId => "jegotrip_rpc_call_id_invalid",
            Self::InvalidField => "jegotrip_rpc_field_invalid",
            Self::RemoteFailure => "jegotrip_rpc_remote_failure",
            Self::TrailingData => "jegotrip_rpc_trailing_data",
        }
    }
}

/// Contains tokens, credentials or message contents; never a public DTO.
pub struct RpcBytes(Vec<u8>);

impl RpcBytes {
    pub fn expose_to_transport(&self) -> &[u8] { &self.0 }
}

impl fmt::Debug for RpcBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcBytes").field("byte_length", &self.0.len())
            .field("data", &"<redacted>").finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RpcProperty<'a> {
    pub key: &'a [u8],
    pub value: &'a [u8],
}

impl fmt::Debug for RpcProperty<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RpcProperty(<redacted>)")
    }
}

pub(super) fn compare_keys(left: &[u8], right: &[u8]) -> Ordering {
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

pub(super) struct Writer(Vec<u8>);

impl Writer {
    pub fn new() -> Self { Self(vec![STREAM_DATA]) }

    /// Produces a new type-0 method argument stream, with major/minor words.
    /// Result streams do not repeat those version words.
    pub fn versioned() -> Result<Self, CodecError> {
        let mut writer = Self::new();
        writer.u16(1)?;
        writer.u16(0)?;
        Ok(writer)
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), CodecError> {
        let next = self.0.len().checked_add(bytes.len()).ok_or(CodecError::TooLarge)?;
        if next > MAX_STREAM_BYTES { return Err(CodecError::TooLarge); }
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    pub fn boolean(&mut self, value: bool) -> Result<(), CodecError> { self.append(&[u8::from(value)]) }
    pub fn u16(&mut self, value: u16) -> Result<(), CodecError> { self.append(&value.to_be_bytes()) }
    pub fn u32(&mut self, value: u32) -> Result<(), CodecError> { self.append(&value.to_be_bytes()) }
    pub fn i32(&mut self, value: i32) -> Result<(), CodecError> { self.append(&value.to_be_bytes()) }

    pub fn bytes(&mut self, value: &[u8]) -> Result<(), CodecError> {
        if value.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
        self.u32(u32::try_from(value.len()).map_err(|_| CodecError::TooLarge)?)?;
        self.append(value)
    }

    /// Native map<string,string> compares length first, then bytes.
    pub fn properties(&mut self, fields: &[RpcProperty<'_>]) -> Result<(), CodecError> {
        if fields.len() > MAX_MAP_ENTRIES { return Err(CodecError::TooLarge); }
        let mut fields: Vec<_> = fields.iter().collect();
        fields.sort_by(|left, right| compare_keys(left.key, right.key));
        if fields.windows(2).any(|pair| pair[0].key == pair[1].key) {
            return Err(CodecError::DuplicateKey);
        }
        self.u32(fields.len() as u32)?;
        for field in fields {
            self.bytes(field.key)?;
            self.bytes(field.value)?;
        }
        Ok(())
    }

    pub fn finish(self) -> RpcBytes { RpcBytes(self.0) }
}

pub(super) struct Reader<'a> { remaining: &'a [u8] }

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self, CodecError> {
        if bytes.len() > MAX_STREAM_BYTES { return Err(CodecError::TooLarge); }
        let (&kind, remaining) = bytes.split_first().ok_or(CodecError::Truncated)?;
        if kind != STREAM_DATA { return Err(CodecError::UnsupportedStream); }
        Ok(Self { remaining })
    }

    pub fn versioned(bytes: &'a [u8]) -> Result<Self, CodecError> {
        let mut reader = Self::new(bytes)?;
        if reader.u16()? != 1 || reader.u16()? != 0 { return Err(CodecError::UnsupportedVersion); }
        Ok(reader)
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], CodecError> {
        if count > self.remaining.len() { return Err(CodecError::Truncated); }
        let (value, remaining) = self.remaining.split_at(count);
        self.remaining = remaining;
        Ok(value)
    }

    pub fn u16(&mut self) -> Result<u16, CodecError> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().map_err(|_| CodecError::Truncated)?))
    }

    pub fn u32(&mut self) -> Result<u32, CodecError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().map_err(|_| CodecError::Truncated)?))
    }

    pub fn i32(&mut self) -> Result<i32, CodecError> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().map_err(|_| CodecError::Truncated)?))
    }

    /// The reviewed native bool reader treats any nonzero byte as true.
    pub fn boolean(&mut self) -> Result<bool, CodecError> { Ok(self.take(1)?[0] != 0) }

    pub fn bytes(&mut self) -> Result<&'a [u8], CodecError> {
        let count = self.u32()? as usize;
        if count > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
        self.take(count)
    }

    pub fn count(&mut self, minimum_entry_bytes: usize) -> Result<usize, CodecError> {
        let count = self.u32()? as usize;
        if count > MAX_MAP_ENTRIES { return Err(CodecError::TooLarge); }
        if count > self.remaining.len() / minimum_entry_bytes { return Err(CodecError::Truncated); }
        Ok(count)
    }

    pub fn properties(&mut self) -> Result<Vec<RpcProperty<'a>>, CodecError> {
        let count = self.count(8)?;
        let mut fields: Vec<RpcProperty<'a>> = Vec::with_capacity(count);
        for _ in 0..count {
            let key = self.bytes()?;
            let value = self.bytes()?;
            if fields.iter().any(|field| field.key == key) { return Err(CodecError::DuplicateKey); }
            fields.push(RpcProperty { key, value });
        }
        Ok(fields)
    }

    pub fn is_empty(&self) -> bool { self.remaining.is_empty() }

    pub fn finish(self) -> Result<(), CodecError> {
        if self.is_empty() { Ok(()) } else { Err(CodecError::TrailingData) }
    }
}

/// Generated native RPC result parsers always consume a terminal error string
/// (empty for success) and, when present, one map of result properties. Keep values private and opaque;
/// this is not a source of trusted phone-token renewal/expiry semantics.
pub struct RpcFooter<'a> {
    pub reason: &'a [u8],
    pub properties: Vec<RpcProperty<'a>>,
}

impl fmt::Debug for RpcFooter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RpcFooter(<redacted>)") }
}

pub(super) fn read_footer<'a>(reader: &mut Reader<'a>) -> Result<RpcFooter<'a>, CodecError> {
    let reason = reader.bytes()?;
    let properties = if reader.is_empty() { vec![] } else { reader.properties()? };
    Ok(RpcFooter { reason, properties })
}

#[cfg(test)]
#[path = "rpc_stream_tests.rs"]
mod tests;
