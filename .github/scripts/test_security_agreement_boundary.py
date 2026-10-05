"""The server-list parser is pure and its Rust regressions run in both CI gates."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
BASE = ROOT / "backend/src/connectivity/modems/ims/cellular_ims"


class SecurityAgreementBoundaryTests(unittest.TestCase):
    def test_both_workflows_execute_the_new_regressions(self):
        for name in ["build-release.yml", "beta-validation.yml"]:
            text = (ROOT / ".github/workflows" / name).read_text(encoding="utf8")
            self.assertIn("connectivity::modems::ims::cellular_ims::security_agreement::tests", text)
            self.assertIn("connectivity::modems::ims::cellular_ims::live::tests", text)
        self.assertIn("mod security_agreement;", (BASE / "mod.rs").read_text(encoding="utf8"))

    def test_parser_has_no_hardware_or_process_side_effects(self):
        source = (BASE / "security_agreement.rs").read_text(encoding="utf8")
        production = source.split("#[cfg(test)]", 1)[0]
        for forbidden in ["Command::", "run_ip(", "install_plan", "TcpStream", "UdpSocket", "std::fs"]:
            self.assertNotIn(forbidden, production)
        self.assertIn("MAX_HEADER_BYTES", production)
        self.assertIn("MAX_OFFERS", production)
        self.assertIn('verify: values.join(", ")', production)

    def test_installation_uses_selected_algorithms_not_the_entire_verify_list(self):
        source = (BASE / "live.rs").read_text(encoding="utf8")
        for expected in ["let selected = agreement.binding;", "let verify = agreement.verify;",
                         "let algs = agreement.algorithms;", "super::security_agreement::select("]:
            self.assertIn(expected, source)
        self.assertNotIn("ipsec::xfrm_algs_from_security_server(&verify)", source)
        self.assertIn("client_offer_preserves_all_explicitly_configured_mechanisms", source)
        builder=source[source.index('impl CellularImsSecurityClientOffer'):source.index('impl CellularImsInitialAuthorization')]
        self.assertNotIn('.first()',builder)
        self.assertIn('super::security_agreement::client_offer(',builder)

    def test_client_offer_and_null_key_boundaries_are_exercised(self):
        for name in ('build-release.yml','beta-validation.yml'):
            text=(ROOT/'.github/workflows'/name).read_text()
            self.assertIn('cellular_ims::ipsec::tests',text)
            self.assertIn('cellular_ims::live::refresh_tests',text)
        parser=(BASE/'security_agreement.rs').read_text()
        self.assertIn('allowed.len() > MAX_OFFERS',parser)
        self.assertIn('fields.join(separator)',parser)
        adapter=(ROOT/'offline-registration-sim/cellular_adapter.rs').read_text()
        self.assertIn('variant.build_security_offer(',adapter)
        self.assertIn('select_security_server_for_mechanism(self.profile',adapter)


if __name__ == "__main__":
    unittest.main()
