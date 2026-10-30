//! Recovered Common object-reference grammar, not endpoint/trust authorization.
//!
//! Native identities may have slash-prefix components and balanced []/{} /<>
//! groups. A reference is identity-only, identity@adapter$locator, or identity:
//! a semicolon-delimited endpoint list. Explicit -h/-p and the recovered -ri
//! property are preserved as DATA. Parsing does not select a transport, trust
//! anchor, Router session or receive binding; other options are not guessed away.
use super::rpc_stream::MAX_FIELD_BYTES;
use std::fmt;
use zeroize::Zeroize;

const MAX_COMPONENTS: usize = 32;
const MAX_ENDPOINTS: usize = 32;
const MAX_GROUP_DEPTH: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceError { TooLarge, Malformed, UnsupportedSyntax, UnsupportedEndpoint, NoDirectEndpoints }
impl ReferenceError {
    pub fn code(self) -> &'static str {
        match self {
            Self::TooLarge => "jegotrip_reference_too_large", Self::Malformed => "jegotrip_reference_invalid",
            Self::UnsupportedSyntax => "jegotrip_reference_syntax_unsupported",
            Self::UnsupportedEndpoint => "jegotrip_reference_endpoint_unsupported",
            Self::NoDirectEndpoints => "jegotrip_reference_requires_resolver",
        }
    }
}

struct RouterIdentifier { value: i32, literal: Vec<u8> }
impl Drop for RouterIdentifier { fn drop(&mut self) { self.literal.zeroize(); } }

pub struct ReferenceEndpoint { scheme: Vec<u8>, host: Vec<u8>, port: u16, router_id: Option<RouterIdentifier> }
impl ReferenceEndpoint {
    pub fn scheme(&self) -> &[u8] { &self.scheme }
    pub fn host(&self) -> &[u8] { &self.host }
    pub fn port(&self) -> u16 { self.port }
    /// Original numeric Router association, NOT a port, ARC id or ready proof.
    pub fn router_id(&self) -> Option<i32> { self.router_id.as_ref().map(|id| id.value) }
}

/// Bounded data-only grouping of an explicit Router:tls/-ri reference. Native
/// RouterClientI skips clientLoad for these groups; a future owning startup task
/// must still establish the original Router paths and authenticated callbacks.
/// No TX/RX route is synthesized from the integer or reversed from an endpoint.
pub struct RouterEndpointGroup<'a> { router_id: i32, endpoints: Vec<&'a ReferenceEndpoint> }
impl<'a> RouterEndpointGroup<'a> {
    pub fn router_id(&self) -> i32 { self.router_id }
    pub fn endpoints(&self) -> &[&'a ReferenceEndpoint] { &self.endpoints }
}
impl fmt::Debug for RouterEndpointGroup<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RouterEndpointGroup(<redacted>)") }
}
impl Drop for ReferenceEndpoint { fn drop(&mut self) { self.scheme.zeroize(); self.host.zeroize(); } }
impl fmt::Debug for ReferenceEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ReferenceEndpoint(<redacted>)") }
}

pub enum ReferenceTarget {
    IdentityOnly,
    Direct(Vec<ReferenceEndpoint>),
    Adapter { adapter: Vec<u8>, locator: Option<Vec<u8>> },
}
impl Drop for ReferenceTarget {
    fn drop(&mut self) {
        match self {
            Self::Adapter { adapter, locator } => { adapter.zeroize(); if let Some(locator) = locator { locator.zeroize(); } }
            Self::Direct(endpoints) => endpoints.clear(),
            Self::IdentityOnly => {},
        }
    }
}
impl fmt::Debug for ReferenceTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ReferenceTarget(<redacted>)") }
}

pub struct ObjectReference { identity: Vec<u8>, components: Vec<Vec<u8>>, target: ReferenceTarget }
impl ObjectReference {
    /// Exact native identity path; it is not automatically a SIP user, line id
    /// or authorization token. Adapter/locator routing stays separate.
    pub fn identity(&self) -> &[u8] { &self.identity }
    pub fn components(&self) -> &[Vec<u8>] { &self.components }
    pub fn target(&self) -> &ReferenceTarget { &self.target }
    /// Current mode3 actor requires raw PacketSender TCP with inner TLS. A
    /// native tls/http/udp/proxy endpoint is NOT silently mapped to that route.
    pub fn direct_raw_tcp(&self) -> Result<&[ReferenceEndpoint], ReferenceError> {
        match &self.target {
            ReferenceTarget::Direct(endpoints) if endpoints.iter().all(|e| e.scheme == b"tcp" && e.router_id.is_none()) => Ok(endpoints),
            ReferenceTarget::Direct(_) => Err(ReferenceError::UnsupportedEndpoint),
            _ => Err(ReferenceError::NoDirectEndpoints),
        }
    }
    /// Preserve each original endpoint/Router association. Mixed transports,
    /// absent ids and native synthesized-negative-id discovery are unsupported;
    /// the entire reference fails rather than silently dropping an endpoint.
    pub fn explicit_tls_router_groups(&self) -> Result<Vec<RouterEndpointGroup<'_>>, ReferenceError> {
        if self.identity != b"Router" { return Err(ReferenceError::UnsupportedEndpoint); }
        let ReferenceTarget::Direct(endpoints) = &self.target else { return Err(ReferenceError::NoDirectEndpoints); };
        let mut groups = std::collections::BTreeMap::<i32, Vec<&ReferenceEndpoint>>::new();
        for endpoint in endpoints {
            let id = endpoint.router_id().ok_or(ReferenceError::UnsupportedEndpoint)?;
            // Native RouterClientI 0xd51ba4 admits id >= -999 and id != 0.
            if endpoint.scheme != b"tls" || id < -999 || id == 0 { return Err(ReferenceError::UnsupportedEndpoint); }
            groups.entry(id).or_default().push(endpoint);
        }
        Ok(groups.into_iter().map(|(router_id, endpoints)| RouterEndpointGroup { router_id, endpoints }).collect())
    }
    pub fn encode_native(&self) -> Result<ReferenceBytes, ReferenceError> {
        let mut bytes = self.identity.clone();
        match &self.target {
            ReferenceTarget::IdentityOnly => {},
            ReferenceTarget::Adapter { adapter, locator } => {
                bytes.push(b'@'); bytes.extend_from_slice(adapter);
                if let Some(locator) = locator { bytes.push(b'$'); bytes.extend_from_slice(locator); }
            }
            ReferenceTarget::Direct(endpoints) => {
                bytes.push(b':');
                for endpoint in endpoints {
                    bytes.extend_from_slice(&endpoint.scheme); bytes.extend_from_slice(b" -h ");
                    bytes.extend_from_slice(&endpoint.host); bytes.extend_from_slice(b" -p ");
                    bytes.extend_from_slice(endpoint.port.to_string().as_bytes());
                    if let Some(id) = &endpoint.router_id { bytes.extend_from_slice(b" -ri "); bytes.extend_from_slice(&id.literal); }
                    bytes.push(b';');
                }
            }
        }
        if bytes.len() > MAX_FIELD_BYTES { return Err(ReferenceError::TooLarge); }
        Ok(ReferenceBytes(bytes))
    }
}
impl Drop for ObjectReference {
    fn drop(&mut self) { self.identity.zeroize(); for component in &mut self.components { component.zeroize(); } self.components.clear(); }
}
impl fmt::Debug for ObjectReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ObjectReference(<redacted>)") }
}
pub struct ReferenceBytes(Vec<u8>);
impl ReferenceBytes { pub fn expose_to_backend(&self) -> &[u8] { &self.0 } }
impl Drop for ReferenceBytes { fn drop(&mut self) { self.0.zeroize(); } }
impl fmt::Debug for ReferenceBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ReferenceBytes(<redacted>)") }
}

fn space(byte: u8) -> bool { matches!(byte, b' ' | b'\t' | b'\r' | b'\n') }
fn trim(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|&b| !space(b)).unwrap_or(bytes.len());
    let end = bytes.iter().rposition(|&b| !space(b)).map_or(start, |i| i + 1);
    &bytes[start..end]
}
fn token(bytes: &[u8]) -> Result<(&[u8], Option<u8>, &[u8]), ReferenceError> {
    let bytes = trim(bytes); let mut groups = Vec::new();
    for (position, &byte) in bytes.iter().enumerate() {
        match byte {
            b'[' | b'{' | b'<' => {
                if groups.len() >= MAX_GROUP_DEPTH { return Err(ReferenceError::TooLarge); }
                groups.push(match byte { b'[' => b']', b'{' => b'}', _ => b'>' });
            }
            b']' | b'}' | b'>' => { if groups.pop() != Some(byte) { return Err(ReferenceError::Malformed); } }
            b'/' | b':' | b'@' if groups.is_empty() => {
                let value = trim(&bytes[..position]);
                if value.is_empty() { return Err(ReferenceError::Malformed); }
                return Ok((value, Some(byte), &bytes[position + 1..]));
            }
            b'\'' | b'"' | b'\\' => return Err(ReferenceError::UnsupportedSyntax),
            b' ' | b'\t' | b'\r' | b'\n' if groups.is_empty() => {
                if trim(&bytes[position..]).is_empty() { break; }
                return Err(ReferenceError::UnsupportedSyntax);
            }
            _ => {},
        }
    }
    if !groups.is_empty() || bytes.is_empty() { return Err(ReferenceError::Malformed); }
    Ok((bytes, None, &[]))
}

/// Endpoint parser 0xe84c58 uses Common's strtoll(base=0) helper 0xe73c2c,
/// then RouterClientI consumes a signed W-register id. Preserve the literal so
/// octal/hex/sign syntax is not silently changed by a decimal-only parser.
/// Overflow/wrapping is deliberately not supported by this bounded subset.
fn router_identifier(bytes: &[u8]) -> Result<RouterIdentifier, ReferenceError> {
    if bytes.is_empty() || bytes.len() > 32 { return Err(ReferenceError::Malformed); }
    let (negative, digits) = match bytes[0] {
        b'-' => (true, &bytes[1..]), b'+' => (false, &bytes[1..]), _ => (false, bytes),
    };
    let (radix, digits) = if digits.starts_with(b"0x") || digits.starts_with(b"0X") {
        (16, &digits[2..])
    } else if digits.starts_with(b"0") && digits.len() > 1 { (8, &digits[1..]) } else { (10, digits) };
    if digits.is_empty() || !digits.iter().all(|byte| match radix {
        16 => byte.is_ascii_hexdigit(), 8 => (b'0'..=b'7').contains(byte), _ => byte.is_ascii_digit(),
    }) { return Err(ReferenceError::Malformed); }
    let text = std::str::from_utf8(digits).map_err(|_| ReferenceError::Malformed)?;
    let magnitude = i64::from_str_radix(text, radix).map_err(|_| ReferenceError::Malformed)?;
    let value = i32::try_from(if negative { -magnitude } else { magnitude }).map_err(|_| ReferenceError::Malformed)?;
    if value == 0 { return Err(ReferenceError::Malformed); }
    Ok(RouterIdentifier { value, literal: bytes.to_vec() })
}

fn endpoints(bytes: &[u8]) -> Result<Vec<ReferenceEndpoint>, ReferenceError> {
    let mut endpoints = Vec::new(); let mut entries = bytes.split(|&b| b == b';').peekable();
    while let Some(entry) = entries.next() {
        let entry = trim(entry);
        if entry.is_empty() {
            if entries.peek().is_none() { continue; }
            return Err(ReferenceError::Malformed);
        }
        if endpoints.len() >= MAX_ENDPOINTS { return Err(ReferenceError::TooLarge); }
        let mut words = Vec::new();
        for word in entry.split(|&b| space(b)).filter(|w| !w.is_empty()) {
            if words.len() >= 7 { return Err(ReferenceError::UnsupportedEndpoint); }
            words.push(word);
        }
        let Some(&scheme) = words.first() else { return Err(ReferenceError::Malformed); };
        if scheme.is_empty() || scheme.len() > 16 || !scheme.iter().all(u8::is_ascii_alphanumeric) {
            return Err(ReferenceError::UnsupportedEndpoint);
        }
        let mut host = None; let mut port = None; let mut router_id = None; let mut offset = 1;
        while offset < words.len() {
            let value = words.get(offset + 1).ok_or(ReferenceError::Malformed)?;
            match words[offset] {
                b"-h" if host.is_none() => {
                    if !valid_host(value) { return Err(ReferenceError::UnsupportedEndpoint); }
                    host = Some(value.to_vec());
                }
                b"-p" if port.is_none() => {
                    if value.is_empty() || value.len() > 5 || !value.iter().all(u8::is_ascii_digit) { return Err(ReferenceError::Malformed); }
                    let number = std::str::from_utf8(value).ok().and_then(|v| v.parse::<u16>().ok()).filter(|n| *n != 0).ok_or(ReferenceError::Malformed)?;
                    port = Some(number);
                }
                b"-ri" if router_id.is_none() => {
                    // Native rejects -ri for arc/varc/sarc/svarc. Only its proven
                    // tcp/tls startup subset is admitted here, not other schemes.
                    if scheme != b"tcp" && scheme != b"tls" { return Err(ReferenceError::UnsupportedEndpoint); }
                    router_id = Some(router_identifier(value)?);
                }
                b"-h" | b"-p" | b"-ri" => return Err(ReferenceError::Malformed),
                _ => return Err(ReferenceError::UnsupportedEndpoint),
            }
            offset += 2;
        }
        endpoints.push(ReferenceEndpoint { scheme: scheme.to_vec(), host: host.ok_or(ReferenceError::Malformed)?, port: port.ok_or(ReferenceError::Malformed)?, router_id });
    }
    if endpoints.is_empty() { return Err(ReferenceError::Malformed); }
    Ok(endpoints)
}
fn valid_host(bytes: &[u8]) -> bool {
    let Ok(host) = std::str::from_utf8(bytes) else { return false; };
    if host.is_empty() || host.len() > 253 || !host.is_ascii() { return false; }
    if host.parse::<std::net::IpAddr>().is_ok() { return true; }
    if host.ends_with('.') { return false; }
    host.split('.').all(|label| !label.is_empty() && label.len() <= 63 && label.as_bytes()[0].is_ascii_alphanumeric()
        && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric() && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

fn adapter_target(value: &[u8]) -> Result<ReferenceTarget, ReferenceError> {
    let mut groups = Vec::new(); let mut separator = None;
    for (index, &byte) in value.iter().enumerate() {
        match byte {
            b'[' | b'{' | b'<' => {
                if groups.len() >= MAX_GROUP_DEPTH { return Err(ReferenceError::TooLarge); }
                groups.push(match byte { b'[' => b']', b'{' => b'}', _ => b'>' });
            }
            b']' | b'}' | b'>' => { if groups.pop() != Some(byte) { return Err(ReferenceError::Malformed); } }
            b'$' if groups.is_empty() => { if separator.replace(index).is_some() { return Err(ReferenceError::Malformed); } }
            _ => {},
        }
    }
    if !groups.is_empty() { return Err(ReferenceError::Malformed); }
    let (adapter, locator) = if let Some(index) = separator {
        (trim(&value[..index]), Some(trim(&value[index + 1..])))
    } else { (value, None) };
    for component in [Some(adapter), locator].into_iter().flatten() {
        let (_, delimiter, tail) = token(component)?;
        if delimiter.is_some() || !tail.is_empty() { return Err(ReferenceError::Malformed); }
    }
    Ok(ReferenceTarget::Adapter { adapter: adapter.to_vec(), locator: locator.map(|v| v.to_vec()) })
}

pub fn parse_object_reference(bytes: &[u8]) -> Result<ObjectReference, ReferenceError> {
    if bytes.len() > MAX_FIELD_BYTES { return Err(ReferenceError::TooLarge); }
    if bytes.iter().any(|&b| b == 0 || (b.is_ascii_control() && !space(b))) { return Err(ReferenceError::Malformed); }
    let mut components = Vec::new(); let mut rest = bytes;
    let target = loop {
        let (component, delimiter, tail) = token(rest)?;
        if components.len() >= MAX_COMPONENTS { return Err(ReferenceError::TooLarge); }
        components.push(component.to_vec()); rest = tail;
        match delimiter {
            Some(b'/') => {},
            Some(b':') => break ReferenceTarget::Direct(endpoints(rest)?),
            Some(b'@') => {
                let (adapter, delimiter, tail) = token(rest)?;
                if delimiter.is_some() || !tail.is_empty() { return Err(ReferenceError::Malformed); }
                break adapter_target(adapter)?;
            }
            None => break ReferenceTarget::IdentityOnly,
            _ => return Err(ReferenceError::Malformed),
        }
    };
    let mut identity = Vec::new();
    for (index, component) in components.iter().enumerate() { if index != 0 { identity.push(b'/'); } identity.extend_from_slice(component); }
    Ok(ObjectReference { identity, components, target })
}

#[cfg(test)]
#[path = "object_reference_tests.rs"]
mod tests;
