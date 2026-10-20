"""Virtual USIM (five-code) module contract. Execution happens in Actions."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[2]
VS = ROOT/'backend/src/connectivity/modems/ims/virtual_sim'

class VirtualSimContract(unittest.TestCase):
    def test_module_layout_and_scope(self):
        self.assertTrue((VS/'mod.rs').is_file())
        self.assertTrue((VS/'milenage.rs').is_file())
        doc = (VS/'mod.rs').read_text(encoding='utf-8')
        self.assertIn('VoWiFi only', doc)
        self.assertIn('never replaces a modem line', doc)
        self.assertIn('no ISO 7816 file system', doc)
        ims = (ROOT/'backend/src/connectivity/modems/ims/mod.rs').read_text(encoding='utf-8')
        self.assertIn('pub mod virtual_sim;', ims)

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
        for vector in ['465b5ce8b199b49faa9039792c26e45e', 'cd63cb71954a9c4e270f0bcda89d3b7f',
                       'a54211d5e3ba50bf', 'b40ba9a3c58b2a05bbf0f5c1a5cc2c3a',
                       'f769bcd75137d5d607f5f3d0f3dc6b3f', '4a9ffac354b34760',
                       '0185e1339c2541ca', '451e8beca43b']:
            self.assertIn(vector, tests)

    def test_actions_run_the_module(self):
        for name in ['build-release.yml', 'beta-validation.yml']:
            text = (ROOT/'.github/workflows'/name).read_text(encoding='utf-8')
            self.assertIn('connectivity::modems::ims::virtual_sim::milenage::tests', text)
            self.assertIn('Empty regression filter', text)

if __name__ == '__main__':
    unittest.main()
