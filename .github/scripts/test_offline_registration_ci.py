"""Static/mocked checks only: no compiler or Rust simulation is executed here."""
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("offline_registration_runner", ROOT / "offline-registration-sim/run.py")
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)
FLAGS = {"standard": [], "history": ["--history"], "security_hint": ["--security-hint"], "fallback": ["--fallback"]}


class OfflineRegistrationCiTests(unittest.TestCase):
    def test_non_actions_execution_rejected_before_build_or_evidence_write(self):
        for value in (None, "", "false", "1"):
            for flags in FLAGS.values():
                with self.subTest(value=value, flags=flags), tempfile.TemporaryDirectory() as directory:
                    report = Path(directory) / "new" / "report.json"
                    env = {} if value is None else {"GITHUB_ACTIONS": value}
                    with patch.dict(os.environ, env, clear=True), \
                         patch("sys.argv", ["run.py", *flags, "--report", str(report)]), \
                         patch.object(runner.subprocess, "run") as build, \
                         contextlib.redirect_stderr(io.StringIO()), \
                         self.assertRaises(SystemExit) as error:
                        runner.main()
                    self.assertEqual(error.exception.code, 2)
                    build.assert_not_called()
                    self.assertFalse(report.parent.exists())

    def test_matrix_flags_are_mutually_exclusive_before_build(self):
        for other in ("--history", "--security-hint"):
            with patch("sys.argv", ["run.py", "--fallback", other]), \
                 patch.object(runner.subprocess, "run") as build, \
                 contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                runner.main()
            build.assert_not_called()

    def test_both_workflows_execute_drain_and_all_registration_matrices(self):
        for name in ("beta-validation.yml", "build-release.yml"):
            with self.subTest(workflow=name):
                text = (ROOT / ".github/workflows" / name).read_text(encoding="utf-8")
                self.assertIn("profile_lease::runtime::switch_drain_tests \\", text)
                self.assertIn("cellular_ims::live::register_fallback::tests \\", text)
                for flag, report in (("", "standard"), (" --history", "history"),
                                     (" --security-hint", "security-hint"), (" --fallback", "fallback")):
                    self.assertIn(f"python3 -B offline-registration-sim/run.py{flag} \\", text)
                    self.assertIn(f"--report offline-registration-sim/ci-results/{report}.json", text)
                self.assertIn("if: always()", text)
                self.assertIn("offline-registration-sim/ci-results/", text)

    def test_evidence_binds_real_fallback_and_every_fixture(self):
        for path in ("offline-registration-sim/run.py", "offline-registration-sim/history.rs",
                     "offline-registration-sim/security_hint.rs", "offline-registration-sim/fallback.rs",
                     "offline-registration-sim/simulator.rs", "offline-registration-sim/cellular_adapter.rs",
                     "backend/src/connectivity/modems/ims/cellular_ims/security_agreement.rs",
                     "backend/src/connectivity/modems/ims/cellular_ims/register_fallback.rs"):
            self.assertIn(path, runner.SOURCES)
            self.assertEqual(runner.fingerprints()[path], hashlib.sha256((ROOT / path).read_bytes()).hexdigest())
        self.assertEqual(len(runner.SOURCES), len(set(runner.SOURCES)))
        self.assertEqual({key: row[2] for key, row in runner.MATRICES.items()},
                         {"standard": 24, "history": 18, "security_hint": 12, "fallback": 32})

    def run_mocked(self, matrix, mutate=None, *, log="test result: ok. 1 passed; 0 failed;", returncode=0,
                   fingerprints=None):
        """All subprocess calls are intercepted, even when evidence is invalid."""
        test_filter, report_variable, count, suite_id, _ = runner.MATRICES[matrix]
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "report.json"
            data = {"suite_id": suite_id, "passed": True, "hardware_used": False, "live_network_verified": False,
                    "scenarios": [{"id": f"fixture-{i}", "passed": True, "expected_success": bool(i % 2),
                                   "observed_success": bool(i % 2)} for i in range(count)]}
            if mutate:
                mutate(data)

            def fake_build(command, **kwargs):
                self.assertIn(test_filter, command)
                for flag in ("--locked", "--offline", "--nocapture", "--test-threads=1"):
                    self.assertIn(flag, command)
                self.assertEqual(kwargs["cwd"], ROOT)
                self.assertEqual(kwargs["env"]["GITHUB_ACTIONS"], "true")
                self.assertEqual(Path(kwargs["env"][report_variable]), report.with_suffix(".raw.json"))
                for other in runner.MATRICES.values():
                    if other[1] != report_variable:
                        self.assertNotIn(other[1], kwargs["env"])
                report.with_suffix(".raw.json").write_text(json.dumps(data), encoding="utf-8")
                kwargs["stdout"].write(log)
                return SimpleNamespace(returncode=returncode)

            env = {"GITHUB_ACTIONS": "true", "GITHUB_REPOSITORY": "synthetic/repository", "GITHUB_SHA": "fixture-sha",
                   "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2", "GITHUB_WORKFLOW": "mocked-static-test"}
            with patch.dict(os.environ, env, clear=True), \
                 patch("sys.argv", ["run.py", *FLAGS[matrix], "--report", str(report), "--cargo", "never-executed-cargo"]), \
                 patch.object(runner, "fingerprints", side_effect=fingerprints or [{"fixture": "sha"}] * 2), \
                 patch.object(runner.subprocess, "run", side_effect=fake_build) as build, \
                 contextlib.redirect_stdout(io.StringIO()):
                try:
                    runner.main()
                except RuntimeError:
                    self.assertFalse(report.exists(), "invalid evidence must not be published")
                    raise
            build.assert_called_once()
            result = json.loads(report.read_text(encoding="utf-8"))
            self.assertEqual(result["source_files_sha256"], {"fixture": "sha"})
            self.assertEqual(result["execution_environment"], "github_actions")
            self.assertEqual(result["github_actions"]["commit"], "fixture-sha")
            self.assertEqual(result["github_actions"]["run_attempt"], "2")
            self.assertEqual(result["log_sha256"], hashlib.sha256(log.encode()).hexdigest())
            self.assertEqual(result["test_count"], 1)
            return result

    def test_all_matrices_select_exact_filter_environment_and_count_without_running_rust(self):
        for matrix in FLAGS:
            with self.subTest(matrix=matrix):
                result = self.run_mocked(matrix)
                self.assertEqual(len(result["scenarios"]), runner.MATRICES[matrix][2])
                self.assertEqual(result["interpretation"], runner.MATRICES[matrix][4])

    def test_fallback_rejects_incomplete_duplicate_failed_or_wrong_suite_evidence(self):
        mutations = {
            "missing": lambda d: d["scenarios"].pop(),
            "extra": lambda d: d["scenarios"].append(dict(d["scenarios"][0], id="extra")),
            "duplicate": lambda d: d["scenarios"][1].update(id=d["scenarios"][0]["id"]),
            "failed": lambda d: d["scenarios"][0].update(passed=False),
            "mismatch": lambda d: d["scenarios"][0].update(observed_success=True),
            "wrong_suite": lambda d: d.update(suite_id=runner.MATRICES["security_hint"][3]),
            "live": lambda d: d.update(live_network_verified=True),
            "hardware": lambda d: d.update(hardware_used=True),
            "report_failed": lambda d: d.update(passed=False),
        }
        for name, mutation in mutations.items():
            with self.subTest(name=name), self.assertRaises(RuntimeError):
                self.run_mocked("fallback", mutation)

    def test_skipped_failed_or_source_drifted_fallback_cannot_publish(self):
        for kwargs in ({"log": "test result: ok. 0 passed; 0 failed;"}, {"returncode": 1},
                       {"fingerprints": [{"fixture": "before"}, {"fixture": "after"}]}):
            with self.subTest(kwargs=kwargs), self.assertRaises(RuntimeError):
                self.run_mocked("fallback", **kwargs)


if __name__ == "__main__":
    unittest.main()
