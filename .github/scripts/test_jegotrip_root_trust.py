"""Offline fake-ADB tests; these never query a real phone or execute a provider."""
import base64
import io
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import jegotrip_root_trust as exporter

UUID = "D5C0199B-1644-4D33-8912-B8BD5AB49224"
SDK = "A5C0199B-1644-4D33-8912-B8BD5AB49224"


def xml(key, value):
    return f'<map><string name="{key}">{value}</string><string name="token">PRIVATE-TOKEN</string></map>'.encode()


def make_private(folder):
    """Use only fixed local ACL operations; never an ADB or provider command."""
    if os.name != "nt":
        os.chmod(folder, 0o700)
        return
    quoted = str(folder).replace("'", "''")
    script = "$ErrorActionPreference='Stop'; $p='" + quoted + "'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $acl=[System.IO.DirectoryInfo]::new($p).GetAccessControl([System.Security.AccessControl.AccessControlSections]::Access); $acl.SetAccessRuleProtection($true,$false); foreach($rule in @($acl.GetAccessRules($true,$false,[System.Security.Principal.SecurityIdentifier]))){[void]$acl.RemoveAccessRuleSpecific($rule)}; foreach($id in @($sid,[System.Security.Principal.SecurityIdentifier]::new('S-1-5-18'))){$acl.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new($id,'FullControl','ContainerInherit,ObjectInherit','None','Allow'))}; [System.IO.DirectoryInfo]::new($p).SetAccessControl($acl)"
    encoded = base64.b64encode(script.encode("utf-16-le")).decode("ascii")
    result = subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-EncodedCommand", encoded],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20)
    if result.returncode:
        raise AssertionError("Local private fixture ACL could not be established")


class FakeAdb:
    def __init__(self, root=True, changed=False, sdk=True, sdk_changed=False, user=b"0\n", package=True):
        self.root = root
        self.changed = changed
        self.sdk = sdk
        self.sdk_changed = sdk_changed
        self.user = user
        self.package = package
        self.calls = []
        self.app_reads = 0
        self.sdk_reads = 0

    def __call__(self, adb, arguments, maximum=512 * 1024):
        self.calls.append(arguments)
        if arguments == ["devices", "-l"]:
            return b'List of devices attached\nBOARD device product:occam model:Nexus_4 transport_id:1\nOWN device product:miro model:24122RKC7C transport_id:2\n'
        if arguments[:4] != ["-s", "OWN", "shell", "-T"]:
            raise AssertionError("The unrelated board must never be probed")
        command = arguments[4]
        if command.startswith("su -c "):
            command = shlex.split(command)[2]
        if command == "am get-current-user":
            return self.user
        if command.startswith("pm path --user 0 com.cmi.jegotrip"):
            return b"package:/data/app/com.cmi.jegotrip/base.apk\n" if self.package else b""
        if command.startswith("test -d /data/user/0/com.cmi.jegotrip"):
            if "test ! -L" not in command:
                raise AssertionError("Root preferences must not follow symlinks")
            return b""
        if command == "id":
            return b"uid=0(root) gid=0(root)\n" if self.root else b"uid=2000(shell)\n"
        if command.startswith("dumpsys package com.cmi.jegotrip"):
            return b"versionCode=8940 minSdk=26\nversionName=8.9.4.0\n"
        if exporter.SDK_PREF in command:
            self.sdk_reads += 1
            return xml("uuid", UUID if self.sdk_changed and self.sdk_reads == 2 else SDK) if self.sdk else b""
        if exporter.APP_PREF in command:
            self.app_reads += 1
            return xml("phone_uuid", SDK if self.changed and self.app_reads == 2 else UUID)
        raise AssertionError("Unexpected device command")


class RootTrustExporterTests(unittest.TestCase):
    def reject(self, code, function, *arguments):
        with self.assertRaises(exporter.ExportError) as error:
            function(*arguments)
        self.assertEqual(str(error.exception), code)
        for value in [UUID, SDK, "PRIVATE-TOKEN", "OWN"]:
            self.assertNotIn(value, str(error.exception))

    def test_multiple_devices_require_explicit_selection_before_any_phone_query(self):
        fake = FakeAdb()
        self.reject("root_export_device_selection_required", exporter.collect_phone, "fake-adb", None, None, fake)
        self.assertEqual(fake.calls, [["devices", "-l"]])

    def test_model_selection_reads_only_the_owned_phone_and_keeps_sdk_identity_independent(self):
        fake = FakeAdb()
        value = exporter.collect_phone("fake-adb", model="24122RKC7C", runner=fake)
        self.assertEqual(value["app_device_uuid"], UUID)
        self.assertEqual(value["sdk_device_uuid"], SDK)
        self.assertEqual(value["app_version"], "8.9.4.0")
        self.assertEqual(value["app_version_code"], 8940)
        self.assertTrue(value["identity_stable"])
        self.assertEqual(fake.app_reads, 2)
        self.assertEqual(fake.sdk_reads, 2)
        self.assertTrue(all(call[:2] == ["-s", "OWN"] for call in fake.calls[1:]))
        commands = "\n".join(" ".join(call) for call in fake.calls)
        for forbidden in ["force-stop", "start-activity", "pm clear", "install", "adb root", "setVoip", "verificationTrustfulDevice", "user.dat", "phone_safe_num", "rm ", "mv ", "dd ", "settings put"]:
            self.assertNotIn(forbidden, commands)

    def test_root_denial_stops_before_private_file_access(self):
        fake = FakeAdb(root=False)
        self.reject("root_export_root_required", exporter.collect_phone, "fake-adb", None, "24122RKC7C", fake)
        self.assertEqual(fake.app_reads, 0)

    def test_identity_changes_during_read_are_not_silently_exported(self):
        fake = FakeAdb(changed=True)
        self.reject("root_export_identity_changed_during_read", exporter.collect_phone, "fake-adb", None, "24122RKC7C", fake)

    def test_sdk_identity_changes_during_read_are_not_exported(self):
        fake = FakeAdb(sdk_changed=True)
        self.reject("root_export_identity_changed_during_read", exporter.collect_phone, "fake-adb", None, "24122RKC7C", fake)

    def test_bad_android_user_stops_before_package_or_root_private_reads(self):
        fake = FakeAdb(user=b"0; PRIVATE-TOKEN\n")
        self.reject("root_export_android_user_invalid", exporter.collect_phone, "fake-adb", None, "24122RKC7C", fake)
        self.assertEqual(fake.app_reads, 0)
        self.assertEqual(len(fake.calls), 2)

    def test_missing_app_stops_before_root_private_reads(self):
        fake = FakeAdb(package=False)
        self.reject("root_export_package_missing", exporter.collect_phone, "fake-adb", None, "24122RKC7C", fake)
        self.assertEqual(fake.app_reads, 0)
        self.assertFalse(any("su -c" in " ".join(call) for call in fake.calls))

    def test_missing_optional_sdk_uuid_does_not_generate_or_reuse_app_uuid_as_sdk(self):
        value = exporter.collect_phone("fake-adb", model="24122RKC7C", runner=FakeAdb(sdk=False))
        self.assertIsNone(value["sdk_device_uuid"])
        self.assertEqual(value["app_device_uuid"], UUID)

    def test_unauthorized_emulator_and_network_devices_are_never_automatic_targets(self):
        devices = exporter.device_list(b'List of devices attached\nusb unauthorized\nemulator-5554 device model:virtual\n192.0.2.1:5555 device model:remote\n')
        self.reject("root_export_device_selection_required", exporter.select_device, devices, None, None)
        self.reject("root_export_selection_invalid", exporter.select_device, devices, "bad; command", None)
        self.reject("root_export_selection_invalid", exporter.select_device, devices, None, "bad model")

    def test_sdk_xml_rejects_entities_duplicate_fields_and_malformed_uuid(self):
        self.reject("root_export_sdk_preferences_invalid", exporter.sdk_uuid, b'<!DOCTYPE map><map/>')
        self.reject("root_export_sdk_preferences_invalid", exporter.sdk_uuid, b'<map><string name="uuid">not-a-uuid</string></map>')
        self.reject("root_export_sdk_preferences_invalid", exporter.sdk_uuid,
            f'<map><string name="uuid">{SDK}</string><string name="uuid">{UUID}</string></map>'.encode())

    def test_material_output_is_compatible_uuid_only_json_without_credentials_or_trust_assertion(self):
        base = ROOT / ".local" / "root-export-fixtures"
        base.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=base) as directory:
            make_private(directory)
            value = exporter.collect_phone("fake-adb", model="24122RKC7C", runner=FakeAdb())
            result = exporter.write_material(Path(directory), value)
            identity = json.loads((Path(directory) / "identity.json").read_text())
            self.assertEqual(identity, {"version": 1, "source": "android_root_export", "app_device_uuid": UUID})
            self.assertFalse(result["server_trust_verified"])
            self.assertFalse(result["provider_request_sent"])
            self.assertFalse(result["phone_or_app_write_sent"])
            self.assertFalse(result["credentials_exported"])
            self.assertTrue(result["app_and_sdk_uuid_differ"])
            stdout = json.dumps(result)
            for private in [UUID, SDK, "PRIVATE-TOKEN", "OWN"]:
                self.assertNotIn(private, stdout)
            self.reject("root_export_output_already_exists", exporter.write_material, Path(directory), value)

    def test_invalid_output_is_rejected_without_running_adb(self):
        self.reject("root_export_output_must_be_private", exporter.validate_output, ROOT / "docs")

    def test_offline_mode_reads_raw_zip_and_never_invokes_adb(self):
        base = ROOT / ".local" / "root-export-fixtures"
        base.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=base) as directory:
            source = Path(directory) / "raw.zip"
            with zipfile.ZipFile(source, "w") as archive:
                archive.writestr(exporter.APP_PREF, xml("phone_uuid", UUID))
                archive.writestr(exporter.SDK_PREF, xml("uuid", SDK))
            with patch.object(exporter, "execute", side_effect=AssertionError("No ADB in offline mode")):
                value = exporter.collect_offline(source)
            self.assertEqual(value["app_device_uuid"], UUID)
            self.assertEqual(value["sdk_device_uuid"], SDK)
            self.assertEqual(value["source"], "offline_raw_root_zip")

    def test_material_rejects_additional_private_keys_before_writing_any_file(self):
        base = ROOT / ".local" / "root-export-fixtures"
        base.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=base) as directory:
            make_private(directory)
            value = exporter.collect_phone("fake-adb", model="24122RKC7C", runner=FakeAdb())
            value["token"] = "PRIVATE-TOKEN"
            self.reject("root_export_material_invalid", exporter.write_material, Path(directory), value)
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_material_rejects_false_identity_stability_before_writing_any_file(self):
        base = ROOT / ".local" / "root-export-fixtures"
        base.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=base) as directory:
            make_private(directory)
            value = exporter.collect_phone("fake-adb", model="24122RKC7C", runner=FakeAdb())
            value["identity_stable"] = False
            self.reject("root_export_material_invalid", exporter.write_material, Path(directory), value)
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_existing_unrelated_file_in_output_is_not_overwritten_or_used_as_export_directory(self):
        base = ROOT / ".local" / "root-export-fixtures"
        base.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=base) as directory:
            original = Path(directory) / "preserve.txt"
            original.write_text("KEEP", encoding="utf-8")
            self.reject("root_export_output_already_exists", exporter.validate_output, Path(directory))
            self.assertEqual(original.read_text(encoding="utf-8"), "KEEP")

    def test_offline_mode_refuses_sdk_identity_from_another_export_root(self):
        base = ROOT / ".local" / "root-export-fixtures"
        base.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=base) as directory:
            source = Path(directory) / "mixed.zip"
            with zipfile.ZipFile(source, "w") as archive:
                archive.writestr("owner/" + exporter.APP_PREF, xml("phone_uuid", UUID))
                archive.writestr("unrelated/" + exporter.SDK_PREF, xml("uuid", SDK))
            self.reject("root_export_sdk_preferences_invalid", exporter.collect_offline, source)

    def test_duplicate_models_require_serial_and_never_probe_either_phone(self):
        devices = exporter.device_list(b"List of devices attached\nONE device model:24122RKC7C\nTWO device model:24122RKC7C\n")
        self.reject("root_export_device_selection_required", exporter.select_device, devices, None, "24122RKC7C")
        self.assertEqual(exporter.select_device(devices, "ONE", "24122RKC7C")["serial"], "ONE")

    def test_powershell_wrapper_protects_new_directory_before_running_python_and_supports_whatif(self):
        text = (ROOT / "scripts/Export-JegotripTrust.ps1").read_text(encoding="utf-8-sig")
        self.assertIn("SupportsShouldProcess = $true", text)
        self.assertIn("$PSCmdlet.ShouldProcess", text)
        self.assertIn("$acl.SetAccessRuleProtection($true, $false)", text)
        self.assertLess(text.index(".SetAccessControl($acl)"), text.index("& $python.Source @arguments"))
        self.assertIn("AccessControlSections]::Access", text)
        self.assertIn("[System.Security.AccessControl.PropagationFlags]::None", text)
        self.assertNotIn("'/T'", text)
        self.assertNotIn("'/inheritance:r'", text)
        self.assertIn("-OfflineBackup", text)
        self.assertIn("--model", text)
        self.assertIn("--serial", text)


if __name__ == "__main__":
    unittest.main()
