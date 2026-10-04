"""Registration builds are Actions-only, and new suites cannot be silently skipped."""
import contextlib
import importlib.util
import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("offline_registration_runner", ROOT / "offline-registration-sim/run.py")
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class OfflineRegistrationCiTests(unittest.TestCase):
    def test_non_actions_execution_rejected_before_build_or_evidence_write(self):
        for value in (None, "", "false", "1"):
            with self.subTest(value=value), tempfile.TemporaryDirectory() as directory:
                report = Path(directory) / "new" / "report.json"
                env = {} if value is None else {"GITHUB_ACTIONS": value}
                with patch.dict(os.environ, env, clear=True), \
                     patch("sys.argv", ["run.py", "--report", str(report)]), \
                     patch.object(runner.subprocess, "run") as build, \
                     contextlib.redirect_stderr(io.StringIO()), \
                     self.assertRaises(SystemExit) as error:
                    runner.main()
                self.assertEqual(error.exception.code, 2)
                build.assert_not_called()
                self.assertFalse(report.parent.exists())

    def test_both_workflows_execute_drain_and_both_registration_matrices(self):
        for name in ("beta-validation.yml", "build-release.yml"):
            with self.subTest(workflow=name):
                text = (ROOT / ".github/workflows" / name).read_text(encoding="utf-8")
                self.assertIn("profile_lease::runtime::switch_drain_tests \\", text)
                self.assertIn("python3 -B offline-registration-sim/run.py \\", text)
                self.assertIn("python3 -B offline-registration-sim/run.py --history \\", text)
                self.assertIn("--report offline-registration-sim/ci-results/standard.json", text)
                self.assertIn("--report offline-registration-sim/ci-results/history.json", text)
                self.assertIn("if: always()", text)
                self.assertIn("offline-registration-sim/ci-results/", text)

    def test_evidence_binds_runner_history_and_actions_identity(self):
        for path in ("offline-registration-sim/run.py", "offline-registration-sim/history.rs",
                     "backend/src/connectivity/modems/ims/cellular_ims/security_agreement.rs"):
            self.assertIn(path, runner.SOURCES)
        source = (ROOT / "offline-registration-sim/run.py").read_text(encoding="utf-8")
        for name in ("GITHUB_REPOSITORY", "GITHUB_SHA", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"):
            self.assertIn(name, source)
        self.assertIn("expected_count=18 if args.history else 24", source)


if __name__ == "__main__":
    unittest.main()
