"""Offline credential contract only; no fictitious service transport."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
MOD = ROOT / 'backend/src/connectivity/app_voice/providers/jegotrip'
APP = ROOT / 'backend/src/connectivity/app_voice'


class JegotripContract(unittest.TestCase):
    def test_separates_sdk_token_and_hosting_password(self):
        source = (MOD / 'session.rs').read_text(encoding='utf-8')
        self.assertIn('phone_login_token: sdk.phone_login_token.expose_to_backend()', source)
        self.assertIn('sip_password: hosting.sip_password.expose_to_backend()', source)
        self.assertIn('STATUS_REGISTER: i32 = 4', source)
        self.assertIn('STATUS_UNREGISTER: i32 = 3', source)
        for key in ['AuthnamePrefix', 'ChannelId', 'DeleteIntCode', 'SipNumber', 'SipPwd']:
            self.assertIn(f'serde(rename = "{key}")', source)

    def test_contract_is_not_a_login_or_transport_implementation(self):
        source = (MOD / 'credentials.rs').read_text(encoding='utf-8')
        self.assertIn('account_login_ui_required: false', source)
        self.assertIn('service_credentials_required: true', source)
        self.assertIn('transport_implemented: false', source)
        self.assertIn('direct_sip_interoperability: "unverified"', source)
        self.assertNotIn('derive(Clone, Debug, Serialize)', source)
        self.assertIn('f.write_str("<redacted>")', source)
        whole = '\n'.join(path.read_text(encoding='utf-8') for path in MOD.glob('*.rs'))
        for unsafe_shortcut in ['dlopen(', 'reqwest::', 'https://']:
            self.assertNotIn(unsafe_shortcut, whole)

    def test_callbacks_are_owned_and_import_does_not_grant_ready(self):
        source = (MOD / 'session.rs').read_text(encoding='utf-8')
        self.assertIn('owner.line_id == self.line_id && owner.generation == self.generation', source)
        self.assertIn('self.phase = Phase::CredentialsImported;', source)
        self.assertIn('self.phase == Phase::Hosted && self.incoming_call_channel', source)
        self.assertIn('pub fn outbound_authorized(&self) -> bool { false }', source)

    def test_provider_directories_and_backward_compatible_aliases(self):
        for name in ['jegotrip', 'google_voice', 'telegram', 'imessage']:
            self.assertTrue((APP / 'providers' / name / 'mod.rs').is_file())
        old = (ROOT / 'backend/src/connectivity/modems/ims/mod.rs').read_text(encoding='utf-8')
        self.assertIn('pub use crate::connectivity::app_voice::providers::jegotrip;', old)
        self.assertIn('registry as non_3gpp_voice', old)
        self.assertFalse((ROOT / 'backend/src/connectivity/modems/ims/jegotrip').exists())
        declarations = (APP / 'providers/mod.rs').read_text(encoding='utf-8')
        self.assertIn('imessage::PROVIDER_ID => vec![VoiceCapability::Messaging]', declarations)

    def test_controller_requires_a_backend_and_orders_sdk_stages(self):
        source = (MOD / 'controller.rs').read_text(encoding='utf-8')
        self.assertIn('backend: Option<Arc<dyn HostingBackend>>', source)
        self.assertIn('backend.login(owner.clone(), request)', source)
        self.assertIn('backend.update_status(owner.clone(), STATUS_REGISTER, request)', source)
        self.assertIn('backend.update_status(owner.clone(), STATUS_UNREGISTER, request)', source)
        self.assertIn('backend.logout(owner.clone())', source)
        self.assertIn('backend_owner: Option<AttemptOwner>', source)
        self.assertIn('let original = self.backend_owner.clone()', source)
        self.assertIn('if released { self.backend_owner = None; }', source)
        self.assertIn('backend.retire(&owner)', source)
        session = (MOD / 'session.rs').read_text(encoding='utf-8')
        teardown = session.split('pub fn begin_unhosting', 1)[1].split('pub fn current_owner', 1)[0]
        self.assertLess(teardown.index('self.current_owner()'), teardown.index('self.next_owner()?'))
        self.assertIn('tokio::time::timeout', source)
        for name in ['beta-validation.yml', 'build-release.yml']:
            self.assertIn('connectivity::app_voice::providers::jegotrip::controller::tests',
                          (ROOT / '.github/workflows' / name).read_text(encoding='utf-8'))

    def test_native_gateway_codec_is_bounded_and_is_not_a_network_frame(self):
        source = (MOD / 'rpc_arguments.rs').read_text(encoding='utf-8')
        self.assertIn('updateStatus.SipGatewayPub.SipGateway', source)
        stream = (MOD / 'rpc_stream.rs').read_text(encoding='utf-8')
        self.assertIn('left.len().cmp(&right.len())', stream)
        self.assertIn('left.cmp(right)', stream)
        self.assertIn('to_be_bytes()', stream)
        self.assertIn('MAX_ARGUMENT_BYTES', source)
        self.assertIn('"<redacted>"', stream)
        self.assertNotIn('derive(Serialize)', source + stream)
        for name in ['beta-validation.yml', 'build-release.yml']:
            workflow = (ROOT / '.github/workflows' / name).read_text(encoding='utf-8')
            self.assertIn('connectivity::app_voice::providers::jegotrip::rpc_arguments::tests', workflow)
            self.assertIn('connectivity::app_voice::providers::jegotrip::native_events::tests', workflow)

    def test_rpc_stream_envelopes_and_batches_keep_type_marker_and_bound_parsers(self):
        stream = (MOD / 'rpc_stream.rs').read_text(encoding='utf-8')
        self.assertIn('Self(vec![STREAM_DATA])', stream)
        self.assertIn('MAX_FIELD_BYTES', stream)
        self.assertIn('MAX_MAP_ENTRIES', stream)
        self.assertIn('CodecError::DuplicateKey', stream)
        self.assertIn('reader.bytes()?', stream)
        arguments = (MOD / 'rpc_arguments.rs').read_text(encoding='utf-8')
        self.assertIn('Writer::versioned()', arguments)
        envelope = (MOD / 'rpc_envelope.rs').read_text(encoding='utf-8')
        self.assertIn('Reader::new(bytes)?', envelope)
        self.assertIn('reader.finish()?', envelope)
        batch = (MOD / 'rpc_batch.rs').read_text(encoding='utf-8')
        self.assertIn('swapCalls.AccountEntry.Account', batch)
        self.assertIn('BTreeMap<i32, BatchRequest', batch)
        self.assertIn('if result_code != 0', batch)
        self.assertIn('read_footer(&mut reader)?', batch)
        pending = (MOD / 'rpc_pending.rs').read_text(encoding='utf-8')
        self.assertIn('Arc::ptr_eq(&self.connection.0, &connection.0)', pending)
        self.assertIn('self.pending.remove(&reply.call_id)', pending)
        self.assertIn('request.operation == NativeOperation::HostingRegister', pending)
        self.assertIn('request.operation == NativeOperation::HostingUnregister', pending)
        self.assertIn('self.unhosting.push(OwnedUnhostingReply', pending)
        self.assertIn('id.checked_add(1)', pending)
        self.assertNotIn('session.current_owner()', pending)
        for module in ['rpc_stream', 'rpc_envelope', 'rpc_batch', 'rpc_login', 'rpc_pending']:
            for name in ['beta-validation.yml', 'build-release.yml']:
                self.assertIn(f'connectivity::app_voice::providers::jegotrip::{module}::tests',
                              (ROOT / '.github/workflows' / name).read_text(encoding='utf-8'))

    def test_arc_session_preserves_selected_id_password_and_metadata_token_layers(self):
        source = (MOD / 'arc_session.rs').read_text(encoding='utf-8')
        self.assertIn('encode_swap_calls(&auth.user_id, &auth.selected_session_id, payload)', source)
        self.assertIn('RpcProperty { key: b"pwd", value: inputs.phone_login_token }', source)
        self.assertIn('RpcProperty { key: b"token", value: &auth.token }', source)
        self.assertIn('field.key == b"ForceFlag" && field.value != b"0"', source)
        self.assertIn('self.connection.matches(connection)', source)
        self.assertIn('self.pending.remove(&reply.call_id)', source)
        self.assertIn('PendingKind::Login', source)
        self.assertIn('decode_login_session_result', source)
        self.assertIn('self.authentication = None', source)
        self.assertNotIn('session.current_owner()', source)
        batch = (MOD / 'rpc_batch.rs').read_text(encoding='utf-8')
        self.assertIn('writer.bytes(selected_session_id)?', batch)
        self.assertNotIn('writer.bytes(session_token)?', batch)
        for module in ['rpc_bootstrap', 'arc_session', 'transport_packets', 'arc_protocol']:
            self.assertIn(f'pub mod {module};', (MOD / 'mod.rs').read_text(encoding='utf-8'))
            for name in ['beta-validation.yml', 'build-release.yml']:
                self.assertIn(f'connectivity::app_voice::providers::jegotrip::{module}::tests',
                              (ROOT / '.github/workflows' / name).read_text(encoding='utf-8'))

    def test_transport_packets_do_not_claim_tls_or_raw_tcp_message_boundaries(self):
        source = (MOD / 'transport_packets.rs').read_text(encoding='utf-8')
        self.assertIn('&[0xff, 0xff, DATA_KEY_TLS]', source)
        self.assertIn('(payload.len() as u32).to_be_bytes()', source)
        self.assertIn('remaining.len() / 2', source)
        self.assertIn('TransportError::UnsupportedCompression', source)
        self.assertIn('if result.is_err() { self.close(); }', source)
        self.assertNotIn('from_utf8_lossy', source)
        self.assertNotIn('rustls::', source)
        protocol = (MOD / 'arc_protocol.rs').read_text(encoding='utf-8')
        body = protocol.split('pub fn on_plaintext_packet', 1)[1]
        self.assertLess(body.index('self.rpc.accepts_scope(owner, connection)'), body.index('self.assembler.on_packet(packet)'))
        self.assertIn('CallKind::Request => self.incoming_request(', body)
        self.assertIn('self.rpc.phase() != ArcPhase::Authenticated || self.incoming.is_none()', body)
        self.assertIn('self.close(); return Err(TransportError::UnsupportedKind.into());', body)
        self.assertIn('self.direct_pending.contains_key(&request.call_id)', body)
        self.assertIn('request.call_id <= self.direct_last_id', body)
        self.assertIn('pub fn complete_incoming(&mut self, ticket: &DirectReplyTicket', protocol)
        self.assertNotIn('impl HostingBackend', protocol)
        tracker = (MOD / 'rpc_pending.rs').read_text(encoding='utf-8')
        self.assertIn('Delivery::Batch', tracker)
        self.assertIn('Delivery::Envelope', tracker)
        self.assertIn('request.operation == NativeOperation::HostingRegister', tracker)
        self.assertIn('request.operation == NativeOperation::HostingUnregister', tracker)
        self.assertIn('self.unhosting.push(OwnedUnhostingReply', tracker)
        self.assertIn('batch.matches(owner, &self.connection)', tracker)
        self.assertIn('self.batch_owner.as_ref() != Some(&owner)', tracker)
        self.assertIn('if !self.connection.is_open()', tracker)
        self.assertIn('Arc<AtomicBool>', tracker)
        self.assertNotIn('connection: &RpcConnectionScope, reply: &BatchReply', tracker)
        arc = (MOD / 'arc_session.rs').read_text(encoding='utf-8')
        self.assertIn('Swap(OwnedBatchReply', arc)
        self.assertIn('self.hosting_tracker_taken', arc)
        self.assertIn('inputs.selected_session_id.len() > 127', arc)
        self.assertIn('self.connection.close();', arc)
        self.assertIn('impl Drop for ArcRpcSession', arc)
        self.assertIn('if self.rpc.phase() == ArcPhase::Closed { self.assembler.close(); }', protocol)

    def test_owned_batch_scheduler_does_not_rebind_results_or_ignore_retention(self):
        source = (MOD / 'batch_scheduler.rs').read_text(encoding='utf-8')
        self.assertIn('MAX_BATCH_EPOCHS: usize = 32', source)
        self.assertIn('REQUEST_DEDUP_BITS: usize = 2048', source)
        self.assertIn('BATCH_RETENTION_MS: u64 = 45_000', source)
        self.assertIn('if sequence < self.base { return false; }', source)
        self.assertIn('words.ack_incarnation != self.local_incarnation', source)
        self.assertIn('batch.matches_scheduler(&self.identity)', source)
        self.assertIn('words.ack_next_batch - 1 - bit', source)
        self.assertIn('self.fail(BatchError::Retention)', source)
        self.assertIn('ticket: &ServerRequestTicket', source)
        self.assertIn('self.pending_client.remove(sequence)', source)
        self.assertIn('self.submitted_client.remove(sequence)', source)
        tracker = (MOD / 'rpc_pending.rs').read_text(encoding='utf-8')
        self.assertIn('if batch.is_scheduled() { return Err(CodecError::InvalidField); }', tracker)
        self.assertIn('delivery.matches(owner, &self.connection)', tracker)
        arc = (MOD / 'arc_session.rs').read_text(encoding='utf-8')
        self.assertIn('if self.batch_scheduler_taken { return Err(CodecError::InvalidField); }', arc)
        self.assertIn('self.prepare_swap(&batch.payload(), routing, Some(batch.identity()))', arc)
        self.assertIn('batch.mark_encoded()', arc)
        for module in ['batch_scheduler', 'rpc_incoming', 'offline_backend']:
            for name in ['beta-validation.yml', 'build-release.yml']:
                self.assertIn(f'connectivity::app_voice::providers::jegotrip::{module}::tests',
                              (ROOT / '.github/workflows' / name).read_text(encoding='utf-8'))

    def test_incoming_dispatch_is_exact_configured_and_never_a_public_readiness_endpoint(self):
        source = (MOD / 'rpc_incoming.rs').read_text(encoding='utf-8')
        self.assertIn('recvOnlineMessage.MessageReceiver.Client', source)
        self.assertIn('Reader::versioned(arguments)', source)
        self.assertIn('request.ticket().matches(&self.owner, &self.connection)', source)
        self.assertIn('property(b"id") != Some(self.object_id.as_slice())', source)
        self.assertIn('property(b"domain") != Some(self.domain.as_slice())', source)
        self.assertIn('message.receiver_kind != self.receiver_kind', source)
        self.assertIn('binding.receiver_kind.contains(&b\'*\')', source)
        self.assertNotIn('from_utf8_lossy', source)
        engine = (MOD / 'offline_backend.rs').read_text(encoding='utf-8')
        self.assertIn('scheduler.on_reply(&reply, now_ms)', engine)
        self.assertIn('tracker.on_scheduled_delivery(&delivery)', engine)
        self.assertIn('dispatcher.as_ref()', engine)
        self.assertIn('pub fn complete_message(&mut self, ticket: &ServerRequestTicket', engine)
        self.assertNotIn('impl HostingBackend', engine)
        self.assertNotIn('Callback::Channels', engine)
        self.assertNotIn('Callback::HostingAck { success: true }', engine)

    def test_packet_stream_has_native_length_prefix_and_bounded_split_coalesced_eof(self):
        source = (MOD / 'packet_stream.rs').read_text(encoding='utf-8')
        self.assertIn('(packet.len() as u32).to_be_bytes()', source)
        self.assertIn('u32::from_be_bytes(self.prefix)', source)
        self.assertIn('MAX_NETWORK_PACKET_BYTES', source)
        self.assertIn('MAX_PACKETS_PER_INPUT', source)
        self.assertIn('if expected == 0', source)
        self.assertIn('if self.partial.len() != expected { break; }', source)
        self.assertIn('std::mem::take(&mut self.partial)', source)
        self.assertIn('if result.is_err() { self.close(); }', source)
        self.assertIn('pub fn finish_eof', source)
        self.assertNotIn('from_utf8_lossy', source)
        self.assertNotIn('std::net', source)
        for module in ['packet_stream', 'router_packets', 'packet_transport']:
            self.assertIn(f'pub mod {module};', (MOD / 'mod.rs').read_text(encoding='utf-8'))
            for name in ['beta-validation.yml', 'build-release.yml']:
                self.assertIn(f'connectivity::app_voice::providers::jegotrip::{module}::tests',
                              (ROOT / '.github/workflows' / name).read_text(encoding='utf-8'))

    def test_router_header_routing_and_two_half_subset_never_guess_endpoints(self):
        source = (MOD / 'router_packets.rs').read_text(encoding='utf-8')
        self.assertIn('[2, 6, 8, 12, 8, 12, 14, 18]', source)
        self.assertIn('bytes[7] & 0x88 != 0', source)
        self.assertIn('header.packet_kind | (header.fragment_part << 4)', source)
        self.assertIn('flags & !3 != 0 || address1_mask & 2 != 0', source)
        self.assertIn('header.sequence.checked_add(1)', source)
        self.assertIn('body.len() / 2', source)
        self.assertIn('original.header.packet_kind != header.packet_kind', source)
        self.assertIn('TransportError::UnsupportedRoute', source)
        self.assertIn('if result.is_err() { self.close(); }', source)
        self.assertNotIn('from_utf8_lossy', source)
        for forbidden in ['SocketAddr', 'ToSocketAddrs', 'TcpStream', 'rustls::', 'impl HostingBackend']:
            self.assertNotIn(forbidden, source)

    def test_packet_transport_captures_original_arc_and_routes_before_tls_not_rpc(self):
        source = (MOD / 'packet_transport.rs').read_text(encoding='utf-8')
        receive = source.split('pub fn on_bytes', 1)[1].split('fn consume', 1)[0]
        self.assertLess(receive.index('self.accepts(owner, scope)'), receive.index('self.consume(bytes)'))
        self.assertIn('self.connection.matches(scope) && self.connection.is_open()', source)
        self.assertIn('OwnedWireFrames', source)
        self.assertIn('OwnedTlsInput', source)
        self.assertIn('decode_tls_carrier(body.payload)', source)
        self.assertIn('if !receive.matches(&body)', source)
        self.assertIn('last_received.is_some_and(|last| header.sequence <= last)', source)
        self.assertIn('impl Drop for OwnedPacketTransport', source)
        self.assertIn('self.connection.close(); self.decoder.close(); self.router.close();', source)
        for forbidden in ['Callback::', 'on_plaintext_packet(', 'decode_reply(', 'decode_request(',
                          'rustls::', 'TcpStream', 'impl HostingBackend', 'from_utf8_lossy']:
            self.assertNotIn(forbidden, source)
        arc = (MOD / 'arc_session.rs').read_text(encoding='utf-8')
        self.assertIn('if self.packet_transport_taken', arc)
        self.assertIn('self.owner.clone(), self.connection.clone(), route', arc)
        for filename in ['arc_protocol.rs', 'offline_backend.rs']:
            self.assertIn('pub fn take_packet_transport', (MOD / filename).read_text(encoding='utf-8'))

    def test_tls_plaintext_uses_native_self_delimiting_envelopes_not_record_read_boundaries(self):
        source = (MOD / 'tls_plaintext.rs').read_text(encoding='utf-8')
        self.assertIn('NO added inner length prefix', source)
        self.assertIn('u32::from_be_bytes', source)
        self.assertIn('MAX_MAP_ENTRIES', source)
        self.assertIn('MAX_FIELD_BYTES', source)
        self.assertIn('TLS_FRAGMENT_THRESHOLD', source)
        self.assertIn('remaining / 2', source)
        self.assertIn('decode_request(message.payload', source)
        self.assertIn('decode_reply(message.payload', source)
        self.assertIn('if result.is_err() { self.close(); }', source)
        self.assertNotIn('from_utf8_lossy', source)
        self.assertNotIn('rustls::', source)
        for module in ['tls_plaintext', 'tls_engine']:
            for name in ['beta-validation.yml', 'build-release.yml']:
                self.assertIn(f'connectivity::app_voice::providers::jegotrip::{module}::tests',
                              (ROOT / '.github/workflows' / name).read_text(encoding='utf-8'))

    def test_tls_engine_requires_imported_root_original_authority_and_real_verification(self):
        source = (MOD / 'tls_engine.rs').read_text(encoding='utf-8')
        self.assertIn('roots.add(CertificateDer::from(der))', source)
        self.assertIn('with_root_certificates(roots).with_no_client_auth()', source)
        self.assertIn('config.enable_early_data = false', source)
        self.assertIn('Resumption::disabled()', source)
        self.assertIn('config.enable_sni = parameters.enable_sni', source)
        self.assertIn('has_exact_identity(peer.as_ref(), &self.authority)', source)
        self.assertIn('connection.process_new_packets()', source)
        self.assertIn('self.phase != TlsPhase::Verified', source)
        self.assertIn('self.plaintext.on_bytes(&plaintext[..count])', source)
        self.assertIn('self.transport.has_partial()', source)
        self.assertIn('self.partial_deadline.is_none()', source)
        self.assertIn('now >= self.handshake_deadline', source)
        self.assertIn('pub struct VerifiedPlaintextPackets { owner:', source)
        body = source.split('pub fn on_wire_bytes', 1)[1].split('fn receive', 1)[0]
        self.assertLess(body.index('self.accepts(owner, scope)'), body.index('self.receive(bytes, timing, now)'))
        for forbidden in ['dangerous()', 'with_custom_certificate_verifier', 'WebPkiServerVerifier::builder',
                          'webpki_roots::', 'TcpStream', 'reqwest::', 'connect(', 'key_log =', 'from_utf8_lossy']:
            self.assertNotIn(forbidden, source)
        backend = (MOD / 'offline_backend.rs').read_text(encoding='utf-8')
        self.assertIn('pub fn on_verified_plaintext', backend)
        self.assertIn('plaintext.packets_for(owner, scope)', backend)
        self.assertIn('self.protocol.take_tls_engine', backend)
        self.assertNotIn('Callback::Channels', source)

    def test_tls_shared_exact_identity_retains_existing_sip_path_without_new_downgrade(self):
        identity = ROOT / 'backend/src/connectivity/core/tls_identity.rs'
        self.assertIn('value.eq_ignore_ascii_case(dns.as_bytes())', identity.read_text(encoding='utf-8'))
        self.assertIn('certificate.len() > 262_144', identity.read_text(encoding='utf-8'))
        wrapper = (ROOT / 'backend/src/connectivity/modems/ims/cellular_ims/sip_tls_identity.rs').read_text(encoding='utf-8')
        self.assertIn('crate::connectivity::core::tls_identity::has_exact_identity', wrapper)
        for test in ['sip_identity_rejects_wildcard_cn_guesses_and_wrong_san_types',
                     'sip_identity_der_is_bounded_and_never_accepts_truncated_certificates']:
            self.assertIn(test, wrapper)
        for name in ['beta-validation.yml', 'build-release.yml']:
            self.assertIn('connectivity::core::tls_identity::tests', (ROOT / '.github/workflows' / name).read_text(encoding='utf-8'))

    def test_actor_has_original_scope_poll_gate_and_no_network_dial_or_provider(self):
        source = (MOD / 'transport_actor.rs').read_text(encoding='utf-8')
        self.assertIn('AsyncRead + AsyncWrite + Unpin + Send', source)
        self.assertIn('self.backend.on_verified_plaintext', source)
        self.assertIn('self.pending.retain(|id, _| self.backend.is_pending(*id))', source)
        self.assertIn('scope.with_io_gate(|| Pin::new(&mut *io).poll_write', source)
        self.assertIn('scope.with_io_gate(|| Pin::new(&mut *io).poll_flush', source)
        self.assertIn('_ = scope.closed()', source)
        self.assertIn('_ = completion.closed()', source)
        self.assertIn('self.commands.try_send', source)
        self.assertIn('self.events.try_send', source)
        self.assertIn('self.io = None; self.commands.close()', source)
        self.assertIn('pub fn io_released(&self)', source)
        self.assertIn('self.task.as_mut()', source)
        self.assertNotIn('self.task.take()', source)
        self.assertIn('ticket: &ServerRequestTicket', source)
        self.assertIn('ticket: &DirectReplyTicket', source)
        self.assertIn('pub mod transport_actor;', (MOD / 'mod.rs').read_text(encoding='utf-8'))
        for forbidden in ['TcpStream', 'connect(', 'reqwest::', 'impl HostingBackend', 'Callback::Channels', 'from_utf8_lossy']:
            self.assertNotIn(forbidden, source)
        for name in ['beta-validation.yml', 'build-release.yml']:
            self.assertIn('connectivity::app_voice::providers::jegotrip::transport_actor::tests',
                          (ROOT / '.github/workflows' / name).read_text(encoding='utf-8'))

    def test_close_gate_is_shared_terminal_and_poll_leases_are_not_async_locks(self):
        source = (MOD / 'rpc_pending.rs').read_text(encoding='utf-8')
        self.assertIn('Arc<Mutex<()>>', source)
        self.assertIn('Arc<tokio::sync::Notify>', source)
        gate = source.split('pub(super) fn with_io_gate', 1)[1].split('pub(super) async fn closed', 1)[0]
        self.assertIn('if !self.is_open() { return None; }', gate)
        self.assertIn('self.0.store(false, Ordering::Release)', gate)
        self.assertNotIn('.await', gate)
        self.assertIn('notified.as_mut().enable()', source)
        self.assertIn('if !self.is_open() { return; }', source)
        actor = (MOD / 'transport_actor.rs').read_text(encoding='utf-8')
        self.assertIn('tokio::time::MissedTickBehavior::Skip', actor)
        self.assertIn('self.tls.next_deadline()', actor)
        self.assertIn('count > frame.len() - offset', actor)
        self.assertIn('RuntimeError::Cancelled', actor)

    def test_legacy_config_unwrap_requires_explicit_context_and_does_not_weaken_tls(self):
        source = (MOD / 'legacy_config.rs').read_text(encoding='utf-8')
        self.assertIn('pub fn supplied(bytes: &[u8])', source)
        self.assertIn('for _ in 0..999', source)
        self.assertIn('md5::compute(&material[..])', source)
        self.assertIn('raw.len() < 16 || raw.len() % 8 != 0', source)
        self.assertIn('decrypt_padded_mut::<Pkcs7>', source)
        self.assertIn('Zeroizing', source)
        self.assertIn('key.ok_or(ConfigError::KeyRequired)', source)
        self.assertIn('ConfigError::MultipleEnvelopes', source)
        self.assertIn('WEAK_KEYS.iter().any', source)
        self.assertNotIn('derive(Serialize)', source)
        self.assertNotIn('pub const PASSWORD', source)
        self.assertNotIn('root_der', source)
        for source_file in ['tls_engine.rs', 'session.rs', 'transport_packets.rs']:
            self.assertNotIn('des::', (MOD / source_file).read_text(encoding='utf-8'))
        cargo = (ROOT / 'backend/Cargo.toml').read_text(encoding='utf-8')
        self.assertIn('des = { version = "0.8.1", features = ["zeroize"] }', cargo)

    def test_reference_and_deployment_are_bounded_data_not_guessed_transport_authorities(self):
        source = (MOD / 'object_reference.rs').read_text(encoding='utf-8')
        self.assertIn('ReferenceTarget::Adapter', source)
        self.assertIn('ReferenceTarget::Direct', source)
        self.assertIn('bytes.split(|&b| b == b\';\').peekable()', source)
        self.assertIn('e.scheme == b"tcp"', source)
        self.assertIn('NoDirectEndpoints', source)
        self.assertIn('fn adapter_target', source)
        self.assertIn("b'$' if groups.is_empty()", source)
        self.assertIn('impl Drop for ObjectReference', source)
        self.assertIn('impl Drop for ReferenceTarget', source)
        self.assertIn('impl Drop for ReferenceEndpoint', source)
        self.assertIn('self.identity.zeroize()', source)
        self.assertIn('MAX_ENDPOINTS', source)
        self.assertIn('MAX_COMPONENTS', source)
        self.assertIn('MAX_GROUP_DEPTH', source)
        plan = (MOD / 'deployment.rs').read_text(encoding='utf-8')
        body = plan.split('pub fn from_reply', 1)[1]
        self.assertLess(body.index('reply.material_for(owner, scope)'), body.index('decode_configuration(material.account_entry'))
        for forbidden in ['TcpStream', 'connect(', 'with_root_certificates', 'Callback::', 'lookup_ip', 'derive(Serialize)']:
            self.assertNotIn(forbidden, source + plan)
        for module in ['legacy_config', 'object_reference', 'deployment']:
            self.assertIn(f'pub mod {module};', (MOD / 'mod.rs').read_text(encoding='utf-8'))
            for name in ['beta-validation.yml', 'build-release.yml']:
                self.assertIn(f'connectivity::app_voice::providers::jegotrip::{module}::tests',
                              (ROOT / '.github/workflows' / name).read_text(encoding='utf-8'))

    def test_bootstrap_delivery_has_original_provenance_and_actor_does_not_grant_sdk_success(self):
        arc = (MOD / 'arc_session.rs').read_text(encoding='utf-8')
        self.assertIn('pub struct OwnedBootstrapReply', arc)
        self.assertIn('impl Drop for OwnedBootstrapReply', arc)
        self.assertIn('self.account_entry.zeroize()', arc)
        self.assertIn('Bootstrap(OwnedBootstrapReply)', arc)
        self.assertIn('!self.connection.matches(scope) || !self.connection.is_open()', arc)
        self.assertIn('call_id: reply.call_id', arc)
        self.assertNotIn('Bootstrap(AppConfigsResult', arc)
        engine = (MOD / 'offline_backend.rs').read_text(encoding='utf-8')
        self.assertIn('events.push(OfflineEvent::Bootstrap(configuration))', engine)
        actor = (MOD / 'transport_actor.rs').read_text(encoding='utf-8')
        self.assertIn('pub async fn bootstrap(', actor)
        self.assertIn('Operation::Bootstrap(key, routing) => self.backend.prepare_bootstrap', actor)
        self.assertNotIn('Callback::SdkLogin { success: true }', actor)
        credentials = (MOD / 'credentials.rs').read_text(encoding='utf-8')
        self.assertIn('pub client_config_decode_key: Option<Secret>', credentials)
        self.assertIn('pub reference_decode_key: Option<Secret>', credentials)
        self.assertIn('jegotrip_config_decode_key_invalid', credentials)
        self.assertIn('impl Drop for Secret', credentials)
        plan = (MOD / 'deployment.rs').read_text(encoding='utf-8')
        self.assertIn('impl Drop for BootstrapDeployment', plan)
        self.assertIn('value.zeroize()', plan)

    def test_native_router_controls_are_data_only_and_duplicate_bookkeeping_is_not_delivery(self):
        source = (MOD / 'native_router.rs').read_text(encoding='utf-8')
        self.assertIn('packet_kind > 3', source)
        self.assertIn('header.packet_kind != 2', source)
        self.assertIn('header.packet_kind != 3', source)
        self.assertIn('sequence.wrapping_sub(512)', source)
        self.assertIn('distance > 1023', source)
        self.assertIn('ReceiveObservation::Duplicate', source)
        self.assertIn('header.sequence.wrapping_add(1)', source)
        self.assertIn('header.sequence.wrapping_sub(1)', source)
        self.assertIn('HALF_AGE', source)
        self.assertIn('self.halves.clear()', source)
        self.assertIn('impl Drop for Half', source)
        for forbidden in ['Callback::', 'TcpStream', 'login(', 'success: true', 'unsafe {']:
            self.assertNotIn(forbidden, source)

    def test_native_channel_framing_and_record_markers_do_not_sniff_tls_or_grant_auth(self):
        source = (MOD / 'native_channel.rs').read_text(encoding='utf-8')
        self.assertIn('const BASE_HEADER: usize = 8', source)
        self.assertIn('HeaderMode::Timestamped { 2 }', source)
        self.assertIn('PacketKind::from_bits(header[2] >> 5)', source)
        self.assertIn('header[3] & 0x7f', source)
        self.assertIn('packet.inclusive_ack', source)
        self.assertIn('1 << (bit & 7)', source)
        self.assertIn('0xfe | 0xff', source)
        self.assertIn('u16::from_be_bytes([prefix[0],prefix[1]])', source)
        self.assertIn('self.partial.zeroize()', source)
        self.assertIn('extend_wiping(&mut self.partial', source)
        buffers = (MOD / 'transport_packets.rs').read_text(encoding='utf-8')
        self.assertIn('impl Drop for TransportBytes', buffers)
        self.assertIn('buffer.zeroize(); *buffer = replacement', buffers)
        self.assertIn('Vec::with_capacity(expected)', buffers)
        self.assertIn('finish_eof', source)
        for forbidden in ['Callback::', 'TcpStream', 'TlsEngine', 'success: true', 'DEAA7C', 'unsafe {']:
            self.assertNotIn(forbidden, source)
        for module in ['native_router', 'native_channel']:
            self.assertIn(f'pub mod {module};', (MOD / 'mod.rs').read_text(encoding='utf-8'))
            for name in ['beta-validation.yml', 'build-release.yml']:
                self.assertIn(f'connectivity::app_voice::providers::jegotrip::{module}::tests',
                              (ROOT / '.github/workflows' / name).read_text(encoding='utf-8'))

    def test_original_logout_is_native_semantic_completion_not_submission_or_new_owner(self):
        source = (MOD / 'rpc_logout.rs').read_text(encoding='utf-8')
        self.assertIn('logoutSession.AccountEntry.Account', source)
        self.assertIn('writer.bytes(user_id)', source)
        self.assertIn('writer.bytes(selected_session_id)', source)
        self.assertIn('read_footer(&mut reader)', source)
        arc = (MOD / 'arc_session.rs').read_text(encoding='utf-8')
        self.assertIn('LogoutPending', arc)
        self.assertIn('OwnedLogoutReply', arc)
        self.assertIn('self.authentication = None; self.phase = ArcPhase::LoggedOut', arc)
        actor = (MOD / 'transport_actor.rs').read_text(encoding='utf-8')
        self.assertIn('RuntimeCommands', actor)
        self.assertIn('LogoutRetirement', actor)
        self.assertIn('reply.call_id() == pending.id && reply.accepted()', actor)
        self.assertIn('RuntimeError::LoggedOut', actor)
        self.assertIn('let receipt = self.close().await', actor)
        for secret in ['Property', 'Login', 'Binding', 'Gateway']:
            self.assertIn(f'impl Drop for {secret}', actor)
        for name in ['beta-validation.yml', 'build-release.yml']:
            self.assertIn('connectivity::app_voice::providers::jegotrip::rpc_logout::tests',
                          (ROOT / '.github/workflows' / name).read_text(encoding='utf-8'))

    def test_gateway_unregistration_is_a_distinct_original_nested_result_not_outer_ack(self):
        pending = (MOD / 'rpc_pending.rs').read_text(encoding='utf-8')
        self.assertIn('pub struct OwnedUnhostingReply', pending)
        self.assertIn('request.operation == NativeOperation::HostingUnregister', pending)
        self.assertIn('take_unhosting_replies', pending)
        offline = (MOD / 'offline_backend.rs').read_text(encoding='utf-8')
        self.assertIn('events.push(OfflineEvent::Unhosting(reply))', offline)
        actor = (MOD / 'transport_actor.rs').read_text(encoding='utf-8')
        self.assertIn('reply.sequence() == pending.sequence && reply.accepted()', actor)
        self.assertIn('UnhostingRetirement', actor)
        self.assertIn('if now >= pending.deadline', actor)
        self.assertIn('unhosting_sequence = Some(sequence)', actor)

    def test_unregister_admission_precedes_retirement_and_preserves_invalid_input_io(self):
        actor = (MOD / 'transport_actor.rs').read_text(encoding='utf-8')
        enqueue = actor.split('fn enqueue(', 1)[1].split('async fn submit(', 1)[0]
        self.assertIn('operation.byte_length() > MAX_PARAMETER_BYTES', enqueue)
        self.assertIn('self.commands.try_send', enqueue)
        hosting = actor.split('pub async fn hosting(', 1)[1].split('pub async fn complete_batch(', 1)[0]
        self.assertLess(hosting.index('self.enqueue('), hosting.index('Some(UnhostingRetirement'))
        pending = (MOD / 'rpc_pending.rs').read_text(encoding='utf-8')
        self.assertIn('self.unhosting.retain(|reply| reply.sequence != id)', pending)
        self.assertIn('if !self.connection.is_open() { self.retire_all(); return Vec::new(); }', pending)

    def test_unregister_deadline_and_same_delivery_failures_precede_semantic_success(self):
        actor = (MOD / 'transport_actor.rs').read_text(encoding='utf-8')
        self.assertIn('.chain(self.unhosting_pending.as_ref().map(|pending| pending.deadline))', actor)
        self.assertIn('if now >= deadline { self.scope.close(); return Err(RuntimeError::Timeout); }', actor)
        delivery = actor.split('async fn deliver(', 1)[1].split('async fn deliver_events(', 1)[0]
        self.assertLess(delivery.index('self.deliver_events('), delivery.index('pending.completion.send(result)'))
        self.assertIn('if Instant::now() >= pending.deadline', delivery)
        self.assertIn('if !self.scope.is_open()', delivery)
        self.assertIn('if unhosting_accepted || outcome.is_err()', delivery)

    def test_native_notifications_require_local_operation_and_stale_scope_checks(self):
        events = (MOD / 'native_events.rs').read_text(encoding='utf-8')
        controller = (MOD / 'controller.rs').read_text(encoding='utf-8')
        self.assertIn('notification.operation != Some(NativeOperation::HostingRegister)', events)
        self.assertIn('notification.operation != Some(NativeOperation::SdkLogin)', events)
        self.assertNotIn('pub rpc_result', events)
        self.assertNotIn('from_utf8_lossy', events)
        self.assertIn('Arc::ptr_eq(&owner.incarnation, &self.incarnation)', (MOD / 'session.rs').read_text(encoding='utf-8'))
        body = controller.split('pub async fn on_native_notification', 1)[1].split('pub fn tick', 1)[0]
        self.assertLess(body.index('self.session.owns(owner)'), body.index('match decode(notification)'))
        self.assertLess(body.index('self.session.accepts_incoming_message(owner)'), body.index('match decode(notification)'))
        self.assertIn('self.tick(now, Instant::now());', body)

    def test_api_only_returns_metadata_and_tests_are_selected(self):
        main = (ROOT / 'backend/src/main.rs').read_text(encoding='utf-8')
        self.assertIn('/api/non-3gpp/providers/app_voice/credential-contract', main)
        handlers = (ROOT / 'backend/src/api/handlers.rs').read_text(encoding='utf-8')
        body = handlers.split('pub async fn get_jegotrip_credential_contract_handler()', 1)[1]
        body = body.split('pub async fn get_non_3gpp_providers_handler(', 1)[0]
        self.assertIn('jegotrip::credentials::contract()', body)
        for forbidden in ['State(', 'with_conn(', '.login(', 'install(', 'reqwest']:
            self.assertNotIn(forbidden, body)
        for name in ['beta-validation.yml', 'build-release.yml']:
            self.assertIn('connectivity::app_voice::providers::jegotrip::tests',
                          (ROOT / '.github/workflows' / name).read_text(encoding='utf-8'))


if __name__ == '__main__':
    unittest.main()
