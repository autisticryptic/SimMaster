"""Static guards for the recovered TLS/-ri reference DATA subset."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
P = ROOT / 'backend/src/connectivity/app_voice/providers/jegotrip'


class JegotripRouterReferenceContractTests(unittest.TestCase):
    def test_router_option_is_preserved_and_never_enters_the_old_direct_actor(self):
        source = (P / 'object_reference.rs').read_text()
        self.assertIn('router_id: Option<RouterIdentifier>', source)
        self.assertIn('e.scheme == b"tcp" && e.router_id.is_none()', source)
        self.assertIn('bytes.extend_from_slice(&id.literal)', source)
        self.assertIn('explicit_tls_router_groups', source)
        self.assertIn('groups.entry(id).or_default().push(endpoint)', source)
        self.assertIn('id < -999 || id == 0', source)
        # Reference parsing remains data-only: no dial, trust or ready proof.
        for forbidden in ['TcpStream', 'Callback::', 'with_root_certificates', 'unsafe {', 'from_utf8_lossy']:
            self.assertNotIn(forbidden, source)
        connector = (P / 'runtime_connector.rs').read_text()
        self.assertIn('reference.direct_raw_tcp()', connector)
        self.assertNotIn('explicit_tls_router_groups()', connector)

    def test_base_zero_conversion_does_not_confuse_octal_hex_and_decimal(self):
        source = (P / 'object_reference.rs').read_text()
        self.assertIn('strtoll(base=0)', source)
        self.assertIn('(16, &digits[2..])', source)
        self.assertIn('(8, &digits[1..])', source)
        self.assertIn('i64::from_str_radix(text, radix)', source)
        self.assertIn('i32::try_from', source)
        tests = (P / 'object_reference_tests.rs').read_text()
        for name in ['native_router_id_base_zero_conversion_preserves_original_literal',
                     'zero_malformed_or_wrapping_router_ids_fail_closed',
                     'entire_tls_router_reference_rejects_mixed_or_missing_route_associations',
                     'router_grouping_preserves_associations_even_when_host_ports_are_identical']:
            self.assertIn(f'fn {name}', tests)

    def test_both_actions_keep_original_reference_and_connector_tests(self):
        for workflow in ['build-release.yml', 'beta-validation.yml']:
            source = (ROOT / '.github/workflows' / workflow).read_text()
            for module in ['object_reference', 'runtime_connector', 'tls_engine', 'packet_transport']:
                self.assertIn(f'connectivity::app_voice::providers::jegotrip::{module}::tests', source)


if __name__ == '__main__':
    unittest.main()
