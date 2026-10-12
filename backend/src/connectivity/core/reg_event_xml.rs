//! Bounded, namespace-aware RFC 3680 reginfo parsing and own-binding state.
//! Private to reg_event: parsing alone cannot affect registration readiness.

use std::collections::{HashMap, HashSet};
use quick_xml::{events::{BytesStart, Event}, Reader};
use super::{canonical_uri, instance_value, invalid, unsigned, ImsError, RegEventBinding, RegEventEffect, RegEventTerminationReason, MAX_BODY};

const NS: &str = "urn:ietf:params:xml:ns:reginfo";
const MAX_DEPTH: usize = 16;
const MAX_NODES: usize = 1024;
const MAX_REGISTRATIONS: usize = 64;
const MAX_CONTACTS: usize = 256;

struct Node {
    ns: String,
    name: String,
    attrs: Vec<(String, String, String)>,
    children: Vec<Node>,
    text: String,
}
impl Node {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(ns, n, _)| ns.is_empty() && n == name).map(|(_, _, v)| v.as_str())
    }
    fn required(&self, name: &str) -> Result<&str, ImsError> { self.attr(name).ok_or_else(invalid) }
    fn element_only(&self) -> Result<(), ImsError> {
        if !self.text.trim().is_empty() { return Err(invalid()); }
        Ok(())
    }
    fn leaf_text(&self) -> Result<&str, ImsError> {
        if !self.children.is_empty() { return Err(invalid()); }
        Ok(self.text.trim())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State { Active, Terminated }
fn state(value: &str) -> Result<State, ImsError> {
    match value { "active" => Ok(State::Active), "terminated" => Ok(State::Terminated), _ => Err(invalid()) }
}
struct Contact {
    id: String,
    state: State,
    reason: Option<RegEventTerminationReason>,
    uri: Option<String>,
    instance: Option<String>,
    reg_id: Option<u32>,
    retry_after: Option<u32>,
}
struct Registration { id: String, aor: String, state: State, contacts: Vec<Contact> }
pub(super) struct Document { version: u32, full: bool, registrations: Vec<Registration> }

fn critical_element(name: &str) -> bool {
    matches!(name, "reginfo" | "registration" | "contact" | "uri" | "unknown-param")
}
fn critical_attribute(name: &str) -> bool {
    matches!(name, "version" | "state" | "id" | "aor" | "event" | "name" | "expires" | "duration-registered" | "retry-after")
}
fn ignore_extension(node: &Node) -> Result<(), ImsError> {
    // Extensions may be ignored, but cannot smuggle critical fields below an
    // unknown wrapper or hide duplicates using a foreign namespace.
    if critical_element(&node.name) { return Err(invalid()); }
    for child in &node.children { ignore_extension(child)?; }
    Ok(())
}
fn bounded_id(value: &str) -> Result<String, ImsError> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) { return Err(invalid()); }
    Ok(value.to_string())
}

pub(super) fn parse(body: &[u8]) -> Result<Document, ImsError> {
    let root = tree(body)?;
    if root.ns != NS || root.name != "reginfo" { return Err(invalid()); }
    root.element_only()?;
    let version = unsigned(root.required("version")?)?;
    let full = match root.required("state")? { "full" => true, "partial" => false, _ => return Err(invalid()) };
    let mut registrations = Vec::new();
    let mut registration_ids = HashSet::new();
    let mut aors = HashSet::new();
    let mut contact_count = 0;
    for node in &root.children {
        if node.name != "registration" || node.ns != NS { ignore_extension(node)?; continue; }
        node.element_only()?;
        let id = bounded_id(node.required("id")?)?;
        let aor = canonical_uri(node.required("aor")?)?;
        if !registration_ids.insert(id.clone()) || !aors.insert(aor.clone()) { return Err(invalid()); }
        let registration_state = state(node.required("state")?)?;
        let mut contacts = Vec::new();
        let mut contact_ids = HashSet::new();
        for child in &node.children {
            if child.name != "contact" || child.ns != NS { ignore_extension(child)?; continue; }
            child.element_only()?;
            let id = bounded_id(child.required("id")?)?;
            if !contact_ids.insert(id.clone()) { return Err(invalid()); }
            let contact_state = state(child.required("state")?)?;
            let reason = match child.required("event")? {
                "registered" | "created" | "refreshed" | "shortened" if contact_state == State::Active => None,
                "expired" if contact_state == State::Terminated => Some(RegEventTerminationReason::Expired),
                "deactivated" if contact_state == State::Terminated => Some(RegEventTerminationReason::Deactivated),
                "probation" if contact_state == State::Terminated => Some(RegEventTerminationReason::Probation),
                "rejected" if contact_state == State::Terminated => Some(RegEventTerminationReason::Rejected),
                "unregistered" if contact_state == State::Terminated => Some(RegEventTerminationReason::Unregistered),
                _ => return Err(invalid()),
            };
            if registration_state == State::Terminated && contact_state == State::Active { return Err(invalid()); }
            let retry_after = child.attr("retry-after").map(unsigned).transpose()?;
            if retry_after.is_some() && reason != Some(RegEventTerminationReason::Probation) { return Err(invalid()); }
            if let Some(expires) = child.attr("expires") {
                let expires = unsigned(expires)?;
                if (contact_state == State::Active && expires == 0) || (contact_state == State::Terminated && expires != 0) { return Err(invalid()); }
            }
            if let Some(duration) = child.attr("duration-registered") { unsigned(duration)?; }
            let mut uri = None;
            let mut instance = None;
            let mut reg_id = None;
            let mut param_names = HashSet::new();
            for field in &child.children {
                match (field.ns.as_str(), field.name.as_str()) {
                    (NS, "uri") => {
                        if uri.is_some() { return Err(invalid()); }
                        uri = Some(canonical_uri(field.leaf_text()?)?);
                    }
                    (NS, "unknown-param") => {
                        let name = field.required("name")?.to_ascii_lowercase();
                        if name.is_empty() || name.len() > 128 || !param_names.insert(name.clone()) { return Err(invalid()); }
                        let value = field.leaf_text()?;
                        match name.as_str() {
                            "+sip.instance" => instance = Some(instance_value(value)?),
                            "reg-id" => reg_id = Some(super::positive(value)?),
                            _ => {}
                        }
                    }
                    _ => ignore_extension(field)?,
                }
            }
            if (full || contact_state == State::Active) && uri.is_none() { return Err(invalid()); }
            contacts.push(Contact { id, state: contact_state, reason, uri, instance, reg_id, retry_after });
            contact_count += 1;
            if contact_count > MAX_CONTACTS { return Err(invalid()); }
        }
        registrations.push(Registration { id, aor, state: registration_state, contacts });
        if registrations.len() > MAX_REGISTRATIONS { return Err(invalid()); }
    }
    Ok(Document { version, full, registrations })
}

#[derive(Clone, Default)]
pub(super) struct Snapshot {
    version: Option<u32>,
    own: Option<(String, String)>,
    revoked: bool,
}
impl Snapshot {
    pub(super) fn apply(&mut self, document: &Document, binding: &RegEventBinding) -> Result<RegEventEffect, ImsError> {
        if self.version.is_some_and(|version| document.version <= version) { return Ok(RegEventEffect::None); }
        if !document.full && self.version.and_then(|v| v.checked_add(1)) != Some(document.version) { return Ok(RegEventEffect::NeedFull); }
        let registration = document.registrations.iter().find(|r| r.aor == binding.aor);
        let mut effect = RegEventEffect::None;
        let mut next_own = if document.full { None } else { self.own.clone() };
        if let Some(registration) = registration {
            let prior_contact = self.own.as_ref().filter(|(id, _)| id == &registration.id).map(|(_, id)| id);
            let mut matched = None;
            for contact in &registration.contacts {
                let exact = contact.uri.as_deref() == Some(binding.contact_uri.as_str())
                    && contact.instance == binding.instance && contact.reg_id == binding.reg_id;
                let mapped = !document.full && prior_contact == Some(&contact.id);
                if mapped {
                    // An old contact id is not permission to borrow another
                    // flow's explicit URI/instance/reg-id. Reject ambiguity.
                    if contact.uri.as_ref().is_some_and(|uri| uri != &binding.contact_uri)
                        || contact.instance.as_ref().is_some_and(|instance| Some(instance) != binding.instance.as_ref())
                        || contact.reg_id.is_some_and(|id| Some(id) != binding.reg_id)
                    { return Err(invalid()); }
                }
                if exact || mapped {
                    if matched.is_some() { return Err(invalid()); }
                    matched = Some(contact);
                }
            }
            if let Some(contact) = matched {
                if contact.state == State::Terminated {
                    effect = RegEventEffect::BindingTerminated { reason: contact.reason.ok_or_else(invalid)?, retry_after: contact.retry_after };
                    next_own = None;
                } else {
                    next_own = Some((registration.id.clone(), contact.id.clone()));
                }
            } else if prior_contact.is_some() && registration.contacts.is_empty()
                && (document.full || registration.state == State::Terminated)
            {
                // An initial empty full is NOT evidence. Disappearance is only
                // authoritative after this exact AoR/registration/contact was
                // established by a valid full document in this subscription.
                effect = RegEventEffect::BindingTerminated { reason: RegEventTerminationReason::Unregistered, retry_after: None };
                next_own = None;
            }
        }
        self.version = Some(document.version);
        self.own = next_own;
        if self.revoked { return Ok(RegEventEffect::None); }
        if matches!(effect, RegEventEffect::BindingTerminated { .. }) { self.revoked = true; }
        Ok(effect)
    }
}

// Use quick-xml's tokenizer and entity decoder, with explicit bounds and
// namespace scopes. DTD/entity declarations and processing instructions are
// forbidden; only predefined/numeric XML character references are decoded.
fn tree(body: &[u8]) -> Result<Node, ImsError> {
    if body.is_empty() || body.len() > MAX_BODY || std::str::from_utf8(body).is_err() { return Err(invalid()); }
    let mut reader = Reader::from_reader(body);
    reader.config_mut().check_end_names = true;
    reader.config_mut().check_comments = true;
    let mut stack: Vec<(Node, HashMap<String, String>)> = Vec::new();
    let mut root = None;
    let mut count = 0;
    let mut declaration = false;
    let mut prolog_content = false;
    loop {
        let event = reader.read_event().map_err(|_| invalid())?;
        let empty = matches!(&event, Event::Empty(_));
        match event {
            Event::Start(event) | Event::Empty(event) => {
                if stack.len() >= MAX_DEPTH || root.is_some() && stack.is_empty() { return Err(invalid()); }
                count += 1;
                if count > MAX_NODES { return Err(invalid()); }
                let parent_ns = stack.last().map(|(_, ns)| ns.clone()).unwrap_or_default();
                let (node, namespaces) = node(&event, parent_ns, &reader)?;
                if empty { append(node, &mut stack, &mut root)?; }
                else { stack.push((node, namespaces)); }
            }
            Event::End(_) => {
                let (node, _) = stack.pop().ok_or_else(invalid)?;
                append(node, &mut stack, &mut root)?;
            }
            Event::Text(text) => {
                if text.as_ref().windows(3).any(|w| w == b"]]>") { return Err(invalid()); }
                if stack.is_empty() { prolog_content = true; }
                let text = text.unescape().map_err(|_| invalid())?;
                if text.chars().any(|c| !xml_char(c)) { return Err(invalid()); }
                if let Some((node, _)) = stack.last_mut() {
                    if node.text.len() + text.len() > 4096 { return Err(invalid()); }
                    node.text.push_str(&text);
                } else if !text.trim().is_empty() { return Err(invalid()); }
            }
            Event::CData(text) => {
                let text = std::str::from_utf8(text.as_ref()).map_err(|_| invalid())?;
                if text.chars().any(|c| !xml_char(c)) { return Err(invalid()); }
                let (node, _) = stack.last_mut().ok_or_else(invalid)?;
                if node.text.len() + text.len() > 4096 { return Err(invalid()); }
                node.text.push_str(text);
            }
            Event::Decl(decl) => {
                if declaration || prolog_content || root.is_some() || !stack.is_empty() { return Err(invalid()); }
                if decl.version().map_err(|_| invalid())?.as_ref() != b"1.0" { return Err(invalid()); }
                if let Some(encoding) = decl.encoding() {
                    if !encoding.map_err(|_| invalid())?.eq_ignore_ascii_case(b"utf-8") { return Err(invalid()); }
                }
                declaration = true;
            }
            Event::Comment(comment) => {
                if stack.is_empty() { prolog_content = true; }
                if comment.as_ref().windows(2).any(|w| w == b"--") { return Err(invalid()); }
            }
            Event::Eof => break,
            Event::DocType(_) | Event::PI(_) => return Err(invalid()),
        }
    }
    if !stack.is_empty() { return Err(invalid()); }
    root.ok_or_else(invalid)
}
fn xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || (c >= '\u{20}' && c != '\u{fffe}' && c != '\u{ffff}')
}
fn append(node: Node, stack: &mut [(Node, HashMap<String, String>)], root: &mut Option<Node>) -> Result<(), ImsError> {
    if let Some((parent, _)) = stack.last_mut() { parent.children.push(node); }
    else if root.replace(node).is_some() { return Err(invalid()); }
    Ok(())
}
fn expand(name: &str, namespaces: &HashMap<String, String>, attribute: bool) -> Result<(String, String), ImsError> {
    let (prefix, local) = name.split_once(':').map_or((None, name), |(p, l)| (Some(p), l));
    if local.is_empty() || local.contains(':') || !local.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        || !local.as_bytes().first().is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
    { return Err(invalid()); }
    let ns = match prefix {
        Some(prefix) if !prefix.is_empty() => namespaces.get(prefix).cloned().ok_or_else(invalid)?,
        Some(_) => return Err(invalid()),
        None if attribute => String::new(),
        None => namespaces.get("").cloned().unwrap_or_default(),
    };
    Ok((ns, local.to_string()))
}
fn node(event: &BytesStart<'_>, mut namespaces: HashMap<String, String>, reader: &Reader<&[u8]>) -> Result<(Node, HashMap<String, String>), ImsError> {
    let mut attrs = Vec::new();
    let mut raw_names = HashSet::new();
    for attr in event.attributes() {
        let attr = attr.map_err(|_| invalid())?;
        let name = std::str::from_utf8(attr.key.as_ref()).map_err(|_| invalid())?.to_string();
        if !raw_names.insert(name.clone()) || raw_names.len() > 24 { return Err(invalid()); }
        if attr.value.as_ref().contains(&b'<') { return Err(invalid()); }
        let value = attr.decode_and_unescape_value(reader.decoder()).map_err(|_| invalid())?.into_owned();
        if value.len() > 2048 || value.chars().any(|c| !xml_char(c)) { return Err(invalid()); }
        if name == "xmlns" { namespaces.insert(String::new(), value); }
        else if let Some(prefix) = name.strip_prefix("xmlns:") {
            if prefix.is_empty() || prefix.contains(':') || prefix == "xmlns" || prefix == "xml" || value.is_empty() { return Err(invalid()); }
            namespaces.insert(prefix.to_string(), value);
        } else { attrs.push((name, value)); }
    }
    if namespaces.len() > 32 { return Err(invalid()); }
    // XML's predefined prefix is immutable and does not require a declaration.
    namespaces.insert("xml".into(), "http://www.w3.org/XML/1998/namespace".into());
    let qualified = event.name();
    let name = std::str::from_utf8(qualified.as_ref()).map_err(|_| invalid())?;
    let (ns, name) = expand(name, &namespaces, false)?;
    if critical_element(&name) && ns != NS { return Err(invalid()); }
    let mut expanded = Vec::new();
    let mut seen = HashSet::new();
    for (name, value) in attrs {
        let (ns, name) = expand(&name, &namespaces, true)?;
        if !seen.insert((ns.clone(), name.clone())) || (!ns.is_empty() && critical_attribute(&name)) { return Err(invalid()); }
        expanded.push((ns, name, value));
    }
    Ok((Node { ns, name, attrs: expanded, children: Vec::new(), text: String::new() }, namespaces))
}
