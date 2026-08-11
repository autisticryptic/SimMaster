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
    #[serde(
        rename = "originating-identity-presentation-restriction",
        alias = "originating-identity-restriction"
    )]
    OriginatingIdentityRestriction,
}

impl UtDocumentKind {
    pub fn document_name(self) -> &'static str {
        match self {
            Self::CommunicationWaiting => "communication-waiting",
            Self::CommunicationDiversion => "communication-diversion",
            Self::OriginatingIdentityPresentation => "originating-identity-presentation",
            Self::OriginatingIdentityRestriction => "originating-identity-presentation-restriction",
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

    /// Set the subscriber-visible identity state represented by this document.
    ///
    /// OIP and OIR use the same `active` XML element with inverse semantics:
    /// OIP active means presentation is allowed, while OIR active means
    /// presentation is restricted. Keeping that inversion here prevents API
    /// callers from accidentally reporting a network-confirmed CLIR setting
    /// opposite to what the carrier read back.
    pub fn set_identity_presentation(
        &mut self,
        value: IdentityPresentation,
    ) -> Result<(), UtError> {
        identity_active_value(self.kind, value)?;
        self.identity_presentation = Some(value);
        self.dirty = true;
        Ok(())
    }

    pub fn set_forwarding_rule(&mut self, rule: CallForwardingRule) -> Result<(), UtError> {
        if self.kind != UtDocumentKind::CommunicationDiversion {
            return Err(UtError::new("ut_document_kind_mismatch"));
        }
        if let Some(target) = rule.target_uri.as_deref() {
            validate_target_uri(target)?;
        }
        if let Some(existing) = self
            .forwarding
            .iter_mut()
            .find(|existing| existing.condition == rule.condition)
        {
            *existing = rule;
        } else {
            self.forwarding.push(rule);
        }
        self.dirty = true;
        Ok(())
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
            | UtDocumentKind::OriginatingIdentityRestriction => self
                .identity_presentation
                .and_then(|presentation| identity_active_value(self.kind, presentation).ok())
                .map(|active| active.to_string()),
            UtDocumentKind::CommunicationDiversion => {
                return rewrite_diversion_document(original, &self.forwarding)
            }
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
            if let Ok(active) = identity_active_value(self.kind, presentation) {
                xml.push_str(&format!("<active>{active}</active>"));
            }
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

fn rewrite_diversion_document(original: &str, desired: &[CallForwardingRule]) -> Option<String> {
    let mut reader = Reader::from_str(original);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::with_capacity(original.len() + 128));
    let mut rule_writer: Option<Writer<Vec<u8>>> = None;
    let mut rule_depth = 0usize;
    let mut seen = Vec::new();

    loop {
        let event = reader.read_event().ok()?;
        if let Some(buffer) = rule_writer.as_mut() {
            match &event {
                Event::Start(_) => rule_depth = rule_depth.saturating_add(1),
                Event::End(_) => rule_depth = rule_depth.saturating_sub(1),
                _ => {}
            }
            buffer.write_event(event.into_owned()).ok()?;
            if rule_depth == 0 {
                let fragment = String::from_utf8(rule_writer.take()?.into_inner()).ok()?;
                let parsed =
                    UtDocument::parse(UtDocumentKind::CommunicationDiversion, fragment.as_bytes())
                        .ok()?;
                let current = parsed.forwarding.first()?;
                let replacement = desired
                    .iter()
                    .find(|candidate| candidate.condition == current.condition);
                let output = match replacement {
                    Some(replacement) => rewrite_diversion_rule(&fragment, replacement)?,
                    None => fragment,
                };
                seen.push(current.condition);
                writer.get_mut().extend_from_slice(output.as_bytes());
            }
            continue;
        }

        match &event {
            Event::Start(start) if local_name(start.name().as_ref()) == "rule" => {
                rule_depth = 1;
                let mut buffer = Writer::new(Vec::new());
                buffer.write_event(event.into_owned()).ok()?;
                rule_writer = Some(buffer);
            }
            Event::End(end) if local_name(end.name().as_ref()) == "communication-diversion" => {
                for rule in desired
                    .iter()
                    .filter(|rule| !seen.contains(&rule.condition))
                {
                    writer
                        .get_mut()
                        .extend_from_slice(canonical_forwarding_rule(rule).as_bytes());
                }
                writer.write_event(event.into_owned()).ok()?;
            }
            Event::Eof => break,
            _ => writer.write_event(event.into_owned()).ok()?,
        }
    }
    String::from_utf8(writer.into_inner()).ok()
}

fn rewrite_diversion_rule(original: &str, desired: &CallForwardingRule) -> Option<String> {
    let mut reader = Reader::from_str(original);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::with_capacity(original.len() + 64));
    let mut stack = Vec::<String>::new();
    let mut saw_enabled = false;
    let mut saw_target = false;
    let mut saw_timer = false;
    loop {
        let event = reader.read_event().ok()?;
        match event {
            Event::Start(start) => {
                let name = local_name(start.name().as_ref());
                saw_enabled |= matches!(name.as_str(), "active" | "enabled");
                saw_target |= matches!(name.as_str(), "target" | "forward-to");
                saw_timer |= matches!(name.as_str(), "no-answer-timer" | "no-reply-timer");
                stack.push(name);
                writer.write_event(Event::Start(start.into_owned())).ok()?;
            }
            Event::Text(text) => {
                let replacement = match stack.last().map(String::as_str) {
                    Some("active" | "enabled") => Some(desired.enabled.to_string()),
                    Some("target" | "forward-to") => desired.target_uri.clone(),
                    Some("no-answer-timer" | "no-reply-timer") => desired
                        .no_reply_timer_seconds
                        .map(|value| value.to_string()),
                    _ => None,
                };
                match replacement {
                    Some(value) => writer
                        .write_event(Event::Text(quick_xml::events::BytesText::new(&value)))
                        .ok()?,
                    None => writer.write_event(Event::Text(text.into_owned())).ok()?,
                }
            }
            Event::End(end) => {
                let name = local_name(end.name().as_ref());
                if name == "rule" {
                    if !saw_enabled {
                        writer.get_mut().extend_from_slice(
                            format!("<enabled>{}</enabled>", desired.enabled).as_bytes(),
                        );
                    }
                    if !saw_target {
                        if let Some(target) = desired.target_uri.as_deref() {
                            writer.get_mut().extend_from_slice(
                                format!("<target>{}</target>", xml_escape(target)).as_bytes(),
                            );
                        }
                    }
                    if !saw_timer {
                        if let Some(timer) = desired.no_reply_timer_seconds {
                            writer.get_mut().extend_from_slice(
                                format!("<no-reply-timer>{timer}</no-reply-timer>").as_bytes(),
                            );
                        }
                    }
                }
                writer.write_event(Event::End(end.into_owned())).ok()?;
                stack.pop();
            }
            Event::Eof => break,
            other => writer.write_event(other.into_owned()).ok()?,
        }
    }
    String::from_utf8(writer.into_inner()).ok()
}

fn canonical_forwarding_rule(rule: &CallForwardingRule) -> String {
    canonical_forwarding_rule_with_namespace(rule, "")
}

fn forwarding_rule_id(condition: ForwardingCondition) -> &'static str {
    match condition {
        ForwardingCondition::Unconditional => "unconditional",
        ForwardingCondition::Busy => "busy",
        ForwardingCondition::NoReply => "no-reply",
        ForwardingCondition::NotReachable => "not-reachable",
    }
}

fn canonical_forwarding_rule_with_namespace(rule: &CallForwardingRule, namespace: &str) -> String {
    let id = forwarding_rule_id(rule.condition);
    let namespace = (!namespace.is_empty())
        .then(|| format!(" xmlns=\"{namespace}\""))
        .unwrap_or_default();
    let mut xml = format!(
        "<rule{namespace} id=\"{id}\"><enabled>{}</enabled>",
        rule.enabled
    );
    if let Some(target) = rule.target_uri.as_deref() {
        xml.push_str(&format!("<target>{}</target>", xml_escape(target)));
    }
    if let Some(timer) = rule.no_reply_timer_seconds {
        xml.push_str(&format!("<no-reply-timer>{timer}</no-reply-timer>"));
    }
    xml.push_str("</rule>");
    xml
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
        let condition = match attr(event, "id")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "busy" => ForwardingCondition::Busy,
            "no-answer" | "no-reply" => ForwardingCondition::NoReply,
            "not-reachable" | "not-registered" => ForwardingCondition::NotReachable,
            _ => ForwardingCondition::Unconditional,
        };
        Self {
            condition,
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
    if name == "active" {
        let active = parse_bool(value).ok_or_else(|| UtError::new("ut_boolean_invalid"))?;
        if let Some(presentation) = identity_presentation_from_active(document.kind, active) {
            document.identity_presentation = Some(presentation);
        }
    }
    Ok(())
}

fn identity_active_value(
    kind: UtDocumentKind,
    presentation: IdentityPresentation,
) -> Result<bool, UtError> {
    if presentation == IdentityPresentation::Unavailable {
        return Err(UtError::new("ut_identity_presentation_unavailable"));
    }
    match kind {
        UtDocumentKind::OriginatingIdentityPresentation => {
            Ok(presentation == IdentityPresentation::Allowed)
        }
        UtDocumentKind::OriginatingIdentityRestriction => {
            Ok(presentation == IdentityPresentation::Restricted)
        }
        _ => Err(UtError::new("ut_document_kind_mismatch")),
    }
}

fn identity_presentation_from_active(
    kind: UtDocumentKind,
    active: bool,
) -> Option<IdentityPresentation> {
    match kind {
        UtDocumentKind::OriginatingIdentityPresentation => Some(if active {
            IdentityPresentation::Allowed
        } else {
            IdentityPresentation::Restricted
        }),
        UtDocumentKind::OriginatingIdentityRestriction => Some(if active {
            IdentityPresentation::Restricted
        } else {
            IdentityPresentation::Allowed
        }),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XcapPolicy {
    pub root: String,
    pub document_selector: String,
    pub namespace: String,
    pub partial_update: bool,
    pub call_waiting_selector: Option<String>,
    pub diversion_rule_selector: Option<String>,
    pub oip_selector: Option<String>,
    pub oir_selector: Option<String>,
    pub tls_min_version: String,
    pub tls_max_version: String,
    pub tls_builtin_roots: bool,
    pub tls_additional_ca_pem: Option<String>,
}

impl XcapPolicy {
    pub fn validate(&self) -> Result<(), UtError> {
        let parsed =
            url::Url::parse(&self.root).map_err(|_| UtError::new("ut_xcap_root_invalid"))?;
        if parsed.scheme() != "https" || parsed.host_str().is_none() {
            return Err(UtError::new("ut_xcap_root_must_be_https"));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(UtError::new("ut_xcap_root_invalid"));
        }
        if self.document_selector.trim().is_empty() || self.namespace.trim().is_empty() {
            return Err(UtError::new("ut_xcap_policy_incomplete"));
        }
        let tls_rank = |value: &str| match value.trim() {
            "1.2" | "tls1.2" => Some(12_u8),
            "1.3" | "tls1.3" => Some(13_u8),
            _ => None,
        };
        let min_tls = tls_rank(&self.tls_min_version)
            .ok_or_else(|| UtError::new("ut_xcap_tls_version_invalid"))?;
        let max_tls = tls_rank(&self.tls_max_version)
            .ok_or_else(|| UtError::new("ut_xcap_tls_version_invalid"))?;
        if min_tls > max_tls {
            return Err(UtError::new("ut_xcap_tls_version_range_invalid"));
        }
        if !self.tls_builtin_roots
            && self
                .tls_additional_ca_pem
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err(UtError::new("ut_xcap_tls_trust_anchor_required"));
        }
        let selectors = [
            self.call_waiting_selector.as_deref(),
            self.diversion_rule_selector.as_deref(),
            self.oip_selector.as_deref(),
            self.oir_selector.as_deref(),
        ];
        if self.partial_update && selectors.iter().all(|value| value.is_none()) {
            return Err(UtError::new("ut_xcap_partial_selector_required"));
        }
        for selector in selectors.into_iter().flatten() {
            validate_element_selector(selector)?;
        }
        if self
            .diversion_rule_selector
            .as_deref()
            .is_some_and(|selector| !selector.contains("{rule-id}"))
        {
            return Err(UtError::new("ut_xcap_diversion_selector_template_invalid"));
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

    pub fn element_url(&self, kind: UtDocumentKind, selector: &str) -> Result<String, UtError> {
        validate_element_selector(selector)?;
        let mut uri = self.document_url(kind)?;
        uri.push_str("/~~/");
        uri.push_str(selector.trim());
        let parsed =
            url::Url::parse(&uri).map_err(|_| UtError::new("ut_xcap_partial_selector_invalid"))?;
        let root = url::Url::parse(&self.root).map_err(|_| UtError::new("ut_xcap_root_invalid"))?;
        if parsed.scheme() != root.scheme()
            || parsed.host_str() != root.host_str()
            || parsed.port_or_known_default() != root.port_or_known_default()
        {
            return Err(UtError::new("ut_xcap_partial_selector_invalid"));
        }
        Ok(uri)
    }
}

fn validate_element_selector(selector: &str) -> Result<(), UtError> {
    let selector = selector.trim();
    if selector.is_empty()
        || selector.starts_with('/')
        || selector.contains("://")
        || selector.contains('#')
        || selector.contains('\\')
        || selector.contains("..")
        || selector.chars().any(char::is_control)
    {
        return Err(UtError::new("ut_xcap_partial_selector_invalid"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UtMutation {
    CallWaiting(bool),
    ForwardingRule(CallForwardingRule),
    IdentityPresentation(IdentityPresentation),
}

impl UtMutation {
    pub fn apply(&self, document: &mut UtDocument) -> Result<(), UtError> {
        match self {
            Self::CallWaiting(enabled) => {
                if document.kind != UtDocumentKind::CommunicationWaiting {
                    return Err(UtError::new("ut_document_kind_mismatch"));
                }
                document.set_call_waiting(*enabled);
                Ok(())
            }
            Self::ForwardingRule(rule) => document.set_forwarding_rule(rule.clone()),
            Self::IdentityPresentation(value) => document.set_identity_presentation(*value),
        }
    }

    fn selector(&self, policy: &XcapPolicy, kind: UtDocumentKind) -> Option<String> {
        if !policy.partial_update {
            return None;
        }
        match (kind, self) {
            (UtDocumentKind::CommunicationWaiting, Self::CallWaiting(_)) => {
                policy.call_waiting_selector.clone()
            }
            (UtDocumentKind::CommunicationDiversion, Self::ForwardingRule(rule)) => policy
                .diversion_rule_selector
                .as_ref()
                .map(|selector| selector.replace("{rule-id}", forwarding_rule_id(rule.condition))),
            (UtDocumentKind::OriginatingIdentityPresentation, Self::IdentityPresentation(_)) => {
                policy.oip_selector.clone()
            }
            (UtDocumentKind::OriginatingIdentityRestriction, Self::IdentityPresentation(_)) => {
                policy.oir_selector.clone()
            }
            _ => None,
        }
    }

    fn partial_xml(&self, policy: &XcapPolicy, kind: UtDocumentKind) -> Result<String, UtError> {
        let namespace = xml_escape(&policy.namespace);
        match (kind, self) {
            (UtDocumentKind::CommunicationWaiting, Self::CallWaiting(enabled)) => {
                Ok(format!("<active xmlns=\"{namespace}\">{enabled}</active>"))
            }
            (UtDocumentKind::CommunicationDiversion, Self::ForwardingRule(rule)) => {
                Ok(canonical_forwarding_rule_with_namespace(rule, &namespace))
            }
            (
                UtDocumentKind::OriginatingIdentityPresentation
                | UtDocumentKind::OriginatingIdentityRestriction,
                Self::IdentityPresentation(value),
            ) => {
                let active = identity_active_value(kind, *value)?;
                Ok(format!("<active xmlns=\"{namespace}\">{active}</active>"))
            }
            _ => Err(UtError::new("ut_document_kind_mismatch")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XcapRequest {
    pub method: &'static str,
    pub uri: String,
    pub if_match: Option<String>,
    pub content_type: Option<&'static str>,
    pub body: Option<String>,
}

pub fn build_xcap_get(policy: &XcapPolicy, kind: UtDocumentKind) -> Result<XcapRequest, UtError> {
    Ok(XcapRequest {
        method: "GET",
        uri: policy.document_url(kind)?,
        if_match: None,
        content_type: None,
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
        content_type: Some("application/simservs+xml"),
        body: Some(document.to_xml()),
    })
}

pub fn build_xcap_partial_put(
    policy: &XcapPolicy,
    document: &UtDocument,
    mutation: &UtMutation,
) -> Result<Option<XcapRequest>, UtError> {
    let Some(selector) = mutation.selector(policy, document.kind) else {
        return Ok(None);
    };
    let etag = document
        .etag
        .clone()
        .ok_or_else(|| UtError::new("ut_if_match_required"))?;
    Ok(Some(XcapRequest {
        method: "PUT",
        uri: policy.element_url(document.kind, &selector)?,
        if_match: Some(etag),
        content_type: Some("application/xcap-el+xml"),
        body: Some(mutation.partial_xml(policy, document.kind)?),
    }))
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
    fn identity_documents_apply_their_opposite_active_semantics() {
        let oip_xml = r#"<oip:originating-identity-presentation xmlns:oip="urn:3gpp"><oip:active>true</oip:active><vendor:extension xmlns:vendor="urn:vendor">keep</vendor:extension></oip:originating-identity-presentation>"#;
        let mut oip = UtDocument::parse(
            UtDocumentKind::OriginatingIdentityPresentation,
            oip_xml.as_bytes(),
        )
        .unwrap();
        assert_eq!(
            oip.identity_presentation,
            Some(IdentityPresentation::Allowed)
        );
        oip.set_identity_presentation(IdentityPresentation::Restricted)
            .unwrap();
        let oip_updated = oip.to_xml();
        assert!(oip_updated.contains("<oip:active>false</oip:active>"));
        assert!(oip_updated
            .contains("<vendor:extension xmlns:vendor=\"urn:vendor\">keep</vendor:extension>"));

        let oir_xml = r#"<oir:originating-identity-presentation-restriction xmlns:oir="urn:3gpp"><oir:active>true</oir:active><vendor:extension xmlns:vendor="urn:vendor">keep</vendor:extension></oir:originating-identity-presentation-restriction>"#;
        let mut oir = UtDocument::parse(
            UtDocumentKind::OriginatingIdentityRestriction,
            oir_xml.as_bytes(),
        )
        .unwrap();
        assert_eq!(
            oir.identity_presentation,
            Some(IdentityPresentation::Restricted)
        );
        oir.set_identity_presentation(IdentityPresentation::Allowed)
            .unwrap();
        let oir_updated = oir.to_xml();
        assert!(oir_updated.contains("<oir:active>false</oir:active>"));
        assert!(oir_updated
            .contains("<vendor:extension xmlns:vendor=\"urn:vendor\">keep</vendor:extension>"));
    }

    #[test]
    fn identity_restriction_canonical_xml_and_unavailable_write_are_safe() {
        let mut document = UtDocument::empty(UtDocumentKind::OriginatingIdentityRestriction);
        document
            .set_identity_presentation(IdentityPresentation::Restricted)
            .unwrap();
        assert!(document
            .to_xml()
            .contains("<originating-identity-presentation-restriction"));
        assert!(document.to_xml().contains("<active>true</active>"));
        assert_eq!(
            document
                .set_identity_presentation(IdentityPresentation::Unavailable)
                .unwrap_err()
                .code(),
            "ut_identity_presentation_unavailable"
        );
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
    fn updating_one_diversion_rule_preserves_vendor_extensions_and_other_rules() {
        let xml = r#"<communication-diversion xmlns:vendor="urn:vendor"><rule id="busy"><conditions><busy/></conditions><actions><forward-to><target>tel:+601100000001</target></forward-to></actions><vendor:extension>keep</vendor:extension></rule><rule id="no-reply"><enabled>true</enabled><target>tel:+601100000002</target></rule></communication-diversion>"#;
        let mut document =
            UtDocument::parse(UtDocumentKind::CommunicationDiversion, xml.as_bytes()).unwrap();
        document
            .set_forwarding_rule(CallForwardingRule {
                condition: ForwardingCondition::Busy,
                enabled: false,
                target_uri: Some("tel:+601112023012".to_string()),
                no_reply_timer_seconds: None,
                etag: None,
            })
            .unwrap();
        let updated = document.to_xml();
        assert!(updated.contains("<vendor:extension>keep</vendor:extension>"));
        assert!(updated.contains("tel:+601112023012"));
        assert!(updated.contains("<enabled>false</enabled>"));
        assert!(updated.contains("tel:+601100000002"));
        let readback =
            UtDocument::parse(UtDocumentKind::CommunicationDiversion, updated.as_bytes()).unwrap();
        assert_eq!(readback.forwarding.len(), 2);
        assert_eq!(readback.forwarding[0].condition, ForwardingCondition::Busy);
        assert!(!readback.forwarding[0].enabled);
    }

    #[test]
    fn adding_diversion_rule_round_trips_its_condition() {
        let xml = "<communication-diversion></communication-diversion>";
        let mut document =
            UtDocument::parse(UtDocumentKind::CommunicationDiversion, xml.as_bytes()).unwrap();
        document
            .set_forwarding_rule(CallForwardingRule {
                condition: ForwardingCondition::NotReachable,
                enabled: true,
                target_uri: Some("sip:+601112023012@ims.example".to_string()),
                no_reply_timer_seconds: None,
                etag: None,
            })
            .unwrap();
        let updated = document.to_xml();
        let readback =
            UtDocument::parse(UtDocumentKind::CommunicationDiversion, updated.as_bytes()).unwrap();
        assert_eq!(
            readback.forwarding[0].condition,
            ForwardingCondition::NotReachable
        );
    }

    #[test]
    fn xcap_put_requires_etag_and_uses_https_policy() {
        let policy = XcapPolicy {
            root: "https://xcap.example.test".into(),
            document_selector: "simadmin/users".into(),
            namespace: "urn:3gpp:ns:communication-waiting".into(),
            partial_update: true,
            call_waiting_selector: Some("ss:communication-waiting/ss:active".into()),
            diversion_rule_selector: None,
            oip_selector: None,
            oir_selector: None,
            tls_min_version: "1.2".into(),
            tls_max_version: "1.3".into(),
            tls_builtin_roots: true,
            tls_additional_ca_pem: None,
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

        let mut document = UtDocument::empty(UtDocumentKind::CommunicationWaiting);
        document.etag = Some("v1".into());
        let request = build_xcap_partial_put(&policy, &document, &UtMutation::CallWaiting(true))
            .unwrap()
            .unwrap();
        assert!(request
            .uri
            .contains("/~~/ss:communication-waiting/ss:active"));
        assert_eq!(request.content_type, Some("application/xcap-el+xml"));
    }

    #[test]
    fn xcap_policy_rejects_unsafe_selector_and_tls_range() {
        let mut policy = XcapPolicy {
            root: "https://xcap.example.test".into(),
            document_selector: "simadmin/users".into(),
            namespace: "urn:3gpp:ns:xml:simservs".into(),
            partial_update: true,
            call_waiting_selector: Some("https://attacker.invalid/active".into()),
            diversion_rule_selector: None,
            oip_selector: None,
            oir_selector: None,
            tls_min_version: "1.2".into(),
            tls_max_version: "1.3".into(),
            tls_builtin_roots: true,
            tls_additional_ca_pem: None,
        };
        assert_eq!(
            policy.validate().unwrap_err().code(),
            "ut_xcap_partial_selector_invalid"
        );

        policy.call_waiting_selector = Some("ss:waiting/ss:active".into());
        policy.tls_min_version = "1.3".into();
        policy.tls_max_version = "1.2".into();
        assert_eq!(
            policy.validate().unwrap_err().code(),
            "ut_xcap_tls_version_range_invalid"
        );

        policy.tls_min_version = "1.2".into();
        policy.tls_builtin_roots = false;
        assert_eq!(
            policy.validate().unwrap_err().code(),
            "ut_xcap_tls_trust_anchor_required"
        );
    }
}
