"""Owned IPv6 multi-context proof must not become APN/CID-only P-CSCF reuse."""
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[2]
QCM=ROOT/'backend/src/hardware/devices/qcm410'

class OwnedIpv6ContextTests(unittest.TestCase):
    def test_extra_reads_require_owned_ipv6_profile_and_actual_mm_prefix(self):
        text=(QCM/'primary_ims_pcscf.rs').read_text()
        for gate in ('owned_profile', 'expected.ipv4_address.is_none()', 'rows.len() == 1', 'matches!(d.pdp_type.as_str(), "IPV6" | "IPV4V6")',
                     'pinned_ipv6_prefix(row, cid, &state, profile_id, expected)', 'expected.ipv6_prefix != Some(64)'):
            self.assertIn(''.join(gate.split()),''.join(text.split()))
        self.assertIn('discover_with_policy(expected, profile_id, apn, delay, false, read, at)',text)
        session=(QCM/'primary_ims_session.rs').read_text()
        self.assertIn('let owned_profile_context = pinned_bus.is_some() && request.profile_id.is_some()',session)
        self.assertIn('if self.controller.owned_profile_context',session)

    def test_read_only_full_address_and_all_other_context_exclusion(self):
        text=(QCM/'primary_ims_pcscf_separated.rs').read_text()
        for gate in ('!(2..=3).contains(&fields.len())', 'cid(fields[0])? != target', 'address != expected', 'Ipv4Addr::UNSPECIFIED',
                     'state.active.iter().copied().filter', 'other_context_prefix_ambiguous',
                     'other_context_family_unverified', 'other_context_bearer_ambiguous'):
            self.assertIn(''.join(gate.split()),''.join(text.split()))
        for forbidden in ('qmicli', 'AT+CGACT=', 'AT+CGDCONT=', 'set_link', 'candidates.push', 'parse_cgcontrdp_addresses'):
            self.assertNotIn(forbidden,text)

    def test_second_proof_precedes_final_mm_binding_validation(self):
        text=(QCM/'primary_ims_pcscf.rs').read_text()
        block=text.split('if let Some(proof) = separated_proof',1)[1].split('return Ok(ImsPcscfDiscovery',1)[0]
        self.assertLess(block.index('separated::observe('),block.index('if read().await? != *expected'))
        self.assertIn('context_state(',block)
        self.assertIn('context_changed',block)

    def test_both_suites_include_new_proof_and_private_bus_checks(self):
        for name in ('beta-validation.yml','build-release.yml'):
            text=(ROOT/'.github/workflows'/name).read_text()
            self.assertIn('hardware::devices::qcm410::primary_ims_pcscf::separated::tests',text)
            self.assertIn('hardware::devices::qcm410::primary_ims_lifecycle::ip_config_dbus_tests',text)

if __name__=='__main__':unittest.main()
