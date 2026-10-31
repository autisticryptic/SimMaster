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
