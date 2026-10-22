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
        self.assertIn('backend.logout(owner)', source)
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
        self.assertIn('request.operation != NativeOperation::HostingRegister', pending)
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
        self.assertIn('CallKind::Request => { self.close();', body)
        self.assertNotIn('impl HostingBackend', protocol)
        tracker = (MOD / 'rpc_pending.rs').read_text(encoding='utf-8')
        self.assertIn('Delivery::Batch', tracker)
        self.assertIn('Delivery::Envelope', tracker)
        self.assertIn('request.operation != NativeOperation::HostingRegister', tracker)
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
