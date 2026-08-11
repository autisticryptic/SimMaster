//! Access-neutral IMS Ut/XCAP domain types.
//!
//! The access adapters own HTTP/TLS, Digest-AKA and routing.  This module only
//! validates the catalog policy, parses the small set of Ut documents we use,
//! and describes the GET/conditional PUT transaction.  Keeping this boundary
//! transport-free lets VoLTE and VoWiFi use exactly the same state machine.

use quick_xml::events::Event;
use quick_xml::{Reader, Writer};
use serde::{Deserialize, Serialize};

use super::supplementary::{CallForwardingRule, ForwardingCondition, IdentityPresentation};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UtDocumentKind {
    CommunicationWaiting,
    CommunicationDiversion,
    OriginatingIdentityPresentation,
    OriginatingIdentityRestriction,
}

impl UtDocumentKind {
    pub fn document_name(self) -> &'static str {
        match self {
            Self::CommunicationWaiting => "communication-waiting",
            Self::CommunicationDiversion => "communication-diversion",
            Self::OriginatingIdentityPresentation => "originating-identity-presentation",
            Self::OriginatingIdentityRestriction => "originating-identity-restriction",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UtDocument {
    pub kind: UtDocumentKind,
    pub call_waiting: Option<bool>,
    pub forwarding: Vec<CallForwardingRule>,
    pub identity_presentation: Option<IdentityPresentation>,
    /// The last network ETag. It is intentionally metadata, never a secret.
    pub etag: Option<String>,
    /// Original bytes are retained so a read-only GET/parse/GET round trip does
    /// not destroy carrier-specific XML extensions.
    #[serde(skip)]
    original_xml: Option<String>,
    #[serde(skip)]
    dirty: bool,
}

impl UtDocument {
    pub fn empty(kind: UtDocumentKind) -> Self {
        Self {
            kind,
            call_waiting: None,
            forwarding: Vec::new(),
            identity_presentation: None,
            etag: None,
            original_xml: None,
            dirty: true,
        }
    }

    pub fn parse(kind: UtDocumentKind, xml: &[u8]) -> Result<Self, UtError> {
        let text = std::str::from_utf8(xml).map_err(|_| UtError::new("ut_xml_not_utf8"))?;
        let mut reader = Reader::from_str(text);
        reader.config_mut().trim_text(true);
        let mut document = Self::empty(kind);
        document.dirty = false;
        document.original_xml = Some(text.to_string());
        let mut stack: Vec<String> = Vec::new();
        let mut rule: Option<PendingRule> = None;
        let mut text_value = String::new();

        loop {
            match reader.read_event() {
                Ok(Event::Start(event)) => {
                    let name = local_name(event.name().as_ref());
                    if name == "rule" {
                        rule = Some(PendingRule::from_attrs(&event));
                    } else if let Some(current) = rule.as_mut() {
                        current.observe_tag(&name, &event);
                    }
                    stack.push(name);
                }
                Ok(Event::Empty(event)) => {
                    let name = local_name(event.name().as_ref());
                    if name == "rule" {
                        document
                            .forwarding
                            .push(PendingRule::from_attrs(&event).finish()?);
                    } else if let Some(current) = rule.as_mut() {
                        current.observe_tag(&name, &event);
                    }
                    apply_empty_document_tag(&mut document, &name, &event)?;
                }
                Ok(Event::Text(event)) => {
                    text_value.push_str(
                        event
                            .unescape()
                            .map_err(|_| UtError::new("ut_xml_text_invalid"))?
                            .trim(),
                    );
                }
                Ok(Event::End(event)) => {
                    let name = local_name(event.name().as_ref());
                    let value = std::mem::take(&mut text_value);
                    if let Some(current) = rule.as_mut() {
                        current.observe_text(&name, &value)?;
                    } else {
                        apply_document_text(&mut document, &name, &value)?;
                    }
                    if name == "rule" {
                        if let Some(current) = rule.take() {
                            document.forwarding.push(current.finish()?);
                        }
                    }
                    stack.pop();
                }
                Ok(Event::Eof) => break,
                Err(_) => return Err(UtError::new("ut_xml_invalid")),
                _ => {}
            }
        }
        if !stack.is_empty() || rule.is_some() {
            return Err(UtError::new("ut_xml_unbalanced"));
        }
        Ok(document)
    }

    pub fn set_call_waiting(&mut self, enabled: bool) {
        self.call_waiting = Some(enabled);
        self.dirty = true;
    }

    pub fn set_identity_presentation(&mut self, value: IdentityPresentation) {
        self.identity_presentation = Some(value);
        self.dirty = true;
    }

    pub fn semantically_matches(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.call_waiting == other.call_waiting
            && self.forwarding == other.forwarding
            && self.identity_presentation == other.identity_presentation
    }

    pub fn to_xml(&self) -> String {
        if !self.dirty {
            if let Some(original) = &self.original_xml {
                return original.clone();
            }
        }
        if let Some(original) = &self.original_xml {
            if let Some(updated) = self.rewrite_original(original) {
                return updated;
            }
        }
        self.to_canonical_xml()
    }

    fn rewrite_original(&self, original: &str) -> Option<String> {
        let replacement = match self.kind {
            UtDocumentKind::CommunicationWaiting => {
                self.call_waiting.map(|value| value.to_string())
            }
            UtDocumentKind::OriginatingIdentityPresentation
            | UtDocumentKind::OriginatingIdentityRestriction => {
                self.identity_presentation.map(|presentation| {
                    matches!(presentation, IdentityPresentation::Allowed).to_string()
                })
            }
            UtDocumentKind::CommunicationDiversion => None,
        }?;
        let mut reader = Reader::from_str(original);
        reader.config_mut().trim_text(false);
        let mut writer = Writer::new(Vec::with_capacity(original.len() + 16));
        let mut stack: Vec<String> = Vec::new();
        let mut replaced = false;
        loop {
            let event = reader.read_event().ok()?;
            match event {
                Event::Start(start) => {
                    stack.push(local_name(start.name().as_ref()));
                    writer.write_event(Event::Start(start.into_owned())).ok()?;
                }
                Event::Empty(empty) => {
                    writer.write_event(Event::Empty(empty.into_owned())).ok()?;
                }
                Event::Text(_text) if stack.last().is_some_and(|name| name == "active") => {
                    writer
                        .write_event(Event::Text(quick_xml::events::BytesText::new(&replacement)))
                        .ok()?;
                    replaced = true;
                }
                Event::End(end) => {
                    writer.write_event(Event::End(end.into_owned())).ok()?;
                    stack.pop();
                }
                Event::Eof => break,
                other => writer.write_event(other.into_owned()).ok()?,
            }
        }
        replaced
            .then(|| String::from_utf8(writer.into_inner()).ok())
            .flatten()
    }

    fn to_canonical_xml(&self) -> String {
        let root = self.kind.document_name();
        let mut xml = format!("<{} xmlns=\"urn:3gpp:ns:xml:ue:communication\">", root);
        if let Some(enabled) = self.call_waiting {
            xml.push_str(&format!("<active>{}</active>", enabled));
        }
        if let Some(presentation) = self.identity_presentation {
            let value = match presentation {
                IdentityPresentation::Allowed => "true",
                IdentityPresentation::Restricted | IdentityPresentation::Unavailable => "false",
            };
            xml.push_str(&format!("<active>{}</active>", value));
        }
        for forwarding in &self.forwarding {
            let id = match forwarding.condition {
                ForwardingCondition::Unconditional => "unconditional",
                ForwardingCondition::Busy => "busy",
                ForwardingCondition::NoReply => "no-reply",
                ForwardingCondition::NotReachable => "not-reachable",
            };
            xml.push_str(&format!("<rule id=\"{}\">", id));
            xml.push_str(&format!("<enabled>{}</enabled>", forwarding.enabled));
            if let Some(target) = &forwarding.target_uri {
                xml.push_str("<target>");
                xml.push_str(&xml_escape(target));
                xml.push_str("</target>");
            }
            if let Some(timer) = forwarding.no_reply_timer_seconds {
                xml.push_str(&format!("<no-reply-timer>{timer}</no-reply-timer>"));
            }
            xml.push_str("</rule>");
        }
        xml.push_str(&format!("</{}>", root));
        xml
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingRule {
    condition: ForwardingCondition,
    enabled: bool,
    target_uri: Option<String>,
    timer: Option<u16>,
}

impl PendingRule {
    fn from_attrs(event: &quick_xml::events::BytesStart<'_>) -> Self {
        let enabled = attr(event, "active")
            .or_else(|| attr(event, "enabled"))
            .and_then(|value| parse_bool(&value))
            .unwrap_or(true);
        Self {
            condition: ForwardingCondition::Unconditional,
            enabled,
            target_uri: None,
            timer: None,
        }
    }

    fn observe_tag(&mut self, name: &str, event: &quick_xml::events::BytesStart<'_>) {
        self.condition = match name {
            "busy" => ForwardingCondition::Busy,
            "no-answer" | "no-reply" => ForwardingCondition::NoReply,
            "not-reachable" => ForwardingCondition::NotReachable,
            _ => self.condition,
        };
        if matches!(name, "active" | "enabled") {
            if let Some(value) = attr(event, "value").and_then(|value| parse_bool(&value)) {
                self.enabled = value;
            }
        }
    }

    fn observe_text(&mut self, name: &str, value: &str) -> Result<(), UtError> {
        if value.is_empty() {
            return Ok(());
        }
        match name {
            "target" | "forward-to" => self.target_uri = Some(value.to_string()),
            "active" | "enabled" => {
                self.enabled =
                    parse_bool(value).ok_or_else(|| UtError::new("ut_boolean_invalid"))?;
            }
            "no-answer-timer" | "no-reply-timer" => {
                self.timer = Some(
                    value
                        .parse()
                        .map_err(|_| UtError::new("ut_timer_invalid"))?,
                );
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(self) -> Result<CallForwardingRule, UtError> {
        if let Some(target) = &self.target_uri {
            validate_target_uri(target)?;
        }
        Ok(CallForwardingRule {
            condition: self.condition,
            enabled: self.enabled,
            target_uri: self.target_uri,
            no_reply_timer_seconds: self.timer,
            etag: None,
        })
    }
}

fn apply_empty_document_tag(
    document: &mut UtDocument,
    name: &str,
    event: &quick_xml::events::BytesStart<'_>,
) -> Result<(), UtError> {
    if matches!(name, "active" | "enabled") {
        if let Some(value) = attr(event, "value").or_else(|| attr(event, "active")) {
            let value = parse_bool(&value).ok_or_else(|| UtError::new("ut_boolean_invalid"))?;
            if document.kind == UtDocumentKind::CommunicationWaiting {
                document.call_waiting = Some(value);
            }
        }
    }
    Ok(())
}

fn apply_document_text(document: &mut UtDocument, name: &str, value: &str) -> Result<(), UtError> {
    if matches!(name, "active" | "enabled") && document.kind == UtDocumentKind::CommunicationWaiting
    {
        document.call_waiting =
            Some(parse_bool(value).ok_or_else(|| UtError::new("ut_boolean_invalid"))?);
    }
    if name == "active"
        && matches!(
            document.kind,
            UtDocumentKind::OriginatingIdentityPresentation
                | UtDocumentKind::OriginatingIdentityRestriction
        )
    {
        let active = parse_bool(value).ok_or_else(|| UtError::new("ut_boolean_invalid"))?;
        document.identity_presentation = Some(if active {
            IdentityPresentation::Allowed
        } else {
            IdentityPresentation::Restricted
        });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XcapPolicy {
    pub root: String,
    pub document_selector: String,
    pub namespace: String,
    pub partial_update: bool,
}

impl XcapPolicy {
    pub fn validate(&self) -> Result<(), UtError> {
        let parsed =
            url::Url::parse(&self.root).map_err(|_| UtError::new("ut_xcap_root_invalid"))?;
        if parsed.scheme() != "https" || parsed.host_str().is_none() {
            return Err(UtError::new("ut_xcap_root_must_be_https"));
        }
        if self.document_selector.trim().is_empty() || self.namespace.trim().is_empty() {
            return Err(UtError::new("ut_xcap_policy_incomplete"));
        }
        Ok(())
    }

    pub fn document_url(&self, kind: UtDocumentKind) -> Result<String, UtError> {
        self.validate()?;
        let mut root = self.root.trim_end_matches('/').to_string();
        root.push('/');
        root.push_str(self.document_selector.trim_matches('/'));
        root.push('/');
        root.push_str(kind.document_name());
        Ok(root)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XcapRequest {
    pub method: &'static str,
    pub uri: String,
    pub if_match: Option<String>,
    pub body: Option<String>,
}

pub fn build_xcap_get(policy: &XcapPolicy, kind: UtDocumentKind) -> Result<XcapRequest, UtError> {
    Ok(XcapRequest {
        method: "GET",
        uri: policy.document_url(kind)?,
        if_match: None,
        body: None,
    })
}

pub fn build_xcap_put(policy: &XcapPolicy, document: &UtDocument) -> Result<XcapRequest, UtError> {
    if document.etag.is_none() {
        return Err(UtError::new("ut_if_match_required"));
    }
    Ok(XcapRequest {
        method: "PUT",
        uri: policy.document_url(document.kind)?,
        if_match: document.etag.clone(),
        body: Some(document.to_xml()),
    })
}

fn validate_target_uri(value: &str) -> Result<(), UtError> {
    let value = value.trim();
    if value.starts_with("tel:") {
        let number = value
            .trim_start_matches("tel:")
            .split(';')
            .next()
            .unwrap_or_default();
        if number.starts_with('+') && number[1..].chars().all(|c| c.is_ascii_digit()) {
            return Ok(());
        }
    }
    if value.starts_with("sip:") || value.starts_with("sips:") {
        return Ok(());
    }
    Err(UtError::new("ut_forward_target_invalid"))
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" => Some(true),
        "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

fn attr(event: &quick_xml::events::BytesStart<'_>, wanted: &str) -> Option<String> {
    event.attributes().flatten().find_map(|attribute| {
        (local_name(attribute.key.as_ref()).eq_ignore_ascii_case(wanted))
            .then(|| String::from_utf8_lossy(attribute.value.as_ref()).into_owned())
    })
}

fn local_name(value: &[u8]) -> String {
    value
        .rsplit(|byte| *byte == b':')
        .next()
        .map(|part| String::from_utf8_lossy(part).to_ascii_lowercase())
        .unwrap_or_default()
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtError {
    code: &'static str,
}

impl UtError {
    pub const fn new(code: &'static str) -> Self {
        Self { code }
    }

    pub const fn code(self) -> &'static str {
        self.code
    }
}

impl std::fmt::Display for UtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code)
    }
}

impl std::error::Error for UtError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_namespaced_call_waiting_and_round_trips_unknown_xml() {
        let xml = br#"<cw:communication-waiting xmlns:cw=\"urn:3gpp:ns:communication-waiting\"><cw:active>true</cw:active><vendor:extension xmlns:vendor=\"urn:vendor\">x</vendor:extension></cw:communication-waiting>"#;
        let document = UtDocument::parse(UtDocumentKind::CommunicationWaiting, xml).unwrap();
        assert_eq!(document.call_waiting, Some(true));
        assert_eq!(document.to_xml(), String::from_utf8(xml.to_vec()).unwrap());
    }

    #[test]
    fn updating_call_waiting_preserves_unknown_extension() {
        let xml = "<cw:communication-waiting xmlns:cw=\"urn:3gpp:ns:communication-waiting\"><cw:active>true</cw:active><vendor:extension xmlns:vendor=\"urn:vendor\"><vendor:value>x</vendor:value></vendor:extension></cw:communication-waiting>";
        let mut document =
            UtDocument::parse(UtDocumentKind::CommunicationWaiting, xml.as_bytes()).unwrap();
        document.set_call_waiting(false);
        let updated = document.to_xml();
        assert!(updated.contains("<cw:active>false</cw:active>"));
        assert!(updated.contains(
            "<vendor:extension xmlns:vendor=\"urn:vendor\"><vendor:value>x</vendor:value></vendor:extension>"
        ));
    }

    #[test]
    fn parses_diversion_rules_and_rejects_local_target() {
        let xml = br#"<communication-diversion><rule id=\"busy\"><conditions><busy/></conditions><actions><forward-to><target>tel:+601112023012</target></forward-to></actions></rule></communication-diversion>"#;
        let document = UtDocument::parse(UtDocumentKind::CommunicationDiversion, xml).unwrap();
        assert_eq!(document.forwarding[0].condition, ForwardingCondition::Busy);
        assert_eq!(
            document.forwarding[0].target_uri.as_deref(),
            Some("tel:+601112023012")
        );
        assert!(UtDocument::parse(UtDocumentKind::CommunicationDiversion, br#"<communication-diversion><rule><target>tel:123</target></rule></communication-diversion>"#).is_err());
    }

    #[test]
    fn xcap_put_requires_etag_and_uses_https_policy() {
        let policy = XcapPolicy {
            root: "https://xcap.example.test".into(),
            document_selector: "simadmin/users".into(),
            namespace: "urn:3gpp:ns:communication-waiting".into(),
            partial_update: true,
        };
        assert!(build_xcap_get(&policy, UtDocumentKind::CommunicationWaiting).is_ok());
        assert_eq!(
            build_xcap_put(
                &policy,
                &UtDocument::empty(UtDocumentKind::CommunicationWaiting)
            )
            .unwrap_err()
            .code(),
            "ut_if_match_required"
        );
    }
}
