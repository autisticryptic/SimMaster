import json
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest

from sync_release_version import check_versions, read_versions, sync_versions

ROOT = Path(__file__).resolve().parents[2]


def fixture(root, version="1.1.8"):
    (root / "backend").mkdir()
    (root / "frontend").mkdir()
    (root / "VERSION").write_text(version + "\n", encoding="utf8")
    (root / "backend/Cargo.toml").write_text(
        f'[package]\nname = "simadmin"\nversion = "{version}"\nedition = "2021"\n\n'
        '[dependencies]\nexample = "1.1.8"\n', encoding="utf8",
    )
    (root / "backend/Cargo.lock").write_text(
        '# Generated lockfile\nversion = 4\n\n[[package]]\nname = "example"\nversion = "1.1.8"\n\n'
        f'[[package]]\nname = "simadmin"\nversion = "{version}"\ndependencies = ["example"]\n\n'
        '[[package]]\nname = "trailing"\nversion = "0.3.0"\n', encoding="utf8",
    )
    (root / "frontend/package.json").write_text(json.dumps({
        "name": "simadmin-web", "private": True, "version": version,
        "dependencies": {"example": "1.1.8"},
    }, indent=2) + "\n", encoding="utf8")


class ReleaseManifestTests(unittest.TestCase):
    def test_repository_versions_match(self):
        version = check_versions(ROOT)
        self.assertEqual(set(read_versions(ROOT).values()), {version})

    def test_explicit_downgrade_updates_all_manifests_not_dependencies(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root)
            self.assertEqual(sync_versions(root, "1.1.5"), "1.1.5")
            self.assertEqual(set(read_versions(root).values()), {"1.1.5"})
            cargo = tomllib.loads((root / "backend/Cargo.toml").read_text())
            lock = tomllib.loads((root / "backend/Cargo.lock").read_text())
            self.assertEqual(cargo["dependencies"]["example"], "1.1.8")
            self.assertEqual(lock["version"], 4)
            self.assertEqual({p["name"]: p["version"] for p in lock["package"]},
                             {"example": "1.1.8", "simadmin": "1.1.5", "trailing": "0.3.0"})
            self.assertEqual(json.loads((root / "frontend/package.json").read_text())["dependencies"],
                             {"example": "1.1.8"})

    def test_sync_is_idempotent_and_supports_prereleases(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root)
            for version in ["1.1.5", "1.1.6-beta.1"]:
                sync_versions(root, version)
                before = {p: p.read_bytes() for p in root.rglob("*") if p.is_file()}
                sync_versions(root, version)
                self.assertEqual(before, {p: p.read_bytes() for p in before})
                self.assertEqual(check_versions(root), version)

    def test_invalid_version_never_writes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root)
            before = {p: p.read_bytes() for p in root.rglob("*") if p.is_file()}
            for value in ["auto", "v1.1.5", "1.1.5\nmake_latest=true", "1.1.5-01", "../1.1.5"]:
                with self.subTest(version=value), self.assertRaises(ValueError):
                    sync_versions(root, value)
                self.assertEqual(before, {p: p.read_bytes() for p in before})

    def test_drift_is_rejected_by_check(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root)
            (root / "VERSION").write_text("1.1.5\n")
            with self.assertRaisesRegex(ValueError, "Version drift"):
                check_versions(root)

    def test_ambiguous_lock_package_rejected_before_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root)
            lock = root / "backend/Cargo.lock"
            lock.write_text(lock.read_text() + '\n[[package]]\nname = "simadmin"\nversion = "9.9.9"\n')
            before = {p: p.read_bytes() for p in root.rglob("*") if p.is_file()}
            with self.assertRaises(ValueError):
                sync_versions(root, "1.1.5")
            self.assertEqual(before, {p: p.read_bytes() for p in before})

    def test_cli_sync_and_readonly_check(self):
        script = Path(__file__).with_name("sync_release_version.py")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root)
            for options in [["1.1.5"], ["--check"], ["1.1.5", "--check"]]:
                result = subprocess.run([sys.executable, str(script), "--root", str(root), *options],
                                        capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("consistent: 1.1.5", result.stdout)

    def test_workflow_syncs_test_and_build_versions_and_keeps_lockfile(self):
        workflow = (ROOT / ".github/workflows/build-release.yml").read_text(encoding="utf8")
        prepare = workflow.split("\n  prepare:\n", 1)[1].split("\n  check-tests:\n", 1)[0]
        tests = workflow.split("\n  check-tests:\n", 1)[1].split("\n  build:\n", 1)[0]
        build = workflow.split("\n  build:\n", 1)[1].split("\n  release:\n", 1)[0]
        self.assertIn("sync_release_version.py", prepare)
        self.assertIn("needs: prepare", tests.split("    steps:", 1)[0])
        self.assertIn("sync_release_version.py", tests)
        self.assertIn("needs.prepare.outputs.version", tests)
        self.assertIn("sync_release_version.py", build)
        self.assertIn("cargo build --locked --release", build)
        self.assertNotIn("sed -i", prepare + build)

    def test_binary_version_cannot_fall_back_to_a_made_up_number(self):
        source = (ROOT / "backend/build.rs").read_text(encoding="utf8")
        self.assertIn("CARGO_PKG_VERSION", source)
        self.assertIn("assert_eq!", source)
        self.assertNotIn('"3.0.0"', source)
        self.assertIn('expect("VERSION is required', source)


if __name__ == "__main__":
    unittest.main()
