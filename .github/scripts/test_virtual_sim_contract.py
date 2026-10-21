"""Virtual USIM (five-code) module contract. Execution happens in Actions."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[2]
VS = ROOT/'backend/src/connectivity/modems/ims/virtual_sim'

class VirtualSimContract(unittest.TestCase):
    def test_module_layout_and_scope(self):
        self.assertTrue((VS/'mod.rs').is_file())
        for name in ['milenage.rs', 'sqn.rs', 'store.rs']:
            self.assertTrue((VS/name).is_file(), name)
        doc = (VS/'mod.rs').read_text(encoding='utf-8')
        self.assertIn('VoWiFi only', doc)
        self.assertIn('never replaces a modem line', doc)
        self.assertIn('no ISO 7816 file system', doc)
        ims = (ROOT/'backend/src/connectivity/modems/ims/mod.rs').read_text(encoding='utf-8')
        self.assertIn('pub mod virtual_sim;', ims)

    def test_sqn_rule_and_plain_record_format(self):
        sqn = (VS/'sqn.rs').read_text(encoding='utf-8')
        self.assertIn('pub fn accepts(self, network: Self) -> bool', sqn)
        self.assertIn('network.0 > self.0', sqn)
        self.assertIn('MASK48', sqn)
        store = (VS/'store.rs').read_text(encoding='utf-8')
        # Records are plaintext on the host by explicit decision; keys still
        # never reach logs or the API.
        self.assertNotIn('AES_256_GCM', store)
        self.assertNotIn('MasterKey', store)
        self.assertIn('ims_virtual_sim_op_opc_mismatch', store)
        self.assertIn('fn encode(&self) -> Vec<u8>', store)
        self.assertIn('ims_virtual_sim_record_version_unsupported', store)
        self.assertIn('op_kind', store)

    def test_wifi_identity_uses_the_software_usim_without_reading_the_card(self):
        live = (ROOT/'backend/src/connectivity/modems/ims/vowifi/live_identity.rs').read_text(encoding='utf-8')
        self.assertIn('virtual_sim::for_line(line_id)', live)
        self.assertIn('return load_virtual(line_id, profile, virtual_usim);', live)
        load_virtual = live[live.index('fn load_virtual('):live.index('#[cfg(test)]\npub(super) fn selection_for_test(')]
        # No card I/O on this path.
        for forbidden in ['capture_card_binding', 'read_isim', 'qmi_uim', 'spawn_blocking']:
            self.assertNotIn(forbidden, load_virtual)
        self.assertIn('IdentitySourcePolicy::UsimDerived', load_virtual)
        self.assertIn('virtual:', load_virtual)
        # AKA and verification branch on the software USIM.
        self.assertIn('profile.authenticate(rand, autn)?', live)
        self.assertIn('ims_virtual_sim_changed', live)
        self.assertIn('if self.virtual_usim.is_some() { return self.verify(line_id).await; }', live)

    def test_uses_standard_algorithms_only(self):
        text = (VS/'milenage.rs').read_text(encoding='utf-8')
        self.assertIn('use aes::Aes128', text)
        self.assertIn('derive_opc', text)
        self.assertIn('ims_virtual_sim_autn_mac_invalid', text)
        # AUTN layout and AUTS length are fixed by the standard.
        self.assertIn('pub const AUTN_LEN: usize = 16', text)
        self.assertIn('pub const AUTS_LEN: usize = 14', text)

    def test_key_material_is_never_formatted(self):
        text = (VS/'milenage.rs').read_text(encoding='utf-8')
        self.assertIn('Milenage(<redacted>)', text)
        for forbidden in ['tracing::info!(', 'tracing::warn!(', 'println!']:
            self.assertNotIn(forbidden, text)

    def test_vectors_come_from_the_standard(self):
        tests = (VS/'milenage_tests.rs').read_text(encoding='utf-8')
        for vector in ['465b5ce8b199b49faa5f0a2ee238a6bc', 'cd63cb71954a9f4e48a5994e37a02baf',
                       'a54211d5e3ba50bf', 'b40ba9a3c58b2a05bbf0d987b21bf8cb',
                       'f769bcd751044604127672711c6d3441', '4a9ffac354dfafb3',
                       '01cfaf9ec4e871e9', '451e8beca43b', '9e2980c59739da67b136355e3cede6a2']:
            self.assertIn(vector, tests)

    def test_actions_run_the_module(self):
        for name in ['build-release.yml', 'beta-validation.yml']:
            text = (ROOT/'.github/workflows'/name).read_text(encoding='utf-8')
            self.assertIn('connectivity::modems::ims::virtual_sim::milenage::tests', text)
            self.assertIn('Empty regression filter', text)

if __name__ == '__main__':
    unittest.main()
