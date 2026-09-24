"""Keep native discovery passive and separate from backend activation.

Runtime behavior is covered by discovery::tests in Actions; this guard checks
that the CLI cannot silently grow an active modem probe or owner takeover.
"""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
DISCOVERY = ROOT / "backend/src/hardware/cellular/backends/discovery.rs"


class NativeDiscoveryBoundaryTests(unittest.TestCase):
    def test_discovery_uses_only_read_only_filesystem_operations(self):
        source = DISCOVERY.read_text().split("#[cfg(all(test, unix))]", 1)[0]
        operations = set(re.findall(r"fs::([a-z_]+)\(", source))
        self.assertEqual(operations, {"canonicalize", "read_dir", "read_link", "read_to_string"})
        for forbidden in ("Command::", "Connection::", "OpenOptions", "File::", "unsafe", "run_at", "run_qmi"):
            self.assertNotIn(forbidden, source)

    def test_candidate_never_selects_an_at_hint_or_bearer_endpoint(self):
        source = DISCOVERY.read_text().split("fn candidate(", 1)[1].split("#[cfg", 1)[0]
        for value in ("at_device: None", "sms_reception_enabled: false", "ims: None", "data: None"):
            self.assertIn(value, source)
        self.assertNotIn(".first()", source)
        self.assertNotIn("modem.at_port_hint", source)
        self.assertIn("_ => return None", source)

    def test_cli_returns_before_opening_the_modem_backend(self):
        source = (ROOT / "backend/src/main.rs").read_text()
        block = source.split("if let Some(CliCommand::DiscoverNative", 1)[1].split(
            "if matches!(&cli.command, Some(CliCommand::InspectModems))", 1
        )[0]
        self.assertIn("return Ok(())", block)
        for forbidden in ("Connection::", "initialize(", "Database::", "ConfigManager::"):
            self.assertNotIn(forbidden, block)

    def test_discovery_regressions_run_in_both_workflows(self):
        for name in ("beta-validation.yml", "build-release.yml"):
            source = (ROOT / ".github/workflows" / name).read_text()
            self.assertIn("hardware::cellular::backends::discovery::tests", source)


if __name__ == "__main__":
    unittest.main()
