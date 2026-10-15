"""Static seams for N8; executed Rust/HTTP/worker regressions remain mandatory in Actions."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
IMS = ROOT / 'backend/src/connectivity/modems/ims'
HW = ROOT / 'backend/src/hardware'


class BearerCapabilitiesBoundary(unittest.TestCase):
    def test_profile_writes_are_after_requirement_admission(self):
        text = (IMS / 'cellular_ims/live.rs').read_text()
        body = text[text.index('async fn connect_inner('):]
        gate = body.index('validate_bearer_profile_requirements(')
        for mutation in ['mm_ims_profile_lease::runtime::prepare(',
                         'prepare_ims_profile_context(', 'set_pcscf_reporting(']:
            self.assertLess(gate, body.index(mutation), mutation)
        self.assertIn('effective.validate_bearer()', text)
        self.assertIn('request.requirements = device_identity.effective_ims.bearer.clone()', body)

    def test_all_native_family_dispatches_keep_requirements(self):
        text = (IMS / 'cellular_ims/native_bearer.rs').read_text()
        body = text[text.index('pub async fn establish_native_ims_bearer_observed('):
                    text.index('fn forced_family_needs_another_attempt(')]
        self.assertEqual(body.count('.establish_ims_bearer_with_requirements('), 2)
        self.assertEqual(body.count('&request.requirements,'), 2)
        self.assertIn('ImsBearerErrorKind::RequirementsUnsupported', body)
        self.assertNotIn('.establish_ims_bearer(', body)

    def test_qmi_query_is_read_only_and_never_invents_an_ipv6_tlv(self):
        text = (HW / 'cellular/backends/wds_pcscf.rs').read_text()
        self.assertIn('GET_CURRENT_SETTINGS: u16 = 0x002d', text)
        self.assertIn('(1 << 11) | (1 << 12)', text)
        self.assertIn('0x23 =>', text)
        self.assertIn('0x24 =>', text)
        self.assertNotIn('0x25 =>', text)
        body = text[text.index('async fn exchange('):]
        for forbidden in ['native_management_exchange(', 'allocate_client(', 'release_client(',
                          'CommandRequest::', 'wds-start-network', 'wds-stop-network']:
            self.assertNotIn(forbidden, body)
        io = (HW / 'cellular/backends/io.rs').read_text()
        verify = io[io.index('async fn verify_retained_wds_endpoint('):io.index('async fn verify(&self,')]
        self.assertIn('self.verify(&endpoint.control_device)', verify)
        self.assertNotIn('/sys/class/net', verify, 'retained netdev has moved to the worker')

    def test_dhcp_is_opt_in_information_only_in_captured_worker(self):
        live = (IMS / 'cellular_ims/live.rs').read_text()
        self.assertIn('DhcpPcscfPolicy::InformationRequest', live)
        self.assertIn('super::dhcp_pcscf::discover(&network_worker_binding,', live)
        text = (IMS / 'cellular_ims/dhcp_pcscf.rs').read_text()
        self.assertIn('binding.create_socket(spec)', text)
        self.assertIn('push_v4(&mut packet, 53, &[8])', text)
        self.assertIn('let mut packet = vec![11]', text)
        for forbidden in ['Command::', 'NetConfigOp::', 'getaddrinfo(', 'lookup_host(', 'UdpSocket::bind(']:
            self.assertNotIn(forbidden, text)

    def test_new_regressions_are_nonzero_workflow_filters(self):
        filters = [
            'hardware::devices::ims_bearer_policy::tests',
            'hardware::devices::transport::capabilities::tests',
            'hardware::cellular::backends::wds_pcscf::tests',
            'hardware::cellular::backends::io::tests::retained_wds_',
            'connectivity::modems::ims::cellular_ims::dhcp_pcscf::tests',
            'connectivity::modems::ims::cellular_ims::native_bearer::policy_tests',
            'connectivity::modems::ims::cellular_ims::live::bearer_policy_tests',
            'connectivity::modems::ims::effective_profile::bearer_tests',
            'connectivity::modems::ims::profile_override::bearer_tests',
            'connectivity::modems::ims::vowifi::profile_record::bearer_tests',
            'connectivity::modems::ims::vowifi::carrier_catalog::v7::bearer_tests',
            'api::handlers::ims_bearer_api_tests',
        ]
        for workflow in ['build-release.yml', 'beta-validation.yml']:
            text = (ROOT / '.github/workflows' / workflow).read_text()
            for filter_ in filters:
                self.assertIn(filter_, text)
            self.assertIn('Empty regression filter', text)


if __name__ == '__main__':
    unittest.main()
