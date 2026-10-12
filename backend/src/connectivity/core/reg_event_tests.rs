//! Pure source regressions. Execute only in Actions: core::reg_event::tests.
use super::*;
use crate::connectivity::core::registration::{ImsRegistrationAccess, RegistrationLease};
use std::time::SystemTime;

const AOR: &str = "sip:alice@ims.example.test";
const CONTACT: &str = "sip:flow-a@192.0.2.20:5062;transport=udp";
const INSTANCE: &str = "<urn:uuid:00000000-0000-4000-8000-000000000001>";

fn register(contact: &str) -> Vec<u8> {
    format!("REGISTER sip:ims.example.test SIP/2.0\r\nFrom: <{AOR}>;tag=register\r\nTo: <{AOR}>\r\nContact: {contact}\r\nAuthorization: Digest username=\"secret-not-retained\"\r\nContent-Length: 0\r\n\r\n").into_bytes()
}
fn binding() -> RegEventBinding {
    RegEventBinding::from_register_request(&register(&format!("<{CONTACT}>;+sip.instance=\"{INSTANCE}\";reg-id=1;expires=3600"))).unwrap()
}
fn route(transport: SipTransport) -> ImsRoute {
    ImsRoute { local_addr: "192.0.2.20:5062".parse().unwrap(), pcscf_addr: "192.0.2.1:5060".parse().unwrap(), transport }
}
fn registration() -> RegisteredImsContext {
    RegisteredImsContext {
        access: ImsRegistrationAccess::CellularIms, registered_at: SystemTime::UNIX_EPOCH,
        lease: RegistrationLease::from_expires(600),
        service_route: Some("<sip:proxy.example.test;lr>".into()),
        associated_uris: vec!["sip:business-alias@example.test".into()],
    }
}
fn start(now: Instant, transport: SipTransport) -> (RegEventSubscription, Vec<u8>) {
    let mut sub = RegEventSubscription::new(binding(), now).unwrap();
    let wire = sub.build_request(&route(transport), &registration(), "test-agent", &[], None, now).unwrap();
    (sub, wire)
}
fn response(sub: &RegEventSubscription, status: u16, extra: &str) -> Vec<u8> {
    let branch = &sub.transaction.as_ref().unwrap().branch;
    let accepted = if status == 200 || status == 202 { "Contact: <sip:notifier@192.0.2.1:5060>\r\nExpires: 600\r\n" } else { "" };
    format!("SIP/2.0 {status} Test\r\nVia: SIP/2.0/UDP 192.0.2.20:5062;branch={branch}\r\nFrom: <{AOR}>;tag={}\r\nTo: <{AOR}>;tag=remote\r\nCall-ID: {}\r\nCSeq: {} SUBSCRIBE\r\n{accepted}{extra}Content-Length: 0\r\n\r\n", sub.local_tag, sub.call_id, sub.cseq).into_bytes()
}
fn notify(sub: &RegEventSubscription, cseq: u32, body: &str, state: &str) -> Vec<u8> {
    format!("NOTIFY {CONTACT} SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bKnotify\r\nFrom: <{AOR}>;tag=remote\r\nTo: <{AOR}>;tag={}\r\nCall-ID: {}\r\nCSeq: {cseq} NOTIFY\r\nContact: <sip:notifier@192.0.2.1:5060>\r\nEvent: reg\r\nSubscription-State: {state}\r\nContent-Type: application/reginfo+xml\r\nContent-Length: {}\r\n\r\n{body}", sub.local_tag, sub.call_id, body.len()).into_bytes()
}
fn xml_document(version: u32, mode: &str, aor: &str, contacts: &str) -> String {
    format!("<reginfo xmlns=\"urn:ietf:params:xml:ns:reginfo\" version=\"{version}\" state=\"{mode}\"><registration id=\"r1\" aor=\"{aor}\" state=\"active\">{contacts}</registration></reginfo>")
}
fn contact(id: &str, uri: &str, state: &str, event: &str) -> String {
    format!("<contact id=\"{id}\" state=\"{state}\" event=\"{event}\"><uri>{uri}</uri><unknown-param name=\"+sip.instance\">&lt;urn:uuid:00000000-0000-4000-8000-000000000001&gt;</unknown-param><unknown-param name=\"reg-id\">1</unknown-param></contact>")
}
fn active(version: u32) -> String { xml_document(version, "full", AOR, &contact("c1", CONTACT, "active", "registered")) }
fn termination(version: u32, mode: &str) -> String { xml_document(version, mode, AOR, &contact("c1", CONTACT, "terminated", "deactivated")) }
fn assert_notify(frame: RegEventFrame, status: u16, expected: RegEventEffect) {
    match frame {
        RegEventFrame::Notify { response_status, effect } => { assert_eq!(response_status, status); assert_eq!(effect, expected); }
        other => panic!("unexpected result: {other:?}"),
    }
}
fn establish(sub: &mut RegEventSubscription, now: Instant) {
    let response = response(sub, 200, "");
    assert!(matches!(sub.on_frame(&response, now), RegEventFrame::Response { status: 200 }));
    assert!(!sub.is_disabled());
}
fn deactivated() -> RegEventEffect {
    RegEventEffect::BindingTerminated { reason: RegEventTerminationReason::Deactivated, retry_after: None }
}

#[test]
fn folded_notify_headers_preserve_dialog_and_event_identity() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    establish(&mut sub, now);
    let body = active(0);
    let wire = String::from_utf8(notify(&sub, 1, &body, "active;expires=600")).unwrap()
        .replace("Event: reg", "Event :\r\n reg")
        .replace("active;expires=600", "active;\r\n expires=600");
    assert!(sub.wants_frame(wire.as_bytes()));
    assert_notify(sub.on_frame(wire.as_bytes(), now), 200, RegEventEffect::None);
    let other = wire.replace("Event :\r\n reg", "Event: message-summary").replace(&sub.call_id, "other");
    assert!(!sub.wants_frame(other.as_bytes()));
    assert!(matches!(sub.on_frame(other.as_bytes(), now), RegEventFrame::Other));
}

#[test]
fn captures_exact_final_register_binding_not_business_alias() {
    let binding = binding();
    assert_eq!(binding.aor, AOR);
    assert_eq!(binding.contact_uri, CONTACT);
    assert_eq!(binding.instance.as_deref(), Some(INSTANCE));
    assert_eq!(binding.reg_id, Some(1));
    assert_eq!(binding, binding.clone());
}

#[test]
fn binding_rejects_missing_ambiguous_custom_and_duplicate_contact() {
    for value in ["*", "<tel:+1000>", "<sip:u@example>;reg-id=1", "<sip:u@example>;reg-id=1;reg-id=2", "<sip:u@example>,<sip:v@example>", "sip:u@example;transport=udp"] {
        assert!(RegEventBinding::from_register_request(&register(value)).is_err());
    }
    let duplicate = String::from_utf8(register("<sip:u@example>")).unwrap().replace("Content-Length:", "Contact: <sip:v@example>\r\nContent-Length:");
    assert!(RegEventBinding::from_register_request(duplicate.as_bytes()).is_err());
}

#[test]
fn canonicalization_never_discards_user_port_or_uri_parameters() {
    assert_eq!(canonical_uri("SIP:Case@EXAMPLE.test:5070;transport=udp;ob").unwrap(), "sip:Case@example.test:5070;transport=udp;ob");
    for uri in ["sip:u:password@example.test", "sip:u@example:0", "sip:u@example:65536", "sip:u@example?Contact=x", "sip:u%xx@example", "sip:u@example;transport=udp;transport=tcp"] { assert!(canonical_uri(uri).is_err()); }
    assert_ne!(canonical_uri(CONTACT).unwrap(), canonical_uri(&CONTACT.replace(":5062", ":5063")).unwrap());
}

#[test]
fn redacted_debug_omits_binding_dialog_request_and_challenge_material() {
    let now = Instant::now();
    let (sub, wire) = start(now, SipTransport::Udp);
    let output = format!("{:?} {:?} {:?}", binding(), sub, RegEventAction::Retransmit(wire));
    for secret in [AOR, CONTACT, INSTANCE, "secret-not-retained", &sub.call_id, &sub.local_tag] { assert!(!output.contains(secret)); }
    let challenge = super::super::digest_aka::parse_digest_challenge("Digest realm=\"realm-secret\", nonce=\"nonce-secret\", algorithm=AKAv1-MD5", false).unwrap();
    assert_eq!(format!("{:?}", RegEventFrame::AuthChallenge { challenge }), "AuthChallenge([redacted])");
}

#[test]
fn valid_subscribe_uses_registered_route_and_exact_identity_with_fresh_ids() {
    let now = Instant::now();
    let (sub, wire) = start(now, SipTransport::Udp);
    let frame = Frame::parse(&wire).unwrap();
    assert_eq!(frame.method(), Some("SUBSCRIBE"));
    assert_eq!(frame.request_uri(), Some(AOR));
    assert_eq!(frame.one("event").unwrap(), Some("reg"));
    assert_eq!(frame.one("accept").unwrap(), Some("application/reginfo+xml"));
    assert_eq!(frame.one("expires").unwrap(), Some("600"));
    assert_eq!(frame.one("route").unwrap(), Some("<sip:proxy.example.test;lr>"));
    assert_eq!(frame.one("user-agent").unwrap(), Some("test-agent"));
    assert!(!String::from_utf8_lossy(&wire).contains("business-alias"));
    let (other, _) = start(now, SipTransport::Udp);
    assert_ne!(sub.call_id, other.call_id);
    assert_ne!(sub.local_tag, other.local_tag);
}

#[test]
fn request_rejects_injected_reserved_headers_and_stale_preemptive_auth() {
    let now = Instant::now();
    for header in [SipHeader::new("Event", "message-summary"), SipHeader::new("X-Test", "ok\r\nVia: malicious"), SipHeader::new("m", "<sip:other@example>")] {
        let mut sub = RegEventSubscription::new(binding(), now).unwrap();
        assert!(sub.build_request(&route(SipTransport::Udp), &registration(), "test", &[header], None, now).is_err());
    }
    let mut sub = RegEventSubscription::new(binding(), now).unwrap();
    assert!(sub.build_request(&route(SipTransport::Udp), &registration(), "test", &[], Some("Authorization: Digest response=\"old\""), now).is_err());
}

#[test]
fn udp_retransmits_are_identical_and_stop_at_finite_deadline() {
    let now = Instant::now();
    let (mut sub, wire) = start(now, SipTransport::Udp);
    assert_eq!(sub.next_deadline(), Some(now + T1));
    assert!(matches!(sub.poll(now), RegEventAction::Idle));
    match sub.poll(now + T1) { RegEventAction::Retransmit(bytes) => assert_eq!(bytes, wire), other => panic!("{other:?}") }
    assert!(matches!(sub.poll(now + TRANSACTION_TIMEOUT), RegEventAction::Idle));
    assert!(sub.is_disabled());
    assert_eq!(sub.next_deadline(), None);
    assert!(matches!(sub.poll(now + Duration::from_secs(86_400)), RegEventAction::Idle));
}

#[test]
fn tcp_never_retransmits_but_has_same_finite_timeout() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Tcp);
    assert_eq!(sub.next_deadline(), Some(now + TRANSACTION_TIMEOUT));
    assert!(matches!(sub.poll(now + Duration::from_secs(10)), RegEventAction::Idle));
    sub.poll(now + TRANSACTION_TIMEOUT);
    assert!(sub.is_disabled());
}

#[test]
fn provisional_response_does_not_complete_transaction_or_extend_timeout() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    let response = response(&sub, 100, "");
    assert!(matches!(sub.on_frame(&response, now + T1), RegEventFrame::Response { status: 100 }));
    assert_eq!(sub.next_deadline(), Some(now + T1 + T2));
    sub.poll(now + TRANSACTION_TIMEOUT);
    assert!(sub.is_disabled());
}

#[test]
fn success_establishes_dialog_target_and_reverses_record_route() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    let response = response(&sub, 202, "Record-Route: <sip:first.example;lr>, <sip:second.example;lr>\r\n");
    sub.on_frame(&response, now);
    assert_eq!(sub.request_uri(), "sip:notifier@192.0.2.1:5060");
    let renewal = now + Duration::from_secs(300);
    assert!(matches!(sub.poll(renewal), RegEventAction::Begin));
    let wire = sub.build_request(&route(SipTransport::Udp), &registration(), "test", &[], None, renewal).unwrap();
    let text = String::from_utf8(wire).unwrap();
    assert!(text.contains(&format!("To: <{AOR}>;tag=remote")));
    assert!(text.find("Route: <sip:second.example;lr>").unwrap() < text.find("Route: <sip:first.example;lr>").unwrap());
    assert!(text.contains("CSeq: 2 SUBSCRIBE"));
}

#[test]
fn success_lifetime_is_capped_at_requested_lease() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    let response = String::from_utf8(response(&sub, 200, "")).unwrap().replace("Expires: 600", "Expires: 4294967295");
    sub.on_frame(response.as_bytes(), now);
    assert_eq!(sub.lease_deadline, Some(now + Duration::from_secs(600)));
}

#[test]
fn mismatched_response_identifiers_do_not_mutate_transaction() {
    let now = Instant::now();
    for replace in [("CSeq: 1 SUBSCRIBE", "CSeq: 2 SUBSCRIBE"), (";tag=remote", ";tag=remote"), ("branch=z9hG4bK", "branch=z9hG4bKwrong")] {
        let (mut sub, _) = start(now, SipTransport::Udp);
        let mut wire = String::from_utf8(response(&sub, 200, "")).unwrap();
        if replace.0 == ";tag=remote" { wire = wire.replace(&sub.local_tag, "wronglocal"); }
        else { wire = wire.replace(replace.0, replace.1); }
        assert!(matches!(sub.on_frame(wire.as_bytes(), now), RegEventFrame::Other));
        assert!(sub.transaction.is_some());
        assert!(sub.remote_tag.is_none());
    }
}

#[test]
fn early_notify_is_accepted_only_after_complete_validation() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    let malformed = notify(&sub, 1, "<broken>", "active;expires=600");
    assert_notify(sub.on_frame(&malformed, now), 400, RegEventEffect::None);
    assert!(sub.remote_tag.is_none());
    let wire = notify(&sub, 1, &active(0), "active;expires=600");
    assert_notify(sub.on_frame(&wire, now), 200, RegEventEffect::None);
    assert_eq!(sub.remote_tag.as_deref(), Some("remote"));
    establish(&mut sub, now);
}

#[test]
fn early_notify_wrong_dialog_never_pins_remote_tag() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    let wire = String::from_utf8(notify(&sub, 1, &active(0), "active;expires=600")).unwrap().replace(&sub.local_tag, "wronglocal");
    assert_notify(sub.on_frame(wire.as_bytes(), now), 481, RegEventEffect::None);
    assert!(sub.remote_tag.is_none());
}

#[test]
fn full_current_binding_termination_revokes_once_and_duplicate_is_acked() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    establish(&mut sub, now);
    let wire = notify(&sub, 1, &termination(0, "full"), "active;expires=600");
    assert_notify(sub.on_frame(&wire, now), 200, deactivated());
    assert_notify(sub.on_frame(&wire, now), 200, RegEventEffect::None);
    assert!(sub.is_disabled());
    let active = notify(&sub, 2, &active(1), "active;expires=600");
    assert_notify(sub.on_frame(&active, now), 481, RegEventEffect::None);
}

#[test]
fn partial_contact_id_removal_requires_previously_verified_own_mapping() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    establish(&mut sub, now);
    let full = notify(&sub, 1, &active(0), "active;expires=600");
    assert_notify(sub.on_frame(&full, now), 200, RegEventEffect::None);
    let partial = xml_document(1, "partial", AOR, "<contact id=\"c1\" state=\"terminated\" event=\"expired\"/>");
    let wire = notify(&sub, 2, &partial, "active;expires=600");
    assert_notify(sub.on_frame(&wire, now), 200, RegEventEffect::BindingTerminated { reason: RegEventTerminationReason::Expired, retry_after: None });
}

#[test]
fn partial_without_baseline_requests_full_without_revoking() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    establish(&mut sub, now);
    let wire = notify(&sub, 1, &termination(1, "partial"), "active;expires=600");
    assert_notify(sub.on_frame(&wire, now), 200, RegEventEffect::NeedFull);
    assert!(matches!(sub.poll(now), RegEventAction::Begin));
    assert!(!sub.is_disabled());
}

#[test]
fn version_gap_requests_full_then_accepts_full_resynchronization() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    establish(&mut sub, now);
    let full = notify(&sub, 1, &active(10), "active;expires=600");
    sub.on_frame(&full, now);
    let gap = notify(&sub, 2, &termination(12, "partial"), "active;expires=600");
    assert_notify(sub.on_frame(&gap, now), 200, RegEventEffect::NeedFull);
    let old = notify(&sub, 3, &termination(9, "full"), "active;expires=600");
    assert_notify(sub.on_frame(&old, now), 200, RegEventEffect::None);
    let full = notify(&sub, 4, &termination(15, "full"), "active;expires=600");
    assert_notify(sub.on_frame(&full, now), 200, deactivated());
}

#[test]
fn repeated_gaps_disable_observation_instead_of_infinite_resubscribe() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    establish(&mut sub, now);
    for cseq in 1..=2 {
        let wire = notify(&sub, cseq, &termination(cseq + 10, "partial"), "active;expires=600");
        assert_notify(sub.on_frame(&wire, now), 200, RegEventEffect::NeedFull);
    }
    assert!(sub.is_disabled());
}

#[test]
fn stale_duplicate_and_overflow_versions_never_roll_back() {
    let binding = binding();
    let mut snapshot = xml::Snapshot::default();
    assert_eq!(snapshot.apply(&xml::parse(active(u32::MAX).as_bytes()).unwrap(), &binding).unwrap(), RegEventEffect::None);
    for (version, mode) in [(u32::MAX, "full"), (u32::MAX - 1, "full"), (0, "partial")] {
        assert_eq!(snapshot.apply(&xml::parse(termination(version, mode).as_bytes()).unwrap(), &binding).unwrap(), RegEventEffect::None);
    }
    let overflow = active(0).replace("version=\"0\"", "version=\"4294967296\"");
    assert!(xml::parse(overflow.as_bytes()).is_err());
}

#[test]
fn notify_cseq_identical_duplicates_accepted_but_mutation_and_stale_rejected() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    establish(&mut sub, now);
    let first = notify(&sub, 10, &active(0), "active;expires=600");
    assert_notify(sub.on_frame(&first, now), 200, RegEventEffect::None);
    assert_notify(sub.on_frame(&first, now), 200, RegEventEffect::None);
    let changed = notify(&sub, 10, &termination(1, "full"), "active;expires=600");
    assert_notify(sub.on_frame(&changed, now), 400, RegEventEffect::None);
    let stale = notify(&sub, 9, &termination(1, "full"), "active;expires=600");
    assert_notify(sub.on_frame(&stale, now), 500, RegEventEffect::None);
    let next = notify(&sub, 12, &termination(1, "full"), "active;expires=600");
    assert_notify(sub.on_frame(&next, now), 200, deactivated());
}

#[test]
fn unrelated_aor_contact_instance_reg_id_and_port_cannot_revoke() {
    let now = Instant::now();
    for body in [
        termination(0, "full").replace(AOR, "sip:other@ims.example.test"),
        termination(0, "full").replace(CONTACT, "sip:other-flow@192.0.2.20:5062;transport=udp"),
        termination(0, "full").replace("4000-8000-000000000001", "4000-8000-000000000002"),
        termination(0, "full").replace("name=\"reg-id\">1", "name=\"reg-id\">2"),
        termination(0, "full").replace(":5062", ":5063"),
    ] {
        let (mut sub, _) = start(now, SipTransport::Udp);
        establish(&mut sub, now);
        let wire = notify(&sub, 1, &body, "active;expires=600");
        assert_notify(sub.on_frame(&wire, now), 200, RegEventEffect::None);
        assert!(!sub.is_disabled());
    }
}

#[test]
fn missing_instance_or_reg_id_is_not_an_exact_binding_match() {
    let mut snapshot = xml::Snapshot::default();
    let body = xml_document(0, "full", AOR, &format!("<contact id=\"c1\" state=\"terminated\" event=\"expired\"><uri>{CONTACT}</uri></contact>"));
    assert_eq!(snapshot.apply(&xml::parse(body.as_bytes()).unwrap(), &binding()).unwrap(), RegEventEffect::None);
}

#[test]
fn mapped_partial_with_conflicting_explicit_identity_is_rejected_atomically() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    establish(&mut sub, now);
    let full = notify(&sub, 1, &active(0), "active;expires=600");
    sub.on_frame(&full, now);
    let bad = termination(1, "partial").replace(CONTACT, "sip:other@192.0.2.20:5062;transport=udp");
    let wire = notify(&sub, 2, &bad, "active;expires=600");
    assert_notify(sub.on_frame(&wire, now), 400, RegEventEffect::None);
    let good = notify(&sub, 2, &termination(1, "partial"), "active;expires=600");
    assert_notify(sub.on_frame(&good, now), 200, deactivated());
}

#[test]
fn empty_full_requires_prior_own_mapping_and_authoritative_registration_id() {
    let binding = binding();
    let empty = xml_document(0, "full", AOR, "");
    let mut snapshot = xml::Snapshot::default();
    assert_eq!(snapshot.apply(&xml::parse(empty.as_bytes()).unwrap(), &binding).unwrap(), RegEventEffect::None);
    snapshot.apply(&xml::parse(active(1).as_bytes()).unwrap(), &binding).unwrap();
    let unrelated_id = xml_document(2, "full", AOR, "").replace("id=\"r1\"", "id=\"r2\"");
    assert_eq!(snapshot.apply(&xml::parse(unrelated_id.as_bytes()).unwrap(), &binding).unwrap(), RegEventEffect::None);
    snapshot.apply(&xml::parse(active(3).as_bytes()).unwrap(), &binding).unwrap();
    let removal = xml_document(4, "full", AOR, "");
    assert_eq!(snapshot.apply(&xml::parse(removal.as_bytes()).unwrap(), &binding).unwrap(), RegEventEffect::BindingTerminated { reason: RegEventTerminationReason::Unregistered, retry_after: None });
}

#[test]
fn subscription_termination_and_timeout_alone_never_report_binding_loss() {
    let now = Instant::now();
    for state in ["terminated;reason=timeout", "terminated;reason=probation;retry-after=120", "terminated;reason=deactivated"] {
        let (mut sub, _) = start(now, SipTransport::Udp);
        establish(&mut sub, now);
        let wire = notify(&sub, 1, "", state);
        assert_notify(sub.on_frame(&wire, now), 200, RegEventEffect::None);
        assert!(sub.is_disabled());
    }
}

#[test]
fn binding_probation_preserves_retry_after_without_untrusted_reason_text() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    establish(&mut sub, now);
    let body = termination(0, "full").replace("event=\"deactivated\"", "event=\"probation\" retry-after=\"120\"");
    let wire = notify(&sub, 1, &body, "active;expires=600");
    assert_notify(sub.on_frame(&wire, now), 200, RegEventEffect::BindingTerminated { reason: RegEventTerminationReason::Probation, retry_after: Some(120) });
}

#[test]
fn mwi_refer_and_unrelated_sip_are_not_consumed() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    for event in ["message-summary", "refer;id=1"] {
        let wire = String::from_utf8(notify(&sub, 1, &active(0), "active;expires=600")).unwrap().replace("Event: reg", &format!("Event: {event}"));
        assert!(matches!(sub.on_frame(wire.as_bytes(), now), RegEventFrame::Other));
    }
    assert!(matches!(sub.on_frame(b"not SIP", now), RegEventFrame::Other));
}

#[test]
fn event_duplicates_ids_wrong_dialog_and_content_type_are_rejected() {
    let now = Instant::now();
    for (old, new, status) in [
        ("Event: reg\r\n", "Event: reg\r\nEvent: reg\r\n", 400),
        ("Event: reg\r\n", "Event: reg;id=other\r\n", 489),
        ("application/reginfo+xml", "text/plain", 415),
        (";tag=remote", ";tag=wrong", 481),
        ("CSeq: 1 NOTIFY", "CSeq: 2147483648 NOTIFY", 400),
        ("active;expires=600", "active;expires=600;expires=600", 400),
    ] {
        let (mut sub, _) = start(now, SipTransport::Udp);
        establish(&mut sub, now);
        let wire = String::from_utf8(notify(&sub, 1, &active(0), "active;expires=600")).unwrap().replace(old, new);
        assert_notify(sub.on_frame(wire.as_bytes(), now), status, RegEventEffect::None);
    }
}

#[test]
fn namespaced_xml_supports_prefixes_and_bounded_foreign_extensions() {
    let body = active(0).replace("xmlns=", "xmlns:r=").replace("<reginfo", "<r:reginfo").replace("</reginfo", "</r:reginfo")
        .replace("<registration", "<r:registration").replace("</registration", "</r:registration")
        .replace("<contact", "<r:contact").replace("</contact", "</r:contact")
        .replace("<uri", "<r:uri").replace("</uri", "</r:uri")
        .replace("<unknown-param", "<r:unknown-param").replace("</unknown-param", "</r:unknown-param")
        .replace("</r:reginfo>", "<x:extension xmlns:x=\"urn:test\"><x:detail>ignored</x:detail></x:extension></r:reginfo>");
    assert!(xml::parse(body.as_bytes()).is_ok());
}

#[test]
fn xml_rejects_wrong_namespace_dtd_entities_multiple_roots_and_duplicate_fields() {
    let valid = active(0);
    let documents = [
        valid.replace("urn:ietf:params:xml:ns:reginfo", "urn:wrong"),
        format!("<!DOCTYPE reginfo [<!ENTITY injected SYSTEM 'file:///never-read'>]>{valid}"),
        valid.replace(CONTACT, "&injected;"),
        format!("{valid}{valid}"),
        valid.replace("version=\"0\"", "version=\"0\" version=\"1\""),
        valid.replace("</uri>", &format!("</uri><uri>{CONTACT}</uri>")),
        valid.replace("state=\"full\"", "state=\"unknown\""),
        valid.replace("event=\"registered\"", "event=\"expired\""),
        valid.replace("</contact>", "<unknown-param name=\"reg-id\">1</unknown-param></contact>"),
        valid.replace("</contact>", "<extension><uri>sip:hidden@example</uri></extension></contact>"),
        valid.replace("<uri>", "<uri xmlns=\"urn:other\">"),
        valid.replace("version=\"0\"", "xmlns:x=\"urn:ext\" x:version=\"0\" version=\"0\""),
    ];
    for document in documents { assert!(xml::parse(document.as_bytes()).is_err()); }
}

#[test]
fn xml_rejects_duplicate_registration_contact_and_own_identity_ambiguity() {
    let duplicate = contact("c1", CONTACT, "active", "registered").repeat(2);
    assert!(xml::parse(xml_document(0, "full", AOR, &duplicate).as_bytes()).is_err());
    let reg_duplicate = active(0).replace("</reginfo>", &format!("<registration id=\"r1\" aor=\"sip:other@example\" state=\"active\"/></reginfo>"));
    assert!(xml::parse(reg_duplicate.as_bytes()).is_err());
    let ambiguous = format!("{}{}", contact("c1", CONTACT, "active", "registered"), contact("c2", CONTACT, "active", "registered"));
    let document = xml::parse(xml_document(0, "full", AOR, &ambiguous).as_bytes()).unwrap();
    assert!(xml::Snapshot::default().apply(&document, &binding()).is_err());
}

#[test]
fn xml_body_depth_node_and_contact_bounds_are_enforced() {
    assert!(xml::parse(&vec![b' '; MAX_BODY + 1]).is_err());
    let deep = format!("<reginfo xmlns=\"urn:ietf:params:xml:ns:reginfo\" version=\"0\" state=\"full\">{}{}{}</reginfo>", "<ext>".repeat(16), "x", "</ext>".repeat(16));
    assert!(xml::parse(deep.as_bytes()).is_err());
    let nodes = format!("<reginfo xmlns=\"urn:ietf:params:xml:ns:reginfo\" version=\"0\" state=\"full\">{}</reginfo>", "<ext/>".repeat(1025));
    assert!(xml::parse(nodes.as_bytes()).is_err());
    let contacts: String = (0..257).map(|i| format!("<contact id=\"{i}\" state=\"terminated\" event=\"expired\"/>")).collect();
    assert!(xml::parse(xml_document(0, "partial", AOR, &contacts).as_bytes()).is_err());
}

#[test]
fn malformed_frame_lengths_duplicate_headers_and_invalid_utf8_are_reg_errors() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    let wire = notify(&sub, 1, &active(0), "active;expires=600");
    let duplicate = String::from_utf8(wire.clone()).unwrap().replace("Content-Length:", "Content-Length: 0\r\nContent-Length:");
    assert_notify(sub.on_frame(duplicate.as_bytes(), now), 400, RegEventEffect::None);
    let mut trailing = wire.clone(); trailing.push(b'x');
    assert_notify(sub.on_frame(&trailing, now), 400, RegEventEffect::None);
    let mut invalid_utf8 = wire; *invalid_utf8.last_mut().unwrap() = 255;
    assert_notify(sub.on_frame(&invalid_utf8, now), 400, RegEventEffect::None);
}

fn challenge_header(proxy: bool) -> &'static str {
    if proxy { "Proxy-Authenticate: Digest realm=\"ims.example.test\", nonce=\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\", algorithm=AKAv1-MD5, qop=\"auth\"\r\n" }
    else { "WWW-Authenticate: Digest realm=\"ims.example.test\", nonce=\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\", algorithm=AKAv1-MD5, qop=\"auth\"\r\n" }
}
fn answer_challenge(sub: &mut RegEventSubscription, now: Instant, proxy: bool) -> Vec<u8> {
    let header = if proxy { "Proxy-Authorization: Digest response=\"fresh\"" } else { "Authorization: Digest response=\"fresh\"" };
    sub.build_request(&route(SipTransport::Udp), &registration(), "test", &[], Some(header), now).unwrap()
}

#[test]
fn both_401_and_407_have_two_auth_rounds_and_fresh_cseq_branch() {
    let now = Instant::now();
    for proxy in [false, true] {
        let status = if proxy { 407 } else { 401 };
        let (mut sub, first) = start(now, SipTransport::Udp);
        let mut old = first;
        for round in 1..=2 {
            let challenge = response(&sub, status, challenge_header(proxy));
            assert!(matches!(sub.on_frame(&challenge, now), RegEventFrame::AuthChallenge { .. }));
            assert_eq!(sub.auth_rounds, round);
            let next = answer_challenge(&mut sub, now, proxy);
            assert_ne!(Frame::parse(&old).unwrap().one("via").unwrap(), Frame::parse(&next).unwrap().one("via").unwrap());
            assert_eq!(sub.cseq, u32::from(round) + 1);
            old = next;
        }
        let third = response(&sub, status, challenge_header(proxy));
        assert!(matches!(sub.on_frame(&third, now), RegEventFrame::Response { .. }));
        assert!(sub.is_disabled());
    }
}

#[test]
fn renewal_resets_auth_budget_without_replaying_old_authorization() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    let challenge = response(&sub, 401, challenge_header(false));
    sub.on_frame(&challenge, now);
    answer_challenge(&mut sub, now, false);
    establish(&mut sub, now);
    let renewal = now + Duration::from_secs(300);
    let wire = sub.build_request(&route(SipTransport::Udp), &registration(), "test", &[], None, renewal).unwrap();
    assert_eq!(Frame::parse(&wire).unwrap().one("authorization").unwrap(), None);
    assert_eq!(sub.auth_rounds, 0);
    let challenge = response(&sub, 401, challenge_header(false));
    assert!(matches!(sub.on_frame(&challenge, renewal), RegEventFrame::AuthChallenge { .. }));
}

#[test]
fn unsupported_security_auth_scope_algorithm_and_423_disable_only_observation() {
    let now = Instant::now();
    for (status, headers) in [
        (423, "Min-Expires: 7200\r\n".to_string()),
        (503, "Retry-After: 600\r\n".to_string()),
        (401, format!("{}Security-Server: ipsec-3gpp\r\n", challenge_header(false))),
        (401, challenge_header(false).replace("AKAv1-MD5", "MD5")),
        (401, format!("{}{}", challenge_header(false), challenge_header(true))),
    ] {
        let (mut sub, _) = start(now, SipTransport::Udp);
        let wire = response(&sub, status, &headers);
        assert!(matches!(sub.on_frame(&wire, now), RegEventFrame::Response { .. }));
        assert!(sub.is_disabled());
    }
    let (mut sub, _) = start(now, SipTransport::Udp);
    let first = response(&sub, 401, challenge_header(false));
    sub.on_frame(&first, now);
    answer_challenge(&mut sub, now, false);
    let switched = response(&sub, 407, challenge_header(true));
    assert!(matches!(sub.on_frame(&switched, now), RegEventFrame::Response { status: 407 }));
    assert!(sub.is_disabled());
}

#[test]
fn pending_auth_times_out_and_old_response_cannot_reset_budget() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    let first = response(&sub, 401, challenge_header(false));
    sub.on_frame(&first, now);
    assert!(matches!(sub.on_frame(&first, now), RegEventFrame::Other));
    assert_eq!(sub.auth_rounds, 1);
    sub.poll(now + TRANSACTION_TIMEOUT);
    assert!(sub.is_disabled());
    assert!(sub.build_request(&route(SipTransport::Udp), &registration(), "test", &[], Some("Authorization: Digest response=\"late\""), now + TRANSACTION_TIMEOUT).is_err());
}

#[test]
fn unsubscribed_classifier_rejects_reg_without_borrowing_a_binding() {
    let now = Instant::now();
    let (sub, _) = start(now, SipTransport::Udp);
    let wire = notify(&sub, 1, &active(0), "active;expires=600");
    assert_eq!(classify_unsubscribed_notify(&wire), Some(481));
    let text = String::from_utf8(wire).unwrap();
    assert_eq!(classify_unsubscribed_notify(text.replace("Event: reg", "Event: message-summary").as_bytes()), None);
    assert_eq!(classify_unsubscribed_notify(text.replace("Event: reg", "Event: reg\r\nEvent: reg").as_bytes()), Some(400));
}

#[test]
fn unrelated_nonempty_full_after_baseline_does_not_infer_own_disappearance() {
    let binding = binding();
    for body in [
        termination(1, "full").replace(CONTACT, "sip:other@192.0.2.20:5062;transport=udp"),
        termination(1, "full").replace("name=\"reg-id\">1", "name=\"reg-id\">2"),
    ] {
        let mut snapshot = xml::Snapshot::default();
        snapshot.apply(&xml::parse(active(0).as_bytes()).unwrap(), &binding).unwrap();
        assert_eq!(snapshot.apply(&xml::parse(body.as_bytes()).unwrap(), &binding).unwrap(), RegEventEffect::None);
    }
}

#[test]
fn early_partial_need_full_survives_subscribe_success_race() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    let wire = notify(&sub, 1, &termination(1, "partial"), "active;expires=600");
    assert_notify(sub.on_frame(&wire, now), 200, RegEventEffect::NeedFull);
    establish(&mut sub, now);
    assert!(matches!(sub.poll(now), RegEventAction::Begin));
}

#[test]
fn subscription_retry_after_is_retained_only_with_proven_binding_termination() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    establish(&mut sub, now);
    let wire = notify(&sub, 1, &termination(0, "full"), "terminated;reason=probation;retry-after=300");
    assert_notify(sub.on_frame(&wire, now), 200, RegEventEffect::BindingTerminated {
        reason: RegEventTerminationReason::Deactivated, retry_after: Some(300),
    });
}

#[test]
fn xml_rejects_invalid_character_syntax_and_misplaced_declarations() {
    for body in [
        active(0).replace(CONTACT, "sip:bad]]>@example"),
        format!(" <!--comment--><?xml version=\"1.0\"?>{}", active(0)),
        active(0).replace(CONTACT, "sip:bad&#0;@example"),
        active(0).replace("id=\"c1\"", "id=\"bad<id\""),
    ] { assert!(xml::parse(body.as_bytes()).is_err()); }
    let declared = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>{}", active(0));
    assert!(xml::parse(declared.as_bytes()).is_ok());
}

#[test]
fn active_notify_cannot_extend_subscription_deadline_or_register_lease() {
    let now = Instant::now();
    let (mut sub, _) = start(now, SipTransport::Udp);
    establish(&mut sub, now);
    let deadline = sub.lease_deadline;
    let wire = notify(&sub, 1, &active(0), "active;expires=4294967295");
    assert_notify(sub.on_frame(&wire, now + Duration::from_secs(10)), 200, RegEventEffect::None);
    assert_eq!(sub.lease_deadline, deadline);
    assert!(matches!(sub.poll(now + Duration::from_secs(600)), RegEventAction::Idle));
    assert!(sub.is_disabled());
}
