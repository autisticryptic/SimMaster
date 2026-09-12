from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]


class ModemObservationBoundaryTests(unittest.TestCase):
    def test_registry_uses_provider_without_dbus_or_mm_calls(self):
        text = (ROOT / "backend/src/services/line_registry.rs").read_text()
        self.assertNotIn("use zbus::", text)
        self.assertNotIn("modem_manager::", text)
        self.assertNotIn("serving_access_snapshot(", text)
        self.assertIn("self.observations.discover().await", text)
        self.assertIn("self.observations.serving_access(binding).await", text)

    def test_provider_contract_and_bindings_do_not_require_a_bus(self):
        for name in ("bindings.rs", "observations.rs"):
            with self.subTest(module=name):
                text = (ROOT / "backend/src/hardware/cellular" / name).read_text()
                self.assertNotIn("use zbus::", text)
                self.assertNotIn("modem_manager::", text)
                self.assertNotIn("Connection::system(", text)

    def test_new_regressions_are_executed_by_both_candidate_workflows(self):
        for name in ("build-release.yml", "beta-validation.yml"):
            text = (ROOT / ".github/workflows" / name).read_text()
            for group in (
                "hardware::cellular::bindings::tests",
                "hardware::cellular::observations::tests",
                "hardware::cellular::mm_observations::tests",
                "services::line_registry::tests",
            ):
                with self.subTest(workflow=name, group=group):
                    self.assertIn(group + " \\", text)


if __name__ == "__main__":
    unittest.main()
