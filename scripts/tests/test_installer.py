"""Hardware-free behavioral tests: no real service manager, network or installer root.

The installer API receives a temporary root and a fake systemctl runner. Bootstrap
subprocess tests put fake curl/id/uname in PATH and execute only a marker installer.
"""
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


installer = load("installer_test_module", ROOT / "deploy/installer.py")
packer = load("packer_test_module", ROOT / "scripts/package-release.py")


def elf_fixture(machine=62):
    """Structurally complete ELF64 header+PT_LOAD; never executed by tests."""
    value = bytearray(120)
    value[:7] = b"\x7fELF\x02\x01\x01"
    struct.pack_into("<HHI", value, 16, 2, machine, 1)
    struct.pack_into("<Q", value, 32, 64)
    struct.pack_into("<HHH", value, 52, 64, 56, 1)
    struct.pack_into("<IIQQQQQQ", value, 64, 1, 5, 0, 0, 0, 120, 120, 4096)
    return bytes(value)


class FakeCommands:
    def __init__(self, active="inactive", enabled="disabled", failure=None):
        self.active = active
        self.enabled = enabled
        self.failure = failure
        self.calls = []
        self.failed = False

    def __call__(self, args):
        self.calls.append(args)
        if args[0] != "systemctl":
            # No fixture ELF or production binary is executed. Read-only CLI
            # behavior is injected separately from non-executing ELF validation.
            if args[1:] == ["--version"]:
                return "simadmin 1.2.3"
            if len(args) >= 4 and args[1:3] == ["modem-backend-mode", "--config"]:
                path = Path(args[3])
                lines = path.read_text().splitlines() if path.is_file() else ["modemmanager"]
                value = lines[0] if lines else ""
                if value not in ("native", "modemmanager") or ("--require-mm" in args and value != "modemmanager"):
                    raise subprocess.CalledProcessError(21, args, stderr="invalid config")
                return value
            raise AssertionError("unexpected binary command: " + repr(args))
        action = args[1]
        if action == self.failure and not self.failed:
            self.failed = True
            raise RuntimeError("injected " + action + " failure")
        if action == "show":
            return self.active if "--property=ActiveState" in args else self.enabled
        if action == "stop":
            self.active = "inactive"
        elif action == "start":
            self.active = "active"
        elif action == "enable":
            self.enabled = "enabled"
        elif action == "disable":
            self.enabled = "disabled"
        elif action != "daemon-reload":
            raise AssertionError("unexpected service command: " + repr(args))
        return ""

    @property
    def mutations(self):
        return [c for c in self.calls if c[0] == "systemctl" and c[1] != "show"]


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.root = self.base / "root"
        self.prefix = self.root / "opt/simadmin"
        self.prefix.mkdir(parents=True)
        self.unit = self.root / "etc/systemd/system/simadmin.service"
        self.unit.parent.mkdir(parents=True)
        self.binary = self.base / "binary"
        self.binary.write_bytes(elf_fixture())
        self.binary.chmod(0o755)
        self.frontend = self.base / "frontend"
        self.frontend.mkdir()
        (self.frontend / "index.html").write_text("new frontend")
        (self.frontend / ".hidden").write_text("hidden asset")
        self.archive = self.base / "simadmin-linux-amd64.tar.gz"
        with contextlib.redirect_stdout(io.StringIO()):
            packer.pack(self.binary, self.frontend, "x86_64-unknown-linux-musl", "1.2.3", "a" * 40, self.archive)
        self.package = self.base / "package"
        self.package.mkdir()
        # Generated fixture is trusted; not an installer operation.
        with tarfile.open(self.archive) as archive:
            archive.extractall(self.package, **({"filter": "data"} if hasattr(tarfile, "data_filter") else {}))
        (self.prefix / "simadmin").write_text("old binary")
        (self.prefix / "www").mkdir()
        (self.prefix / "www/index.html").write_text("old frontend")
        shutil.copy2(ROOT / "scripts/simadmin.service", self.unit)
        self.unit.write_text(self.unit.read_text() + "\n# previous revision\n")
        for name, value in {"config.yaml": "native\n", "data.db": "database",
                            "data.db-wal": "wal", "carrier-bundles.sqlite3": "catalog",
                            "lpac/lpac": "private lpac", "e911/secret": "secret"}.items():
            path = self.prefix / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(value)
        self.preserved = {str(p.relative_to(self.root)): (p.read_bytes(), p.stat().st_mode)
                          for p in self.prefix.rglob("*") if p.is_file() and p.name not in ("simadmin", "index.html")}

    def invoke(self, fake=None, **kwargs):
        fake = fake or FakeCommands()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            installer.install(self.package, self.root, run=fake, machine="x86_64", sleep=lambda _: None, **kwargs)
        return fake

    def remanifest(self):
        files = installer.regular_tree(self.package)
        (self.package / "SHA256SUMS").write_text("".join(installer.digest(p) + "  " + n + "\n"
            for n, p in sorted(files.items()) if n != "SHA256SUMS"))

    def assert_preserved(self):
        for name, (value, mode) in self.preserved.items():
            self.assertEqual((self.root / name).read_bytes(), value, name)
            self.assertEqual((self.root / name).stat().st_mode, mode, name)

    def assert_old(self):
        self.assertEqual((self.prefix / "simadmin").read_text(), "old binary")
        self.assertEqual((self.prefix / "www/index.html").read_text(), "old frontend")
        self.assertIn("previous revision", self.unit.read_text())
        self.assert_preserved()

    def test_shared_package_contract_both_architectures_and_dotfiles(self):
        meta = installer.verify(self.package, "x86_64")
        self.assertEqual(meta["commit"], "a" * 40)
        self.assertEqual((self.package / "www/.hidden").read_text(), "hidden asset")
        self.assertEqual((self.package / "system/simadmin.service").read_bytes(), (ROOT / "scripts/simadmin.service").read_bytes())
        self.assertTrue((self.package / "devices/qcm410/system/simadmin-secondary-qmi.service").is_file())
        self.assertEqual(self.archive.with_name(self.archive.name + ".sha256").read_text().split()[0], installer.digest(self.archive))
        arm = self.base / "arm.tar.gz"
        arm_binary = self.base / "arm-binary"
        arm_binary.write_bytes(elf_fixture(183))
        with contextlib.redirect_stdout(io.StringIO()):
            packer.pack(arm_binary, self.frontend, "aarch64-unknown-linux-musl", "1.2.3", "b" * 40, arm)
        unpack = self.base / "arm"
        with tarfile.open(arm) as archive:
            archive.extractall(unpack, **({"filter": "data"} if hasattr(tarfile, "data_filter") else {}))
        installer.verify(unpack, "aarch64")

    def test_wrong_or_truncated_elf_is_rejected_before_running_any_command(self):
        cases = [b"#!/bin/sh\nexit 0\n", elf_fixture()[:63], elf_fixture()[:119],
                 elf_fixture(183), elf_fixture()[:4] + b"\x01" + elf_fixture()[5:]]
        for contents in cases:
            with self.subTest(contents=contents[:20]):
                (self.package / "simadmin").write_bytes(contents)
                metadata = json.loads((self.package / "meta.json").read_text())
                metadata["binary_md5"] = installer.digest(self.package / "simadmin", "md5")
                (self.package / "meta.json").write_text(json.dumps(metadata))
                self.remanifest()
                fake = FakeCommands(active="active")
                with self.assertRaisesRegex(RuntimeError, "ELF"):
                    self.invoke(fake, activate=True)
                self.assertEqual(fake.calls, [])
                self.assert_old()

    def test_packer_rejects_a_binary_labelled_as_the_other_architecture(self):
        output = self.base / "wrong-arm.tar.gz"
        with self.assertRaisesRegex(RuntimeError, "ELF architecture mismatch"):
            packer.pack(self.binary, self.frontend, "aarch64-unknown-linux-musl", "1.2.3", "a" * 40, output)
        self.assertFalse(output.exists())

    def test_existing_safety_environment_is_never_overwritten(self):
        original = self.unit.read_text()
        for name in ("SIMADMIN_VOWIFI_DEVICE_CHANGES_ALLOWED", "SIMADMIN_VOWIFI_LIVE_NETWORK_ALLOWED"):
            with self.subTest(name=name):
                custom = original.replace(name + "=1", name + "=0")
                self.assertNotEqual(custom, original)
                self.unit.write_text(custom)
                fake = FakeCommands(active="active")
                with self.assertRaisesRegex(RuntimeError, "custom service unit settings"):
                    self.invoke(fake, activate=True)
                self.assertEqual(self.unit.read_text(), custom)
                self.assertEqual(fake.calls, [])
                self.assert_old()
        self.unit.write_text(original)

    def test_vendor_dropin_and_main_unit_are_checked_on_non_usrmerged_systems(self):
        vendor = self.root / "lib/systemd/system"
        dropin = vendor / "simadmin.service.d"
        dropin.mkdir(parents=True)
        (dropin / "10-config.conf").write_text("[Service]\nEnvironment=SIMADMIN_CONFIG=/data/other.yaml\n")
        fake = FakeCommands(active="active")
        with self.assertRaisesRegex(RuntimeError, "drop-ins"):
            self.invoke(fake, activate=True)
        self.assertEqual(fake.calls, [])
        self.assert_old()
        shutil.rmtree(dropin)
        self.unit.unlink()
        custom = (ROOT / "scripts/simadmin.service").read_text().replace(
            "SIMADMIN_VOWIFI_DEVICE_CHANGES_ALLOWED=1", "SIMADMIN_VOWIFI_DEVICE_CHANGES_ALLOWED=0")
        (vendor / "simadmin.service").write_text(custom)
        with self.assertRaisesRegex(RuntimeError, "custom service unit settings"):
            self.invoke(fake, activate=True)
        self.assertEqual(fake.calls, [])
        self.assertFalse(self.unit.exists())
        self.assertEqual((vendor / "simadmin.service").read_text(), custom)

    def test_files_only_preserves_data_and_never_activates_services(self):
        fake = self.invoke()
        self.assertEqual(fake.mutations, [["systemctl", "daemon-reload"]])
        self.assertEqual((self.prefix / "simadmin").read_bytes(), self.binary.read_bytes())
        self.assertEqual(self.unit.read_bytes(), (ROOT / "scripts/simadmin.service").read_bytes())
        self.assertEqual((self.prefix / "www/.hidden").read_text(), "hidden asset")
        self.assert_preserved()
        self.assertFalse(list(self.prefix.glob(".install-*")))

    def test_check_only_does_not_query_or_mutate_services(self):
        fake = self.invoke(check_only=True)
        self.assertFalse(any(c[0] == "systemctl" for c in fake.calls))
        self.assert_old()

    def test_running_service_requires_explicit_maintenance(self):
        fake = FakeCommands(active="active")
        with self.assertRaisesRegex(RuntimeError, "service is running"):
            self.invoke(fake)
        self.assertEqual(fake.mutations, [])
        self.assert_old()

    def test_activate_validates_before_stop_and_only_touches_main_service(self):
        fake = FakeCommands(active="active", enabled="enabled")
        self.invoke(fake, activate=True)
        stop = fake.calls.index(["systemctl", "stop", "simadmin.service"])
        config = next(i for i, c in enumerate(fake.calls) if "modem-backend-mode" in c)
        self.assertLess(config, stop)
        self.assertTrue(all("ModemManager.service" not in c and "--activate" not in c for c in fake.calls))
        self.assertEqual(fake.active, "active")
        self.assert_preserved()

    def test_invalid_config_is_rejected_before_service_changes(self):
        (self.prefix / "config.yaml").write_text("invalid")
        fake = FakeCommands(active="active")
        with self.assertRaises(subprocess.CalledProcessError):
            self.invoke(fake, activate=True)
        self.assertEqual(fake.mutations, [])
        self.assertEqual((self.prefix / "simadmin").read_text(), "old binary")

    def test_data_config_path_wins_over_executable_fallback(self):
        (self.root / "data").mkdir()
        (self.root / "data/config.yaml").write_text("modemmanager")
        fake = self.invoke(check_only=True)
        config_calls = [c for c in fake.calls if "modem-backend-mode" in c]
        self.assertEqual(config_calls[0][-1], str(self.root / "data/config.yaml"))
        self.assertIn("--require-mm", config_calls[1])
        self.assert_old()

    def test_binary_runtime_version_must_match_metadata(self):
        fake = FakeCommands()
        def wrong_version(args):
            return "simadmin 0.0.1" if "--version" in args else fake(args)
        with self.assertRaisesRegex(RuntimeError, "binary version differs"):
            self.invoke(wrong_version)
        self.assertEqual(fake.mutations, [])
        self.assert_old()

    def test_corrupt_missing_extra_and_unsupported_packages_fail_before_commands(self):
        mutations = [
            lambda: (self.package / "www/index.html").write_text("tampered"),
            lambda: (self.package / "system/simadmin.service").unlink(),
            lambda: (self.package / "data.db").write_text("do not replace"),
            lambda: (self.package / "meta.json").write_text("{}"),
        ]
        original = self.base / "original"
        shutil.copytree(self.package, original)
        for mutate in mutations:
            with self.subTest(mutation=mutate):
                shutil.rmtree(self.package)
                shutil.copytree(original, self.package)
                mutate()
                fake = FakeCommands(active="active")
                with self.assertRaises(RuntimeError):
                    self.invoke(fake, activate=True)
                self.assertEqual(fake.calls, [])
                self.assert_old()
        with self.assertRaisesRegex(RuntimeError, "unsupported architecture"):
            installer.verify(original, "mips")
        with self.assertRaisesRegex(RuntimeError, "architecture mismatch"):
            installer.verify(original, "aarch64")

    def test_metadata_hash_mismatch_even_with_updated_manifest(self):
        meta = json.loads((self.package / "meta.json").read_text())
        meta["frontend_md5"] = "0" * 32
        (self.package / "meta.json").write_text(json.dumps(meta))
        self.remanifest()
        with self.assertRaisesRegex(RuntimeError, "frontend metadata"):
            self.invoke()
        self.assert_old()

    def test_reject_symlinks_and_custom_dropins_without_touching_files(self):
        (self.package / "www/link").symlink_to(self.prefix / "config.yaml")
        with self.assertRaisesRegex(RuntimeError, "link or special"):
            self.invoke()
        (self.package / "www/link").unlink()
        dropin = self.unit.parent / "simadmin.service.d"
        dropin.mkdir()
        (dropin / "custom.conf").write_text("[Service]\nEnvironment=SIMADMIN_CONFIG=/other.yaml\n")
        with self.assertRaisesRegex(RuntimeError, "drop-ins"):
            self.invoke()
        self.assert_old()

    def test_destination_symlink_rejected(self):
        (self.prefix / "meta.json").symlink_to(self.prefix / "config.yaml")
        with self.assertRaisesRegex(RuntimeError, "symlink"):
            self.invoke()
        self.assert_old()

    def test_start_reload_and_enable_failure_roll_back_and_restore_service_state(self):
        for failure in ("start", "daemon-reload", "enable"):
            with self.subTest(failure=failure):
                fake = FakeCommands(active="active", enabled="disabled", failure=failure)
                with self.assertRaisesRegex(RuntimeError, "injected"):
                    self.invoke(fake, activate=True)
                self.assert_old()
                self.assertEqual(fake.active, "active")
                self.assertEqual(fake.enabled, "disabled")

    def test_file_replacement_failure_rolls_back_partially_applied_files(self):
        replace = os.replace
        calls = []
        def failing_replace(source, target):
            calls.append((source, target))
            if len(calls) == 4:
                raise OSError("injected rename failure")
            replace(source, target)
        fake = FakeCommands(active="active", enabled="enabled")
        with mock.patch.object(installer.os, "replace", side_effect=failing_replace):
            with self.assertRaisesRegex(OSError, "rename failure"):
                self.invoke(fake, activate=True)
        self.assert_old()
        self.assertEqual(fake.active, "active")

    def test_preflight_staging_copy_failure_never_stops_service(self):
        fake = FakeCommands(active="active")
        with mock.patch.object(installer.shutil, "copy2", side_effect=OSError("disk full")):
            with self.assertRaises(OSError):
                self.invoke(fake, activate=True)
        self.assertEqual(fake.mutations, [])
        self.assert_old()

    def test_fresh_install_failure_removes_new_owned_files_not_user_data(self):
        (self.prefix / "simadmin").unlink()
        shutil.rmtree(self.prefix / "www")
        self.unit.unlink()
        fake = FakeCommands(failure="start")
        with self.assertRaises(RuntimeError):
            self.invoke(fake, activate=True)
        self.assertFalse((self.prefix / "simadmin").exists())
        self.assertFalse(self.unit.exists())
        self.assert_preserved()

    def test_manifest_requires_complete_unique_coverage(self):
        manifest = self.package / "SHA256SUMS"
        original = manifest.read_text()
        for content in (original + original.splitlines()[0] + "\n", "\n".join(original.splitlines()[1:]) + "\n"):
            with self.subTest(content=content[:50]):
                manifest.write_text(content)
                fake = FakeCommands()
                with self.assertRaisesRegex(RuntimeError, "manifest|cover every"):
                    self.invoke(fake)
                self.assertEqual(fake.calls, [])
                self.assert_old()

    def test_failed_health_check_rolls_back(self):
        fake = FakeCommands(active="active", enabled="enabled")
        started = False
        def fail_health(args):
            nonlocal started
            result = fake(args)
            if args[:2] == ["systemctl", "start"] and not started:
                started = True
                fake.active = "failed"
            return result
        with self.assertRaisesRegex(RuntimeError, "remain active"):
            self.invoke(fail_health, activate=True)
        self.assert_old()
        self.assertEqual(fake.active, "active")

    def test_rollback_failure_keeps_backups_and_blocks_retry(self):
        fake = FakeCommands(active="active", enabled="enabled", failure="start")
        def fail_restore(args):
            if fake.failed and args[:2] == ["systemctl", "stop"]:
                raise RuntimeError("cannot stop failed candidate")
            return fake(args)
        with self.assertRaises(RuntimeError):
            self.invoke(fail_restore, activate=True)
        backups = list(self.prefix.glob(".install-*"))
        self.assertEqual(len(backups), 1)
        self.assertEqual((backups[0] / "simadmin.old").read_text(), "old binary")
        self.assert_preserved()
        with self.assertRaisesRegex(RuntimeError, "unfinished installer backup"):
            self.invoke()

    def test_failed_original_rename_preserves_original(self):
        replace = os.replace
        count = 0
        def fail_first(source, target):
            nonlocal count
            count += 1
            if count == 1:
                raise OSError("original rename failure")
            replace(source, target)
        with mock.patch.object(installer.os, "replace", side_effect=fail_first):
            with self.assertRaises(OSError):
                self.invoke(FakeCommands(active="active"), activate=True)
        self.assert_old()

    def test_fresh_install_writes_main_unit_without_activation(self):
        shutil.rmtree(self.prefix)
        self.unit.unlink()
        fake = self.invoke()
        self.assertTrue((self.prefix / "simadmin").is_file())
        self.assertTrue(self.unit.is_file())
        self.assertEqual(fake.mutations, [["systemctl", "daemon-reload"]])
        self.assertFalse((self.prefix / "config.yaml").exists())

    def test_cli_rejects_custom_prefix_and_config_before_root_lock(self):
        for name, value in (("INSTALL_DIR", "/other"), ("PREFIX", "/other"),
                            ("SERVICE_NAME", "other"), ("SIMADMIN_CONFIG", "/other.yaml")):
            with self.subTest(variable=name), mock.patch.dict(os.environ, {name: value}), \
                    mock.patch.object(installer.sys, "argv", ["installer.py", "--check"]), \
                    mock.patch.object(installer.os, "geteuid", side_effect=AssertionError("must reject before root/lock")):
                with self.assertRaisesRegex(RuntimeError, "unsupported"):
                    installer.main()

    def test_pack_wrapper_uses_requested_target_from_any_cwd(self):
        output = self.base / "wrapper-result.tar.gz"
        subprocess.run(["sh", str(ROOT / "scripts/pack-ota.sh"), "--target", "x86_64-unknown-linux-musl",
                        "--binary", str(self.binary), "--frontend", str(self.frontend), "--version", "1.2.3",
                        "--output", str(output)], cwd=self.base, text=True, capture_output=True, check=True)
        with tarfile.open(output) as archive:
            meta = json.load(archive.extractfile("meta.json"))
            self.assertEqual(meta["arch"], "x86_64-unknown-linux-musl")
            self.assertIn("system/simadmin.service", archive.getnames())

    def test_offline_wrapper_resolves_its_own_directory(self):
        # A marker Python helper replaces the installer, so no production CLI runs.
        wrapper = self.base / "wrapper"
        wrapper.mkdir()
        shutil.copy2(ROOT / "deploy/install.sh", wrapper / "install.sh")
        (wrapper / "installer.py").write_text("import pathlib, sys\nprint(pathlib.Path(__file__).parent.name, sys.argv[1])\n")
        result = subprocess.run(["sh", str(wrapper / "install.sh"), "--check"], cwd=self.base,
                                text=True, capture_output=True, check=True)
        self.assertEqual(result.stdout.strip(), "wrapper --check")


class BootstrapTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.bin = self.base / "bin"
        self.bin.mkdir()
        self.log = self.base / "curl.log"
        self.marker = self.base / "marker"
        self.asset = self.base / "simadmin-linux-amd64.tar.gz"
        self.env = {**os.environ, "PATH": str(self.bin) + ":" + os.environ["PATH"],
                    "REPO": "example/SimAdmin", "VERSION": "1.2.3", "MOCK_ROOT": str(self.base)}
        for name, body in {
            "uname": 'case "$1" in -s) echo Linux;; -m) echo "${MOCK_ARCH:-x86_64}";; esac',
            "id": 'echo 0',
            "curl": '''printf '%s\\n' "$*" >> "$MOCK_ROOT/curl.log"
while [ "$#" -gt 0 ]; do
 case "$1" in https://*) url=$1;; -o) shift; destination=$1;; esac
 shift
done
case "$url" in
 */simadmin-linux-amd64.tar.gz) cp "$MOCK_ROOT/simadmin-linux-amd64.tar.gz" "$destination";;
 */SHA256SUMS.txt) cp "$MOCK_ROOT/SHA256SUMS.txt" "$destination";;
 *) exit 90;;
esac''',
        }.items():
            path = self.bin / name
            path.write_text("#!/bin/sh\nset -eu\n" + body + "\n")
            path.chmod(0o755)
        self.make_archive()

    def make_archive(self, version="1.2.3", unsafe=None, link=False):
        with tarfile.open(self.asset, "w:gz") as archive:
            files = {"meta.json": json.dumps({"version": version, "arch": "x86_64-unknown-linux-musl", "installer_format": 1}),
                     "install.sh": '#!/bin/sh\nprintf "%s" "$*" > "$MOCK_ROOT/marker"\n', "installer.py": "# marker only\n"}
            for name, text in files.items():
                entry = tarfile.TarInfo(name)
                entry.size = len(text.encode())
                archive.addfile(entry, io.BytesIO(text.encode()))
            if unsafe:
                entry = tarfile.TarInfo(unsafe)
                if link:
                    entry.type = tarfile.SYMTYPE
                    entry.linkname = "../../escaped"
                archive.addfile(entry, io.BytesIO())
        (self.base / "SHA256SUMS.txt").write_text(installer.digest(self.asset) + "  " + self.asset.name + "\n")

    def invoke(self, *args):
        return subprocess.run(["sh", str(ROOT / "install_latest.sh"), *args], cwd=self.base,
                              env=self.env, text=True, capture_output=True)

    def test_pinned_urls_and_offline_handoff(self):
        result = self.invoke("--check")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.marker.read_text(), "--check")
        calls = self.log.read_text().splitlines()
        self.assertEqual(len(calls), 2)
        self.assertTrue(all("https://github.com/example/SimAdmin/releases/download/v1.2.3/" in c for c in calls))
        self.assertTrue(all("raw.githubusercontent" not in c and "/latest/" not in c for c in calls))

    def test_unsupported_arch_prefix_and_missing_pin_fail_before_download(self):
        for updates in ({"MOCK_ARCH": "mips"}, {"INSTALL_DIR": "/tmp/other"}, {"VERSION": "latest"}, {"REPO": ""}):
            with self.subTest(updates=updates):
                old = self.env.copy()
                self.env.update(updates)
                result = self.invoke()
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(self.log.exists())
                self.assertFalse(self.marker.exists())
                self.env = old

    def test_bad_checksum_never_executes_payload(self):
        (self.base / "SHA256SUMS.txt").write_text("0" * 64 + "  " + self.asset.name + "\n")
        result = self.invoke()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum", result.stderr)
        self.assertFalse(self.marker.exists())

    def test_pinned_metadata_mismatch_never_executes_payload(self):
        self.make_archive(version="9.9.9")
        result = self.invoke()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("metadata mismatch", result.stderr)
        self.assertFalse(self.marker.exists())

    def test_archive_links_duplicates_and_absolute_paths_are_rejected(self):
        for name, link in (("www/link", True), ("meta.json", False), ("/tmp/escaped", False)):
            with self.subTest(name=name):
                self.make_archive(unsafe=name, link=link)
                result = self.invoke()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("unsafe", result.stderr)
                self.assertFalse(self.marker.exists())

    def test_download_failure_never_executes_payload(self):
        (self.bin / "curl").write_text("#!/bin/sh\nexit 22\n")
        result = self.invoke()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.marker.exists())

    def test_archive_traversal_is_rejected_before_execution(self):
        self.make_archive(unsafe="../escaped")
        result = self.invoke()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unsafe", result.stderr)
        self.assertFalse(self.marker.exists())
        self.assertFalse((self.base / "escaped").exists())


if __name__ == "__main__":
    unittest.main()
