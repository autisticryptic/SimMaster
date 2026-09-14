"""Structural guards supplement, but do not replace, Rust regressions on Actions."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "backend/src/connectivity/modems/ims"


class ImsFallbackBoundaryTests(unittest.TestCase):
    def test_runtime_uses_slot_bound_aid_before_sim_identity_fallback(self):
        text = (SRC / "cellular_ims/live.rs").read_text()
        load = text[text.index("async fn load_device_identity("):text.index("async fn load_uicc_applications(")]
        self.assertLess(load.index("load_uicc_applications(device)"), load.index("resolve_fallback_imsi("))
        self.assertIn("parse_uicc_applications_for_slot(&output, device.uim_slot)", text)
        self.assertIn("read_uim_identity(device, &aka_aid)", load)
        self.assertIn("identity::read_mnc_length_via_at", load)
        self.assertIn("control::at_command(&device.modem_id, command)", load)

    def test_profile_definition_is_checked_before_any_cgdcont_write(self):
        text = (SRC / "cellular_ims/pcscf.rs").read_text()
        prepare = text[text.index("pub async fn prepare_ims_profile_context("):text.index("fn select_ims_profile_context(")]
        self.assertLess(prepare.index("select_ims_profile_context("), prepare.index('AT+CGDCONT='))
        self.assertLess(prepare.index("ensure_profile_inactive("), prepare.index('AT+CGDCONT='))
        self.assertIn("volte_ims_preferred_profile_occupied", text)
        self.assertIn("volte_ims_profile_definition_ambiguous", text)

    def test_both_automatic_resolvers_use_the_same_home_boundary(self):
        text = (SRC / "vowifi/profile_store.rs").read_text()
        self.assertEqual(text.count("automatic_home_plmn_hint("), 3)
        self.assertNotIn("cmp(&left.1.meta.plmn.len())", text)
        self.assertIn("automatic_sources_refuse_conflicting_custom_and_catalog_home_boundaries", text)

    def test_ci_selects_the_fallback_and_end_to_end_batch_regressions(self):
        for name in ("beta-validation.yml", "build-release.yml"):
            text = (ROOT / ".github/workflows" / name).read_text()
            for group in ("cellular_ims::identity::tests", "cellular_ims::plan::tests",
                          "cellular_ims::native_bearer::tests", "cellular_ims::pcscf::tests",
                          "vowifi::profile_store::tests", "vowifi::profile_record::tests",
                          "api::handlers::tests::cellular_ims_profile_batch"):
                self.assertIn(group, text)
        package = (ROOT / "frontend/package.json").read_text()
        self.assertIn("tests/cellularImsErrorFormat.test.ts", package)


if __name__ == "__main__":
    unittest.main()
