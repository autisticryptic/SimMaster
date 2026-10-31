"""Static guards for the proven native tls->tlsp boundary; compiled tests run only on Actions."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
NATIVE = ROOT / 'backend/src/connectivity/app_voice/providers/jegotrip'


class EndpointTlsContractTests(unittest.TestCase):
    def test_mode_selection_binds_both_wire_and_authenticated_plaintext_boundaries(self):
        source = (NATIVE / 'tls_engine.rs').read_text()
        self.assertIn('Some(route) => (WireTransport::Carrier', source)
        self.assertIn('None => (WireTransport::Endpoint, PlaintextDecoder::Endpoint(PacketStreamDecoder::default()))', source)
        self.assertIn('TlsEngine::create(owner, scope, None, parameters, now)', source)
        self.assertIn('OwnedWireFrames::endpoint_tls_output(owner, scope, bytes)', source)
        for forbidden in ['TcpStream', 'reqwest::', 'dangerous()', 'with_custom_certificate_verifier', 'webpki_roots::', 'Callback::Channels']:
            self.assertNotIn(forbidden, source)

    def test_packet_sender_prefix_is_inside_endpoint_tls_not_around_ciphertext(self):
        source = (NATIVE / 'tls_engine.rs').read_text()
        endpoint = source.split('impl EndpointTlsEngine {', 1)[1]
        self.assertIn('encode_packet_stream_frame(p.expose_to_transport())', endpoint)
        self.assertIn('self.engine.encrypt(&framed, None)', endpoint)
        self.assertLess(endpoint.index('if total > MAX_TLS_IO_BYTES'), endpoint.index('let framed ='))
        self.assertNotIn('encode_tls_carrier', endpoint)
        self.assertNotIn('PacketRoute::DirectTlsCarrier', endpoint)
        maker = (NATIVE / 'packet_transport.rs').read_text().split('fn endpoint_tls_output(', 1)[1].split('/// Checked snapshot', 1)[0]
        self.assertIn('TransportBytes::from_bytes(bytes.to_vec())', maker)
        self.assertNotIn('encode_packet_stream_frame', maker)
        self.assertNotIn('encode_tls_carrier', maker)

    def test_endpoint_packet_brand_cannot_enter_common_or_restore_sdk_readiness(self):
        source = (NATIVE / 'tls_engine.rs').read_text()
        endpoint = source.split('impl EndpointTlsEngine {', 1)[1]
        self.assertIn('plaintext: VerifiedEndpointPackets', source)
        self.assertNotIn('VerifiedPlaintextPackets', endpoint)
        self.assertIn('if self.plaintext.is_endpoint() { return Err(TlsError::InvalidConfiguration); }', source)
        self.assertIn('if !self.engine.accepts(owner, scope) { return Ok(None); }', endpoint)
        self.assertNotIn('RpcConnectionScope::fresh', endpoint)
        profile = (NATIVE / 'runtime_backend.rs').read_text()
        self.assertIn('if !matches!(input.route, PacketRoute::DirectTlsCarrier)', profile)

    def test_packet_sender_plaintext_does_not_reallocate_or_remain_in_retired_decoder(self):
        source = (NATIVE / 'packet_stream.rs').read_text()
        self.assertIn('self.partial.reserve_exact(expected)', source)
        self.assertLess(source.index('if expected > MAX_NETWORK_PACKET_BYTES'), source.index('self.partial.reserve_exact(expected)'))
        self.assertIn('self.partial.zeroize()', source)
        self.assertIn('impl Drop for PacketStreamDecoder', source)

    def test_router_connect_control_keeps_original_agent_identity_and_credential_namespaces(self):
        source = (NATIVE / 'native_router.rs').read_text()
        self.assertIn('clientConnect1.RouterClient.Common', source)
        self.assertIn('clientConnect3.RouterClient.Common', source)
        self.assertIn('key: b"id", value: b"Router"', source)
        self.assertIn('key: b"arcVer", value: b"16777216"', source)
        self.assertIn('args.u32(0)', source)
        self.assertIn('args.u32(self.nonce)', source)
        self.assertIn('accessKey', source)
        self.assertIn('accessAuth', source)
        for forbidden in ['phone_login_token', 'sip_password', 'Callback::', 'TcpStream', 'from_utf8_lossy']:
            self.assertNotIn(forbidden, source)

    def test_router_assignment_requires_current_endpoint_tls_brand_and_complete_group(self):
        source = (NATIVE / 'native_router.rs').read_text()
        self.assertIn('input: super::tls_engine::VerifiedEndpointPackets', source)
        body = source.split('pub fn on_packets(', 1)[1].split('fn consume(', 1)[0]
        self.assertLess(body.index('self.accepts(owner, scope)'), body.index('input.packets_for(owner, scope)'))
        self.assertLess(body.index('input.packets_for(owner, scope)'), body.index('self.check_time(now)'))
        self.assertIn('reply.call_id != 1', source)
        self.assertIn('reply.result_code != 0', source)
        self.assertIn('router == 0 || node == 0', source)
        self.assertIn('reader.finish()', source)
        self.assertIn('if assignment.is_some() { self.phase = RouterConnectPhase::Connected', source)
        self.assertIn('self.scope.close()', source)

    def test_router_control_tests_use_synthetic_ids_and_original_layered_tls(self):
        source = (NATIVE / 'native_router_tests.rs').read_text()
        self.assertIn('const ROUTER_FIXTURE_ID: i32 = 12001', source)
        self.assertNotIn('mem::forget', source)
        for name in [
            'actual_endpoint_tls_router_connect_assigns_only_logical_ids_after_matched_reply',
            'router_connect_rejected_zero_or_nonzero_outer_results_never_assign_or_grant_sdk',
            'router_connect_foreign_tls_brand_is_ignored_before_clock_or_parsing_and_budget_is_fixed',
            'router_connect_valid_reply_followed_by_bad_router_packet_suppresses_assignment',
        ]:
            self.assertIn('fn ' + name, source)
        for workflow in ['build-release.yml', 'beta-validation.yml']:
            self.assertIn('connectivity::app_voice::providers::jegotrip::native_router::tests',
                (ROOT / '.github/workflows' / workflow).read_text())

    def test_original_and_new_compiled_regressions_remain_selected_in_both_workflows(self):
        tests = (NATIVE / 'tls_engine_tests.rs').read_text()
        for name in [
            'endpoint_tls12_tls13_handshake_ciphertext_is_raw_not_a_carrier_or_stream_frame',
            'endpoint_packets_have_encrypted_be32_lengths_and_preserve_opaque_binary_payloads',
            'endpoint_packet_prefix_and_body_cross_tls_records_without_common_brand_or_false_ready',
            'endpoint_wrong_root_wrong_identity_and_wildcard_fail_before_packet_delivery',
            'endpoint_stale_owner_scope_and_fixed_partial_deadline_do_not_rebind_delivery',
            'endpoint_malformed_or_overfull_authenticated_packets_suppress_whole_step_and_close',
            'endpoint_scoped_packet_brand_eof_and_drop_never_authorize_another_owner_or_arc',
            'tls12_and_tls13_validate_imported_root_authority_and_keep_sdk_login_separate',
        ]:
            self.assertIn('fn ' + name, tests)
        for workflow in ['build-release.yml', 'beta-validation.yml']:
            source = (ROOT / '.github/workflows' / workflow).read_text()
            for module in ['tls_engine', 'packet_stream', 'packet_transport', 'runtime_connector']:
                self.assertIn(f'connectivity::app_voice::providers::jegotrip::{module}::tests', source)


if __name__ == '__main__':
    unittest.main()
