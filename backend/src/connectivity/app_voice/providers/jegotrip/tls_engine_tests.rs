//! Real in-memory rustls client/server exchanges; certificates generated only by
//! the Actions test process. No user SDK keys, target library or network peer.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    arc_protocol::ArcProtocol, arc_session::{ArcLoginInputs, ArcPhase}, controller::Callback,
    offline_backend::{OfflineBackend, OfflineEvent}, packet_stream::{encode_packet_stream_frame, PacketStreamDecoder},
    rpc_envelope::{decode_reply, decode_request, encode_reply}, rpc_stream::Writer,
    session::HostingSession, tests::imported, transport_packets::{decode_tls_carrier, encode_call_packets, encode_tls_carrier, CallKind},
};
use std::{path::PathBuf, process::{Command, Stdio}, sync::atomic::{AtomicU64, Ordering}};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

static NEXT_TEST: AtomicU64 = AtomicU64::new(1);
pub(crate) struct Certificates { directory: PathBuf, pub(crate) root: String, certificate: CertificateDer<'static>, key: Vec<u8> }
impl Drop for Certificates { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.directory); } }
impl Certificates {
    pub(crate) fn generate(san: &str) -> Self {
        let directory = std::env::temp_dir().join(format!("simadmin-jegotrip-tls-{}-{}", std::process::id(), NEXT_TEST.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&directory).unwrap();
        let run = |arguments: &[&str]| {
            assert!(Command::new("openssl").args(arguments).current_dir(&directory)
                .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().expect("Actions openssl").success());
        };
        run(&["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", "key.pem", "-out", "cert.pem", "-days", "1", "-subj", "/CN=fixture.invalid",
            "-addext", &format!("subjectAltName={san}"), "-addext", "basicConstraints=critical,CA:FALSE", "-addext", "keyUsage=critical,digitalSignature,keyEncipherment", "-addext", "extendedKeyUsage=serverAuth"]);
        run(&["x509", "-in", "cert.pem", "-outform", "DER", "-out", "cert.der"]);
        run(&["pkcs8", "-topk8", "-nocrypt", "-in", "key.pem", "-outform", "DER", "-out", "key.der"]);
        let der = std::fs::read(directory.join("cert.der")).unwrap(); let root = STANDARD.encode(&der);
        Self { certificate: CertificateDer::from(der), key: std::fs::read(directory.join("key.der")).unwrap(), root, directory }
    }
    pub(crate) fn server_config(&self, version: &'static rustls::SupportedProtocolVersion) -> Arc<rustls::ServerConfig> {
        Arc::new(rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[version]).unwrap().with_no_client_auth()
            .with_single_cert(vec![self.certificate.clone()], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone()))).unwrap())
    }
    pub(crate) fn server(&self, version: &'static rustls::SupportedProtocolVersion) -> rustls::ServerConnection {
        rustls::ServerConnection::new(self.server_config(version)).unwrap()
    }
}
fn owner() -> AttemptOwner {
    let mut session = HostingSession::new("fixture-line".into()); session.import(imported(), 100).unwrap(); session.begin_login(100).unwrap()
}
fn parameters<'a>(authority: &'a str, root: &'a str) -> TlsParameters<'a> {
    TlsParameters { authority, root_der_base64: root, tls_only: true, enable_sni: true,
        handshake_timeout: Duration::from_secs(3), partial_timeout: Duration::from_secs(1) }
}
fn raw_wire(wire: &[OwnedWireFrames], original: &AttemptOwner, scope: &RpcConnectionScope) -> Vec<u8> {
    let mut decoder = PacketStreamDecoder::default(); let mut out = vec![];
    for item in wire {
        for frame in item.frames_for(original, scope).unwrap() {
            for carrier in decoder.on_bytes(frame.expose_to_transport()).unwrap() {
                out.extend_from_slice(decode_tls_carrier(carrier.expose_to_transport()).unwrap());
            }
        }
    }
    assert!(!decoder.has_partial()); out
}
pub(crate) fn server_input(server: &mut rustls::ServerConnection, bytes: &[u8]) {
    let mut cursor = Cursor::new(bytes);
    while cursor.position() < bytes.len() as u64 {
        assert!(server.read_tls(&mut cursor).unwrap() > 0); server.process_new_packets().unwrap();
    }
}
pub(crate) fn server_output(server: &mut rustls::ServerConnection) -> Vec<u8> {
    let mut out = vec![]; while server.wants_write() { assert!(server.write_tls(&mut out).unwrap() > 0); } out
}
pub(crate) fn server_plaintext(server: &mut rustls::ServerConnection) -> Vec<u8> {
    let mut plaintext = Vec::new(); let mut buffer = [0; 16_384];
    loop {
        match server.reader().read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => plaintext.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("memory TLS reader failed: {error:?}"),
        }
    }
    plaintext
}
fn frame_cipher(bytes: &[u8], carrier_size: usize) -> Vec<u8> {
    bytes.chunks(carrier_size).flat_map(|chunk| {
        encode_packet_stream_frame(encode_tls_carrier(chunk).unwrap().expose_to_transport()).unwrap().expose_to_transport().to_vec()
    }).collect()
}
fn receive_all(engine: &mut TlsEngine, original: &AttemptOwner, scope: &RpcConnectionScope, ciphertext: &[u8],
    carrier_size: usize, wire_size: usize, now: Instant) -> Result<(Vec<TransportBytes>, Vec<OwnedWireFrames>), TlsError> {
    let bytes = frame_cipher(ciphertext, carrier_size); let mut plain = Vec::new(); let mut output = Vec::new();
    for chunk in bytes.chunks(wire_size) {
        let progress = engine.on_wire_bytes(original, scope, chunk, None, now)?.unwrap();
        if let Some(packets) = progress.plaintext.packets_for(original, scope) {
            plain.extend(packets.iter().map(|p| TransportBytes::from_bytes(p.expose_to_transport().to_vec())));
        }
        output.extend(progress.wire);
    }
    Ok((plain, output))
}
pub(crate) fn handshake(engine: &mut TlsEngine, original: &AttemptOwner, scope: &RpcConnectionScope, server: &mut rustls::ServerConnection, now: Instant)
    -> Result<(), TlsError> {
    let mut outgoing = engine.take_wire_output(original, scope, None, now)?;
    for _ in 0..20 {
        let raw = raw_wire(&outgoing, original, scope); if !raw.is_empty() { server_input(server, &raw); }
        let raw = server_output(server); outgoing = Vec::new();
        if !raw.is_empty() { outgoing = receive_all(engine, original, scope, &raw, 127, 97, now)?.1; }
        if engine.phase() == TlsPhase::Verified && !server.is_handshaking() && outgoing.is_empty() { return Ok(()); }
    }
    panic!("bounded in-memory TLS handshake did not finish")
}
fn setup(certificates: &Certificates, version: &'static rustls::SupportedProtocolVersion) -> (ArcProtocol, AttemptOwner, RpcConnectionScope, TlsEngine, rustls::ServerConnection, Instant) {
    let original = owner(); let mut protocol = ArcProtocol::new(original.clone()); let scope = protocol.connection_scope(); let now = Instant::now();
    let mut engine = protocol.take_tls_engine(PacketRoute::DirectTlsCarrier, parameters("fixture.invalid", &certificates.root), now).unwrap();
    let mut server = certificates.server(version); handshake(&mut engine, &original, &scope, &mut server, now).unwrap();
    (protocol, original, scope, engine, server, now)
}

#[test]
fn ciphertext_and_handshake_output_are_bounded_before_unbounded_allocation() {
    let mut output = BoundedTlsOutput(Vec::new());
    assert!(output.write(&vec![0; MAX_TLS_IO_BYTES + 1]).is_err()); assert!(output.0.is_empty());
    assert_eq!(output.write(&vec![0; MAX_TLS_IO_BYTES]).unwrap(), MAX_TLS_IO_BYTES);
    assert!(output.write(&[0]).is_err()); assert_eq!(output.0.len(), MAX_TLS_IO_BYTES);
    let mut records = CipherRecords::default();
    assert_eq!(records.feed(&[22, 3, 3, 0xff, 0xff]).err(), Some(TlsError::TooLarge));
    let mut records = CipherRecords::default();
    assert_eq!(records.feed(&vec![0; MAX_TLS_IO_BYTES + 1]).err(), Some(TlsError::TooLarge));
    let mut records = CipherRecords::default();
    assert_eq!(records.feed(&[22, 3, 3, 0, 0].repeat(MAX_TLS_RECORDS_PER_STEP + 1)).err(), Some(TlsError::TooLarge));
}

#[test]
fn tls12_and_tls13_validate_imported_root_authority_and_keep_sdk_login_separate() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        let (protocol, _, _, engine, server, _) = setup(&cert, version);
        assert_eq!(engine.phase(), TlsPhase::Verified); assert!(!server.is_handshaking());
        assert_eq!(protocol.phase(), ArcPhase::Initial);
    }
}

#[test]
fn missing_weak_or_invalid_tls_configuration_never_consumes_original_transport_slot() {
    let original = owner(); let mut protocol = ArcProtocol::new(original); let now = Instant::now();
    for authority in ["", "scheme://fixture.invalid", "*.fixture.invalid", "fixture.invalid:443", "a..b", "a b", "fixture.invalid."] {
        assert_eq!(protocol.take_tls_engine(PacketRoute::DirectTlsCarrier, parameters(authority, "AA=="), now).err(), Some(TlsError::InvalidConfiguration));
    }
    for root in ["", "not_base64", "AA=="] {
        assert_eq!(protocol.take_tls_engine(PacketRoute::DirectTlsCarrier, parameters("fixture.invalid", root), now).err(), Some(TlsError::InvalidConfiguration));
    }
    let mut p = parameters("fixture.invalid", "AA=="); p.tls_only = false;
    assert_eq!(protocol.take_tls_engine(PacketRoute::DirectTlsCarrier, p, now).err(), Some(TlsError::InvalidConfiguration));
    assert!(protocol.take_packet_transport(PacketRoute::DirectTlsCarrier).is_ok());
}

#[test]
fn wrong_dns_name_untrusted_root_and_ip_without_ip_san_fail_before_plaintext_delivery() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let other = Certificates::generate("DNS:fixture.invalid");
    for (authority, root) in [("other.invalid", &cert.root), ("fixture.invalid", &other.root), ("127.0.0.1", &cert.root)] {
        let original = owner(); let mut protocol = ArcProtocol::new(original.clone()); let scope = protocol.connection_scope(); let now = Instant::now();
        let mut engine = protocol.take_tls_engine(PacketRoute::DirectTlsCarrier, parameters(authority, root), now).unwrap();
        let mut server = cert.server(&rustls::version::TLS13);
        assert_eq!(handshake(&mut engine, &original, &scope, &mut server, now), Err(TlsError::HandshakeFailed));
        assert_eq!(engine.phase(), TlsPhase::Closed); assert_eq!(protocol.phase(), ArcPhase::Closed);
    }
}

#[test]
fn wildcard_dns_is_rejected_even_when_standard_https_matching_accepts_it() {
    let cert = Certificates::generate("DNS:*.fixture.invalid"); let original = owner(); let mut protocol = ArcProtocol::new(original.clone());
    let scope = protocol.connection_scope(); let now = Instant::now();
    let mut engine = protocol.take_tls_engine(PacketRoute::DirectTlsCarrier, parameters("host.fixture.invalid", &cert.root), now).unwrap();
    let mut server = cert.server(&rustls::version::TLS13);
    assert_eq!(handshake(&mut engine, &original, &scope, &mut server, now), Err(TlsError::HandshakeFailed));
    assert_eq!(protocol.phase(), ArcPhase::Closed);
}

#[test]
fn exact_ip_san_is_verified_without_substituting_a_dns_endpoint_identity() {
    let cert = Certificates::generate("IP:127.0.0.1"); let original = owner(); let mut protocol = ArcProtocol::new(original.clone());
    let scope = protocol.connection_scope(); let now = Instant::now();
    let mut engine = protocol.take_tls_engine(PacketRoute::DirectTlsCarrier, parameters("127.0.0.1", &cert.root), now).unwrap();
    let mut server = cert.server(&rustls::version::TLS13); handshake(&mut engine, &original, &scope, &mut server, now).unwrap();
    assert_eq!(engine.phase(), TlsPhase::Verified); assert!(server.server_name().is_none());
}

#[test]
fn sni_is_original_explicit_policy_and_no_application_data_is_queued_before_verified() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner(); let mut protocol = ArcProtocol::new(original.clone());
    let scope = protocol.connection_scope(); let now = Instant::now(); let mut p = parameters("fixture.invalid", &cert.root); p.enable_sni = false;
    let mut engine = protocol.take_tls_engine(PacketRoute::DirectTlsCarrier, p, now).unwrap();
    let packets = encode_call_packets(CallKind::Ping, &[], TLS_FRAGMENT_THRESHOLD).unwrap();
    assert_eq!(engine.encrypt_packets(&original, &scope, &packets, None, now).err(), Some(TlsError::NotReady));
    let wire = engine.take_wire_output(&original, &scope, None, now).unwrap(); let raw = raw_wire(&wire, &original, &scope); assert_eq!(raw[0], 22);
    let mut server = cert.server(&rustls::version::TLS13); server_input(&mut server, &raw); assert!(server.server_name().is_none());
    assert_eq!(protocol.phase(), ArcPhase::Initial);
}

#[test]
fn client_emits_one_canonical_common_packet_per_tls_writer_call_not_a_fake_inner_frame() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (_protocol, original, scope, mut engine, mut server, now) = setup(&cert, &rustls::version::TLS13);
    let result = encode_reply(7, 0, &[0, 3, 14, 0xff]).unwrap(); let packets = encode_call_packets(CallKind::Reply, result.expose_to_transport(), TLS_FRAGMENT_THRESHOLD).unwrap();
    let wire = engine.encrypt_packets(&original, &scope, &packets, None, now).unwrap().unwrap(); server_input(&mut server, &raw_wire(&wire, &original, &scope));
    let ciphertext = raw_wire(&wire, &original, &scope);
    assert_eq!(CipherRecords::default().feed(&ciphertext).unwrap().len(), 1);
    let plaintext = server_plaintext(&mut server);
    assert_eq!(plaintext, packets[0].expose_to_transport()); assert_eq!(plaintext[0], 1); assert_eq!(plaintext[1], 0);
}

#[test]
fn maximum_unfragmented_native_packet_fits_one_tls_record_with_split_outer_carriers() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_protocol, original, scope, mut engine, mut server, now) = setup(&cert, &rustls::version::TLS13);
    let result = encode_reply(7, 0, &vec![0xff; TLS_FRAGMENT_THRESHOLD - 13]).unwrap();
    assert_eq!(result.expose_to_transport().len(), TLS_FRAGMENT_THRESHOLD);
    let packets = encode_call_packets(CallKind::Reply, result.expose_to_transport(), TLS_FRAGMENT_THRESHOLD).unwrap();
    assert_eq!(packets[0].expose_to_transport().len(), TLS_FRAGMENT_THRESHOLD + 1);
    let wire = engine.encrypt_packets(&original, &scope, &packets, None, now).unwrap().unwrap();
    let raw = raw_wire(&wire, &original, &scope); assert_eq!(CipherRecords::default().feed(&raw).unwrap().len(), 1);
    assert!(wire.len() >= 2, "record ciphertext exceeds native16384 flush carrier");
    server_input(&mut server, &raw); assert_eq!(server_plaintext(&mut server), packets[0].expose_to_transport());
}

#[test]
fn split_tls_records_and_coalesced_plaintext_preserve_each_call_packet_and_binary_fields() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (_protocol, original, scope, mut engine, mut server, now) = setup(&cert, &rustls::version::TLS13);
    let first = encode_reply(1, 0, &[0, 3, 14, 0xff]).unwrap(); let second = encode_reply(2, 0, &[0, 1, 0, 0, 0, 0]).unwrap();
    let mut packets = encode_call_packets(CallKind::Reply, first.expose_to_transport(), TLS_FRAGMENT_THRESHOLD).unwrap();
    packets.extend(encode_call_packets(CallKind::Reply, second.expose_to_transport(), TLS_FRAGMENT_THRESHOLD).unwrap());
    let plain = packets.iter().flat_map(|p| p.expose_to_transport().to_vec()).collect::<Vec<_>>();
    server.writer().write_all(&plain[..3]).unwrap(); server.writer().write_all(&plain[3..]).unwrap();
    let ciphertext = server_output(&mut server); let (decoded, _) = receive_all(&mut engine, &original, &scope, &ciphertext, 17, 11, now).unwrap();
    assert_eq!(decoded.iter().map(|p| p.expose_to_transport()).collect::<Vec<_>>(), packets.iter().map(|p| p.expose_to_transport()).collect::<Vec<_>>());
}

#[test]
fn native_balanced_common_fragments_cross_record_and_carrier_boundaries_without_false_ready() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (protocol, original, scope, mut engine, mut server, now) = setup(&cert, &rustls::version::TLS13);
    let reply = encode_reply(1, 0, &vec![0xff; 30_000]).unwrap(); let packets = encode_call_packets(CallKind::Reply, reply.expose_to_transport(), TLS_FRAGMENT_THRESHOLD).unwrap();
    let plain = packets.iter().flat_map(|p| p.expose_to_transport().to_vec()).collect::<Vec<_>>(); server.writer().write_all(&plain).unwrap();
    let ciphertext = server_output(&mut server); let (decoded, _) = receive_all(&mut engine, &original, &scope, &ciphertext, 512, 333, now).unwrap();
    assert_eq!(decoded.len(), packets.len()); assert_eq!(decoded.iter().flat_map(|p| p.expose_to_transport().to_vec()).collect::<Vec<_>>(), plain);
    assert_eq!(protocol.phase(), ArcPhase::Initial);
}

#[test]
fn stale_owner_or_scope_is_ignored_before_clock_ciphertext_and_partial_state() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (protocol, original, scope, mut engine, _, now) = setup(&cert, &rustls::version::TLS13);
    assert!(engine.on_wire_bytes(&owner(), &scope, &[0xff], None, now - Duration::from_secs(10)).unwrap().is_none());
    assert!(engine.on_wire_bytes(&original, &RpcConnectionScope::fresh(), &[0xff], None, now - Duration::from_secs(10)).unwrap().is_none());
    assert!(!engine.tick(&owner(), &scope, now + Duration::from_secs(100)).unwrap());
    assert_eq!(engine.phase(), TlsPhase::Verified); assert_eq!(protocol.phase(), ArcPhase::Initial);
}

#[test]
fn handshake_deadline_and_partial_input_deadline_do_not_extend_on_drip_feed() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner(); let mut protocol = ArcProtocol::new(original.clone());
    let scope = protocol.connection_scope(); let now = Instant::now();
    let mut engine = protocol.take_tls_engine(PacketRoute::DirectTlsCarrier, parameters("fixture.invalid", &cert.root), now).unwrap();
    let wire = engine.take_wire_output(&original, &scope, None, now).unwrap();
    assert_eq!(engine.tick(&original, &scope, now + Duration::from_secs(3)), Err(TlsError::Timeout));
    assert!(wire[0].frames_for(&original, &scope).is_none()); assert_eq!(protocol.phase(), ArcPhase::Closed);
    let (protocol, original, scope, mut engine, _, now) = setup(&cert, &rustls::version::TLS13);
    engine.on_wire_bytes(&original, &scope, &[0], None, now).unwrap();
    engine.on_wire_bytes(&original, &scope, &[0], None, now + Duration::from_millis(999)).unwrap();
    assert_eq!(engine.tick(&original, &scope, now + Duration::from_secs(1)), Err(TlsError::Timeout));
    assert_eq!(protocol.phase(), ArcPhase::Closed);
}

#[test]
fn corrupted_ciphertext_unknown_tls_record_or_unverified_close_never_delivers_rpc() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_protocol, original, scope, mut engine, mut server, now) = setup(&cert, &rustls::version::TLS13);
    server.writer().write_all(&[2]).unwrap(); let mut ciphertext = server_output(&mut server); let end = ciphertext.len() - 1; ciphertext[end] ^= 1;
    assert_eq!(receive_all(&mut engine, &original, &scope, &ciphertext, 512, 999, now).err(), Some(TlsError::HandshakeFailed));
    assert_eq!(engine.phase(), TlsPhase::Closed);
    let (_protocol, original, scope, mut engine, _, now) = setup(&cert, &rustls::version::TLS13);
    assert!(receive_all(&mut engine, &original, &scope, &[24, 3, 3, 0, 0], 512, 999, now).is_err());
    assert_eq!(engine.phase(), TlsPhase::Closed);
}

#[test]
fn malformed_later_authenticated_plaintext_closes_original_scope_and_suppresses_step_delivery() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (protocol, original, scope, mut engine, mut server, now) = setup(&cert, &rustls::version::TLS13);
    server.writer().write_all(&[2, 0xff]).unwrap(); let raw = server_output(&mut server);
    assert_eq!(receive_all(&mut engine, &original, &scope, &raw, 512, 999, now).err(), Some(TlsError::Transport(TransportError::UnsupportedKind)));
    assert_eq!(protocol.phase(), ArcPhase::Closed);
}

#[test]
fn tls_close_notify_plaintext_partial_eof_and_clock_rollback_revoke_original_scope() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (protocol, original, scope, mut engine, mut server, now) = setup(&cert, &rustls::version::TLS13);
    server.send_close_notify(); let raw = server_output(&mut server); assert_eq!(receive_all(&mut engine, &original, &scope, &raw, 512, 999, now).err(), Some(TlsError::Closed));
    assert_eq!(protocol.phase(), ArcPhase::Closed);
    let (protocol, original, scope, mut engine, _, now) = setup(&cert, &rustls::version::TLS13);
    engine.on_wire_bytes(&original, &scope, &[0, 0], None, now).unwrap();
    assert_eq!(engine.finish_eof(&original, &scope), Err(TlsError::Transport(TransportError::Truncated))); assert_eq!(protocol.phase(), ArcPhase::Closed);
    let (protocol, original, scope, mut engine, _, now) = setup(&cert, &rustls::version::TLS13);
    assert_eq!(engine.tick(&original, &scope, now - Duration::from_millis(1)), Err(TlsError::ClockRollback)); assert_eq!(protocol.phase(), ArcPhase::Closed);
}

#[test]
fn invalid_outgoing_group_does_not_emit_or_consume_tls_state_and_drop_invalidates_wire() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (protocol, original, scope, mut engine, _, now) = setup(&cert, &rustls::version::TLS13);
    for raw in [vec![2, 14], vec![1], vec![0xff]] {
        assert!(engine.encrypt_packets(&original, &scope, &[TransportBytes::from_bytes(raw)], None, now).is_err());
        assert_eq!(engine.phase(), TlsPhase::Verified);
    }
    let wire = engine.encrypt_packets(&original, &scope, &encode_call_packets(CallKind::Ping, &[], TLS_FRAGMENT_THRESHOLD).unwrap(), None, now).unwrap().unwrap();
    drop(engine); assert!(wire[0].frames_for(&original, &scope).is_none()); assert_eq!(protocol.phase(), ArcPhase::Closed);
}

#[test]
fn verified_tls_login_roundtrip_reaches_only_original_offline_sdk_callback() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner(); let mut backend = OfflineBackend::new(original.clone());
    let scope = backend.connection_scope(); let now = Instant::now();
    let mut engine = backend.take_tls_engine(PacketRoute::DirectTlsCarrier, parameters("fixture.invalid", &cert.root), now).unwrap();
    let mut server = cert.server(&rustls::version::TLS13); handshake(&mut engine, &original, &scope, &mut server, now).unwrap();
    assert_eq!(backend.phase(), ArcPhase::Initial);
    let login = backend.prepare_login(ArcLoginInputs { typed_identity: b"username:fixture", domain_name: b"fixture.invalid", selected_session_id: b"fixture-selector",
        phone_login_token: b"fixture-pwd", application_key: b"fixture-key", domain_id: b"fixture-domain", application_id: b"fixture-app", device_info_cc: b"", client_properties: &[] }, &[]).unwrap();
    let wire = engine.encrypt_packets(&original, &scope, &login.packets, None, now).unwrap().unwrap(); server_input(&mut server, &raw_wire(&wire, &original, &scope));
    let raw = server_plaintext(&mut server);
    let request = decode_request(&raw[1..]).unwrap(); assert_eq!(request.metadata.iter().find(|p| p.key == b"pwd").unwrap().value, b"fixture-pwd");
    let mut result = Writer::new(); result.boolean(true).unwrap(); result.bytes(b"fixture-user").unwrap(); result.bytes(b"fixture-auth").unwrap(); result.properties(&[]).unwrap(); result.bytes(b"").unwrap(); let result = result.finish();
    let response = encode_reply(login.call_id, 0, result.expose_to_transport()).unwrap();
    let plain = encode_call_packets(CallKind::Reply, response.expose_to_transport(), TLS_FRAGMENT_THRESHOLD).unwrap(); server.writer().write_all(plain[0].expose_to_transport()).unwrap();
    let raw = server_output(&mut server); let wire = frame_cipher(&raw, 512);
    let progress = engine.on_wire_bytes(&original, &scope, &wire, None, now).unwrap().unwrap();
    let callbacks = backend.on_verified_plaintext(&original, &scope, progress.plaintext, 0).unwrap();
    assert_eq!(callbacks.len(), 1); assert!(matches!(&callbacks[0], OfflineEvent::Callback(c) if c.owner == original && c.callback == (Callback::SdkLogin { success: true })));
    assert_eq!(backend.phase(), ArcPhase::Authenticated);
    assert_eq!(decode_reply(response.expose_to_transport()).unwrap().call_id, login.call_id);
    assert!(!format!("{engine:?}").contains("fixture")); assert!(!format!("{backend:?}").contains("fixture"));
}

pub(crate) fn endpoint_raw_wire(wire: &[OwnedWireFrames], original: &AttemptOwner, scope: &RpcConnectionScope) -> Vec<u8> {
    wire.iter().flat_map(|item| item.frames_for(original, scope).unwrap().iter()
        .flat_map(|frame| frame.expose_to_transport().to_vec())).collect()
}
fn endpoint_receive(engine: &mut EndpointTlsEngine, original: &AttemptOwner, scope: &RpcConnectionScope,
    ciphertext: &[u8], chunk_size: usize, now: Instant) -> Result<(Vec<TransportBytes>, Vec<OwnedWireFrames>), TlsError> {
    let mut packets = Vec::new(); let mut output = Vec::new();
    for chunk in ciphertext.chunks(chunk_size) {
        let progress = engine.on_wire_bytes(original, scope, chunk, now)?.unwrap();
        for packet in progress.plaintext.packets_for(original, scope).unwrap() {
            packets.push(TransportBytes::from_bytes(packet.expose_to_transport().to_vec()));
        }
        output.extend(progress.wire);
    }
    Ok((packets, output))
}
fn endpoint_handshake(engine: &mut EndpointTlsEngine, original: &AttemptOwner, scope: &RpcConnectionScope,
    server: &mut rustls::ServerConnection, now: Instant) -> Result<(), TlsError> {
    let mut outgoing = engine.take_wire_output(original, scope, now)?;
    for _ in 0..20 {
        let raw = endpoint_raw_wire(&outgoing, original, scope); if !raw.is_empty() { server_input(server, &raw); }
        let raw = server_output(server); outgoing = Vec::new();
        if !raw.is_empty() { outgoing = endpoint_receive(engine, original, scope, &raw, 97, now)?.1; }
        if engine.phase() == TlsPhase::Verified && !server.is_handshaking() && outgoing.is_empty() { return Ok(()); }
    }
    panic!("bounded raw endpoint TLS handshake did not finish")
}
pub(crate) fn endpoint_setup(cert: &Certificates, version: &'static rustls::SupportedProtocolVersion)
    -> (ArcProtocol, AttemptOwner, RpcConnectionScope, EndpointTlsEngine, rustls::ServerConnection, Instant) {
    let original = owner(); let protocol = ArcProtocol::new(original.clone()); let scope = protocol.connection_scope(); let now = Instant::now();
    let mut engine = EndpointTlsEngine::for_original(original.clone(), scope.clone(), parameters("fixture.invalid", &cert.root), now).unwrap();
    let mut server = cert.server(version); endpoint_handshake(&mut engine, &original, &scope, &mut server, now).unwrap();
    (protocol, original, scope, engine, server, now)
}

#[test]
fn endpoint_tls12_tls13_handshake_ciphertext_is_raw_not_a_carrier_or_stream_frame() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        let original = owner(); let protocol = ArcProtocol::new(original.clone()); let scope = protocol.connection_scope(); let now = Instant::now();
        let mut engine = EndpointTlsEngine::for_original(original.clone(), scope.clone(), parameters("fixture.invalid", &cert.root), now).unwrap();
        let wire = engine.take_wire_output(&original, &scope, now).unwrap(); let raw = endpoint_raw_wire(&wire, &original, &scope);
        assert_eq!(raw[0], 22); assert_eq!(raw[1], 3);
        let mut server = cert.server(version); server_input(&mut server, &raw);
        let cipher = server_output(&mut server); let (_, pending) = endpoint_receive(&mut engine, &original, &scope, &cipher, 17, now).unwrap();
        let pending = endpoint_raw_wire(&pending, &original, &scope); if !pending.is_empty() { server_input(&mut server, &pending); }
        endpoint_handshake(&mut engine, &original, &scope, &mut server, now).unwrap();
        assert_eq!(engine.phase(), TlsPhase::Verified); assert_eq!(protocol.phase(), ArcPhase::Initial);
    }
}

#[test]
fn endpoint_packets_have_encrypted_be32_lengths_and_preserve_opaque_binary_payloads() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (protocol, original, scope, mut engine, mut server, now) = endpoint_setup(&cert, &rustls::version::TLS13);
    let packets = vec![TransportBytes::from_bytes(vec![0xff, 0xff, 6, 0, 2, 0xff]), TransportBytes::from_bytes(vec![0, 7])];
    let expected = packets.iter().flat_map(|p| encode_packet_stream_frame(p.expose_to_transport()).unwrap().expose_to_transport().to_vec()).collect::<Vec<_>>();
    let wire = engine.encrypt_packets(&original, &scope, &packets, now).unwrap().unwrap();
    server_input(&mut server, &endpoint_raw_wire(&wire, &original, &scope));
    assert_eq!(server_plaintext(&mut server), expected);
    assert_eq!(protocol.phase(), ArcPhase::Initial);
    assert_eq!(engine.engine.encrypt_packets(&original, &scope, &packets, None, now).err(), Some(TlsError::InvalidConfiguration));
    assert_eq!(engine.engine.on_wire_bytes(&original, &scope, &[], None, now).err(), Some(TlsError::InvalidConfiguration));
}

#[test]
fn endpoint_packet_prefix_and_body_cross_tls_records_without_common_brand_or_false_ready() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (protocol, original, scope, mut engine, mut server, now) = endpoint_setup(&cert, &rustls::version::TLS13);
    let first = encode_packet_stream_frame(&[0, 0xff, 7, 0]).unwrap(); let second = encode_packet_stream_frame(&[2]).unwrap();
    let raw = first.expose_to_transport(); server.writer().write_all(&raw[..2]).unwrap();
    let cipher = server_output(&mut server); assert!(endpoint_receive(&mut engine, &original, &scope, &cipher, 1, now).unwrap().0.is_empty());
    assert!(engine.next_deadline().is_some());
    server.writer().write_all(&raw[2..]).unwrap(); server.writer().write_all(second.expose_to_transport()).unwrap();
    let cipher = server_output(&mut server); let (packets, _) = endpoint_receive(&mut engine, &original, &scope, &cipher, 11, now).unwrap();
    assert_eq!(packets.iter().map(|p| p.expose_to_transport()).collect::<Vec<_>>(), vec![&[0, 0xff, 7, 0][..], &[2][..]]);
    assert!(engine.next_deadline().is_none()); assert_eq!(protocol.phase(), ArcPhase::Initial);
}

#[test]
fn endpoint_large_packet_spans_tls_records_and_invalid_later_send_emits_nothing() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (protocol, original, scope, mut engine, mut server, now) = endpoint_setup(&cert, &rustls::version::TLS13);
    let large = vec![0xa5; 30_000]; let packet = TransportBytes::from_bytes(large.clone());
    let wire = engine.encrypt_packets(&original, &scope, &[packet], now).unwrap().unwrap(); let cipher = endpoint_raw_wire(&wire, &original, &scope);
    assert!(CipherRecords::default().feed(&cipher).unwrap().len() >= 2); server_input(&mut server, &cipher);
    assert_eq!(server_plaintext(&mut server), encode_packet_stream_frame(&large).unwrap().expose_to_transport());
    let invalid = [TransportBytes::from_bytes(vec![1]), TransportBytes::from_bytes(Vec::new())];
    assert_eq!(engine.encrypt_packets(&original, &scope, &invalid, now).err(), Some(TlsError::Transport(TransportError::InvalidPacket)));
    assert!(engine.take_wire_output(&original, &scope, now).unwrap().is_empty());
    assert_eq!(engine.phase(), TlsPhase::Verified); assert_eq!(protocol.phase(), ArcPhase::Initial);
}

#[test]
fn endpoint_wrong_root_wrong_identity_and_wildcard_fail_before_packet_delivery() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let other = Certificates::generate("DNS:fixture.invalid");
    for (authority, root) in [("other.invalid", &cert.root), ("fixture.invalid", &other.root)] {
        let original = owner(); let scope = RpcConnectionScope::fresh(); let now = Instant::now();
        let mut engine = EndpointTlsEngine::for_original(original.clone(), scope.clone(), parameters(authority, root), now).unwrap();
        assert_eq!(engine.encrypt_packets(&original, &scope, &[TransportBytes::from_bytes(vec![1])], now).err(), Some(TlsError::NotReady));
        assert_eq!(endpoint_handshake(&mut engine, &original, &scope, &mut cert.server(&rustls::version::TLS13), now), Err(TlsError::HandshakeFailed));
        assert!(!scope.is_open());
    }
    let wildcard = Certificates::generate("DNS:*.fixture.invalid"); let original = owner(); let scope = RpcConnectionScope::fresh(); let now = Instant::now();
    let mut engine = EndpointTlsEngine::for_original(original.clone(), scope.clone(), parameters("host.fixture.invalid", &wildcard.root), now).unwrap();
    assert_eq!(endpoint_handshake(&mut engine, &original, &scope, &mut wildcard.server(&rustls::version::TLS13), now), Err(TlsError::HandshakeFailed));
}

#[test]
fn endpoint_stale_owner_scope_and_fixed_partial_deadline_do_not_rebind_delivery() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_protocol, original, scope, mut engine, mut server, now) = endpoint_setup(&cert, &rustls::version::TLS13);
    assert!(engine.on_wire_bytes(&owner(), &scope, &[0xff], now - Duration::from_secs(10)).unwrap().is_none());
    assert!(engine.on_wire_bytes(&original, &RpcConnectionScope::fresh(), &[0xff], now - Duration::from_secs(10)).unwrap().is_none());
    server.writer().write_all(&[0, 0]).unwrap(); let cipher = server_output(&mut server);
    endpoint_receive(&mut engine, &original, &scope, &cipher, 97, now).unwrap();
    server.writer().write_all(&[0]).unwrap(); let cipher = server_output(&mut server);
    endpoint_receive(&mut engine, &original, &scope, &cipher, 97, now + Duration::from_millis(999)).unwrap();
    assert_eq!(engine.tick(&original, &scope, now + Duration::from_secs(1)), Err(TlsError::Timeout)); assert!(!scope.is_open());
}

#[test]
fn endpoint_malformed_or_overfull_authenticated_packets_suppress_whole_step_and_close() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    for raw in [vec![0, 0, 0, 1, 7, 0, 0, 0, 0], vec![0, 0, 0, 1, 7].repeat(super::super::packet_stream::MAX_PACKETS_PER_INPUT + 1)] {
        let (_protocol, original, scope, mut engine, mut server, now) = endpoint_setup(&cert, &rustls::version::TLS13);
        server.writer().write_all(&raw).unwrap(); let cipher = server_output(&mut server);
        assert!(engine.on_wire_bytes(&original, &scope, &cipher, now).is_err()); assert!(!scope.is_open());
    }
}

#[test]
fn endpoint_scoped_packet_brand_eof_and_drop_never_authorize_another_owner_or_arc() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (protocol, original, scope, mut engine, mut server, now) = endpoint_setup(&cert, &rustls::version::TLS13);
    server.writer().write_all(encode_packet_stream_frame(&[2]).unwrap().expose_to_transport()).unwrap(); let cipher = server_output(&mut server);
    let progress = engine.on_wire_bytes(&original, &scope, &cipher, now).unwrap().unwrap();
    assert!(progress.plaintext.packets_for(&owner(), &scope).is_none()); assert!(progress.plaintext.packets_for(&original, &RpcConnectionScope::fresh()).is_none());
    let wire = engine.encrypt_packets(&original, &scope, &[TransportBytes::from_bytes(vec![7])], now).unwrap().unwrap();
    drop(engine); assert!(progress.plaintext.packets_for(&original, &scope).is_none()); assert!(wire[0].frames_for(&original, &scope).is_none());
    assert_eq!(protocol.phase(), ArcPhase::Closed);
    let (_protocol, original, scope, mut engine, mut server, now) = endpoint_setup(&cert, &rustls::version::TLS13);
    server.writer().write_all(&[0, 0]).unwrap(); let cipher = server_output(&mut server); endpoint_receive(&mut engine, &original, &scope, &cipher, 97, now).unwrap();
    assert_eq!(engine.finish_eof(&original, &scope), Err(TlsError::Transport(TransportError::Truncated)));
}

#[test]
fn checked_tls_plaintext_scope_cannot_be_rebound_to_new_arc_or_delivered_after_close() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (mut protocol, original, scope, mut engine, mut server, now) = setup(&cert, &rustls::version::TLS13);
    server.writer().write_all(&[2]).unwrap(); let raw = server_output(&mut server); let wire = frame_cipher(&raw, 512);
    let progress = engine.on_wire_bytes(&original, &scope, &wire, None, now).unwrap().unwrap();
    assert!(progress.plaintext.packets_for(&original, &RpcConnectionScope::fresh()).is_none());
    assert!(progress.plaintext.packets_for(&owner(), &scope).is_none());
    protocol.close(); assert!(progress.plaintext.packets_for(&original, &scope).is_none());
    assert_eq!(engine.phase(), TlsPhase::Closed);
}
