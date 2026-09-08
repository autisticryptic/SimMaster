import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from release_version import resolve_version


class ReleaseVersionTests(unittest.TestCase):
    def test_explicit_beta_versions_on_push(self):
        for version in ("1.1.4-beta1", "1.1.4-beta2", "1.1.4-beta3", "1.1.4-beta.1", "1.1.4-rc.1"):
            with self.subTest(version=version):
                result = resolve_version(version)
                self.assertEqual(result["version"], version)
                self.assertEqual(result["prerelease"], "true")
                self.assertEqual(result["make_latest"], "false")
                self.assertEqual(result["release_type"], "pre-release")

    def test_stable_version_is_not_incremented(self):
        result = resolve_version("1.1.4")
        self.assertEqual(result["version"], "1.1.4")
        self.assertEqual(result["prerelease"], "false")
        self.assertEqual(result["make_latest"], "true")

    def test_push_ignores_dispatch_version(self):
        result = resolve_version("1.1.4-beta1", "1.1.8", "push")
        self.assertEqual(result["version"], "1.1.4-beta1")

    def test_dispatch_cannot_promote_beta_to_latest(self):
        result = resolve_version("1.1.4", "1.1.4-beta2", "workflow_dispatch", "最新正式版（latest release）")
        self.assertEqual(result["version"], "1.1.4-beta2")
        self.assertEqual(result["prerelease"], "true")
        self.assertEqual(result["make_latest"], "false")

    def test_dispatch_auto_uses_file(self):
        for requested in ("auto", "", "  auto\n"):
            result = resolve_version("1.1.4-beta1\n", requested, "workflow_dispatch")
            self.assertEqual(result["version"], "1.1.4-beta1")
            self.assertEqual(result["prerelease"], "true")

    def test_dispatch_can_explicitly_prerelease_stable_version(self):
        result = resolve_version("1.1.4", event="workflow_dispatch", release_type="预发布版（pre-release）")
        self.assertEqual(result["prerelease"], "true")
        self.assertEqual(result["make_latest"], "false")

    def test_dispatch_stable_can_be_latest(self):
        result = resolve_version("1.1.4", event="workflow_dispatch", release_type="最新正式版（latest release）")
        self.assertEqual(result["version"], "1.1.4")
        self.assertEqual(result["make_latest"], "true")

    def test_surrounding_whitespace_only(self):
        self.assertEqual(resolve_version(" 1.1.4-beta1\r\n")["version"], "1.1.4-beta1")
        for version in ("1.1.4 beta1", "1.1. 4", "1.1.4\nmake_latest=true"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                resolve_version(version)

    def test_invalid_versions_fail_before_publish(self):
        for version in ("", "auto", "v1.1.4", "1.1", "1.1.4.5", "../1.1.4", "1.1.4-", "01.1.4", "1.1.4-beta.01", "1.1.4-beta_1"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                resolve_version(version)

    def test_unrecognized_event_fails_closed(self):
        with self.assertRaises(ValueError):
            resolve_version("1.1.4-beta1", event="pull_request")

    def test_workflow_uses_tested_prerelease_outputs(self):
        workflow = Path(__file__).resolve().parents[1] / "workflows/build-release.yml"
        text = workflow.read_text(encoding="utf8")
        self.assertIn("run: python3 .github/scripts/release_version.py", text)
        self.assertIn("prerelease: ${{ fromJSON(needs.prepare.outputs.prerelease) }}", text)
        self.assertIn("make_latest: ${{ needs.prepare.outputs.make_latest }}", text)
        self.assertIn("target_commitish: ${{ github.sha }}", text)
        self.assertIn("tag_name: v${{ needs.prepare.outputs.version }}", text)

    def test_development_candidates_cannot_publish_or_retag_on_push(self):
        workflow = Path(__file__).resolve().parents[1] / "workflows/build-release.yml"
        text = workflow.read_text(encoding="utf8")
        before_release, release = text.split("\n  release:\n", 1)
        self.assertNotIn("uses: softprops/action-gh-release@", before_release)
        self.assertIn(
            "if: github.event_name != 'push' || github.ref == 'refs/heads/master'",
            release.split("    steps:", 1)[0],
        )
        self.assertIn("needs: [prepare, build, check-tests]", release)

    def test_cli_outputs_are_explicit_and_run_number_independent(self):
        script = Path(__file__).with_name("release_version.py").resolve()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "VERSION").write_text("1.1.4-beta1\n", encoding="utf8")
            output = root / "output.txt"
            env = {**os.environ, "GITHUB_OUTPUT": str(output), "EVENT_NAME": "push",
                   "INPUT_VERSION": "auto", "GITHUB_RUN_NUMBER": "98765"}
            subprocess.run([sys.executable, str(script)], cwd=root, env=env, check=True, capture_output=True)
            values = dict(line.split("=", 1) for line in output.read_text(encoding="utf8").splitlines())
            self.assertEqual(values["version"], "1.1.4-beta1")
            self.assertEqual(values["prerelease"], "true")
            self.assertEqual(values["make_latest"], "false")
            self.assertNotIn("98765", output.read_text())


if __name__ == "__main__":
    unittest.main()
