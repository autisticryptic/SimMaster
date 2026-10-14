#!/usr/bin/env python3
"""Static schema/production-wiring guards; no Rust, node, SIP, or hardware run."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]

def read(path):
    return (ROOT / path).read_text(encoding="utf-8")

class ImsExecutionDiagnosticsContract(unittest.TestCase):
    def test_rust_and_typescript_projection_fields_match(self):
        rust = read("backend/src/connectivity/core/registration_diagnostics.rs")
        ts = read("frontend/src/api/contracts.ts")
        rust_body = rust.split("pub struct RegistrationDiagnostics {", 1)[1].split("\n}", 1)[0]
        ts_body = ts.split("export interface ImsRegistrationDiagnostics {", 1)[1].split("\n}", 1)[0]
        rust_fields = set(re.findall(r"\bpub (\w+):", rust_body))
        ts_fields = set(re.findall(r"^  (\w+)\??:", ts_body, re.M))
        self.assertEqual(rust_fields, ts_fields)
        self.assertTrue({"requested_family", "granted_family", "offered_ipsec", "negotiated_ipsec", "negotiated_from_success", "wait", "failure"} <= rust_fields)
        self.assertFalse({"imsi", "impi", "impu", "private_user", "public_uri", "raw_sip", "nonce", "ck", "ik", "aid", "authorization"} & rust_fields)

    def test_status_projections_use_execution_state_not_profile_estimates(self):
        cell = read("backend/src/connectivity/modems/ims/cellular_ims/runtime.rs")
        wifi = read("backend/src/connectivity/modems/ims/vowifi/runtime.rs")
        api = read("backend/src/api/handlers.rs")
        self.assertIn("diagnostics: s.diagnostics.report()", cell)
        self.assertIn("diagnostics: self.diagnostics.report()", wifi)
        self.assertIn("state.three_gpp.diagnostics = cellular_ims.diagnostics.report()", api)
        self.assertIn("state.non_three_gpp.diagnostics = vowifi.diagnostics", api)
        self.assertIn("next.diagnostics = current.diagnostics.clone()", wifi)
        self.assertIn("with_registration_diagnostics", wifi)

    def test_real_adapters_publish_family_offer_and_success(self):
        cell = read("backend/src/connectivity/modems/ims/cellular_ims/live.rs")
        native = read("backend/src/connectivity/modems/ims/cellular_ims/native_bearer.rs")
        wifi = read("backend/src/connectivity/modems/ims/vowifi/live.rs")
        self.assertIn("establish_native_ims_bearer_observed", cell)
        self.assertIn("observe_request(attempt.as_mm_str())", native)
        self.assertIn('observe_request(if forced == 4 { "ipv4" } else { "ipv6" })', native)
        self.assertIn("diagnostics.accepted(diagnostic_route.transport", cell)
        refresh = read("backend/src/connectivity/modems/ims/vowifi/register_refresh_live.rs")
        self.assertIn("diagnostic_scope.accepted(channel.route()", refresh)
        self.assertIn("self.refresh_with_recovery(channel, Some(original_expiry))", wifi)
        self.assertIn("diagnostic_scope.granted_family(session.child_sa", wifi)
        self.assertIn("diagnostic_scope.requested_family(forced_stack)", wifi)

    def test_actions_run_nonempty_backend_and_original_workbench_regressions(self):
        for path in [".github/workflows/build-release.yml", ".github/workflows/beta-validation.yml"]:
            source = read(path)
            for filter_ in ["connectivity::core::registration_diagnostics::tests", "cellular_ims::runtime::diagnostics_tests", "vowifi::live::diagnostics_live::tests", "ims_execution_diagnostics_api_is_same_runtime_projection_without_changing_intents"]:
                self.assertIn(filter_, source)
            self.assertIn("selected no tests", source)
        self.assertIn("imsDiagnosticsFormat.test.ts", read("frontend/package.json"))
        self.assertIn("ims-execution-diagnostics.spec.ts", read("frontend/playwright.lines.config.ts"))
        browser = read("frontend/e2e/ims-execution-diagnostics.spec.ts")
        self.assertIn("/e2e/fixtures/lines.html", browser)
        self.assertNotIn("test.skip", browser)
        self.assertIn("expect(mutations).toEqual([])", browser)

    def test_ui_is_optional_readonly_and_bruno_documents_actual_facts(self):
        panel = read("frontend/src/pages/sim/ImsExecutionDetails.tsx")
        formatter = read("frontend/src/pages/sim/imsDiagnosticsFormat.ts")
        self.assertIn("!rows.length", panel)
        self.assertNotIn("fetch(", panel)
        self.assertNotIn("api.", panel)
        self.assertIn("facts.version !== 1", formatter)
        self.assertIn("response_retry_after_seconds", formatter)
        self.assertIn("wait.retry_after_seconds", formatter)
        self.assertNotIn("JSON.stringify(facts", panel + formatter)
        bruno = read("bruno-api/get_ims_status.bru")
        self.assertIn("negotiated_from_success", bruno)
        self.assertIn("response_retry_after_seconds", bruno)
        self.assertIn("tests {", bruno)

if __name__ == "__main__":
    unittest.main()
