"""Static guards for a scoped restoration of the September CMCC header policy."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[2]

class CmccHistoricalDeclarationTests(unittest.TestCase):
    def test_only_known_home_plmns_restore_flags_not_security_or_identity(self):
        source = (ROOT / 'backend/src/connectivity/modems/ims/vowifi/profiles.rs').read_text()
        helper = source.split('fn proactive_derived_sec_agree(', 1)[1].split('pub fn derive_standard_3gpp_profile(', 1)[0]
        self.assertIn('("460", "00" | "02")', helper)
        self.assertNotIn('registered_plmn', helper)
        self.assertEqual(source.count('proactive_derived_sec_agree(mcc, mnc, access)'), 2)
        factory = source.split('pub fn derive_standard_3gpp_profile(', 1)[1].split('pub fn is_standard_derived_profile', 1)[0]
        self.assertIn('Standard3gppAccess::LteEpc => "aka_empty"', factory)
        self.assertIn('strict_security_server_offer: matches!(access, Standard3gppAccess::LteEpc)', factory)
        self.assertNotIn('hmac-md5-96', factory)
        self.assertIn('sec_agree_mode: "auto"', factory)

    def test_both_gates_run_factory_and_controlled_ab_matrix(self):
        for name in ('beta-validation.yml', 'build-release.yml'):
            text = (ROOT / '.github/workflows' / name).read_text()
            self.assertIn('vowifi::profiles::cmcc_regression_tests', text)
            self.assertIn('offline-registration-sim/run.py --cmcc-regression', text)
            self.assertIn('ci-results/cmcc-regression.json', text)
            self.assertIn('cellular_ims::live::tests', text)
        sim = (ROOT / 'offline-registration-sim/simulator.rs').read_text()
        self.assertIn('cmcc_regressed_primary', sim)
        self.assertIn('Controlled A/B of the two 39b387b flags', sim)
        runner = (ROOT / 'offline-registration-sim/run.py').read_text()
        self.assertIn('expected_count=8 if args.cmcc_regression', runner)
        self.assertIn('offline-registration-sim/cmcc_regression.rs', runner)

if __name__ == '__main__': unittest.main()
