"""R8 identity-source policy: static seams only; execution is Actions-only."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[2]
IMS = ROOT/'backend/src/connectivity/modems/ims'

class IdentitySourcePolicyContract(unittest.TestCase):
    def test_policy_is_behavioral_and_default_safe(self):
        text = (IMS/'uicc_ims.rs').read_text(encoding='utf-8')
        policy = text[text.index('pub enum IdentitySourcePolicy'):text.index('pub fn resolve_ims_identity(')]
        # Legacy labels must not silently change behavior.
        self.assertIn('_ => Self::Auto', policy)
        self.assertIn('"isim_required" | "isim_only" => Self::RequireIsim', policy)
        self.assertIn('"usim_derived" | "derived" | "derived_imsi" => Self::UsimDerived', policy)
        resolve = text[text.index('pub fn resolve_ims_identity('):text.index('/// Captured physical subscription')]
        self.assertIn('policy: IdentitySourcePolicy', resolve)
        self.assertIn('if policy == IdentitySourcePolicy::UsimDerived {', resolve)
        self.assertIn('ims_identity_isim_required_missing', resolve)
        # USIM-derived must not borrow ISIM P-CSCF; RequireIsim must not derive.
        usim_branch = resolve[resolve.index('if policy == IdentitySourcePolicy::UsimDerived {'):
                              resolve.index('if let Some((aid, material)) = isim {')]
        self.assertIn('return Ok(resolved)', usim_branch)
        self.assertNotIn('material.pcscf', usim_branch)

    def test_both_access_legs_pass_the_profile_policy(self):
        cell = (IMS/'cellular_ims/live.rs').read_text(encoding='utf-8')
        wifi = (IMS/'vowifi/live_identity.rs').read_text(encoding='utf-8')
        for text in (cell, wifi):
            self.assertIn('IdentitySourcePolicy::parse(profile.ims.identity_source)', text)
        self.assertIn('uicc_ims::IdentitySourcePolicy::parse', cell)
        self.assertIn('uicc_ims::IdentitySourcePolicy::parse', wifi)

    def test_diagnostics_recognise_the_new_declared_values(self):
        diag = (ROOT/'backend/src/connectivity/core/registration_diagnostics.rs').read_text(encoding='utf-8')
        self.assertIn('"isim" | "isim_required" | "isim_only" => Self::Isim', diag)
        self.assertIn('"usim_derived" => Self::Derived', diag)

if __name__ == '__main__':
    unittest.main()
