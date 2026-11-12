"""Static/local fixture checks; never run the APK, SDK or a provider."""
from __future__ import annotations

import importlib.util
import io
from pathlib import Path
import stat
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("jegotrip_backup_identity", ROOT / "scripts/jegotrip_backup_identity.py")
assert SPEC and SPEC.loader
reader = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(reader)
UUID = "D5C0199B-1644-4D33-8912-B8BD5AB49224"
OTHER = "A5C0199B-1644-4D33-8912-B8BD5AB49224"


def prefs(value: str = UUID) -> bytes:
    return f'<map><string name="phone_uuid">{value}</string><string name="phone_safe_num">PRIVATE-CACHE-NOT-AUTHORIZATION</string><string name="token">PRIVATE-TOKEN-NOT-EXPORTED</string></map>'.encode()


def archive(entries: list[tuple[str | zipfile.ZipInfo, bytes]]) -> bytes:
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_DEFLATED) as output:
        for name, value in entries:
            if isinstance(name, str):
                entry = zipfile.ZipInfo(name)
                # Preserve the real on-wire unsafe name even on Windows, where
                # ZipInfo normally rewrites a backslash before writing it.
                entry.filename = name
                entry.orig_filename = name
                entry.compress_type = zipfile.ZIP_DEFLATED
            else:
                entry = name
            output.writestr(entry, value)
    return stream.getvalue()


class BackupIdentityTests(unittest.TestCase):
    def rejected(self, code: str, function, *args) -> None:
        with self.assertRaises(reader.BackupError) as error:
            function(*args)
        self.assertEqual(str(error.exception), code)
        for private in [UUID, OTHER, "PRIVATE-TOKEN", "PRIVATE-CACHE", "private-password"]:
            self.assertNotIn(private, str(error.exception))

    def test_original_preferences_extract_only_app_uuid_and_preserve_case(self):
        self.assertEqual(reader.read_preferences(prefs()), UUID)
        self.assertEqual(reader.identity_export(UUID), {"version": 1, "source": "android_root_export", "app_device_uuid": UUID})
        self.assertEqual(reader.read_preferences(prefs(UUID.lower())), UUID.lower())

    def test_root_zip_matches_preferences_and_device_dat_without_extracting_files(self):
        data = archive([("data/user/0/com.cmi.jegotrip/shared_prefs/setting_power.xml", prefs()),
            ("data/user/0/com.cmi.jegotrip/files/device.dat", UUID.encode()),
            ("data/user/0/com.cmi.jegotrip/shared_prefs/sdk.xml", b'<map><string name="uuid">SDK-OTHER-ID</string></map>'),
            ("data/user/0/com.cmi.jegotrip/databases/private.db", b"private-password")])
        self.assertEqual(reader.identity_from_bytes(data, "root.zip"), (UUID, 2))

    def test_conflicting_sources_and_multiple_roots_are_not_silently_selected(self):
        for entries in [[("shared_prefs/setting_power.xml", prefs()), ("files/device.dat", OTHER.encode())],
            [("one/shared_prefs/setting_power.xml", prefs()), ("two/files/device.dat", UUID.encode())],
            [("shared_prefs/setting_power.xml", prefs()), ("files/device.dat", UUID.lower().encode())]]:
            self.rejected("jegotrip_backup_uuid_conflict", reader.identity_from_zip, archive(entries))

    def test_duplicate_or_wrong_type_uuid_field_is_rejected(self):
        for body in [f'<map><string name="phone_uuid">{UUID}</string><string name="phone_uuid">{UUID}</string></map>',
            '<map><int name="phone_uuid" value="1"/></map>',
            f'<map><string name="phone_uuid"><string>{UUID}</string></string></map>']:
            self.rejected("jegotrip_backup_uuid_conflict", reader.read_preferences, body.encode())

    def test_sdk_uuid_and_safe_number_cache_do_not_become_app_uuid(self):
        data = archive([("shared_prefs/sdk.xml", f'<map><string name="uuid">{UUID}</string></map>'.encode()),
            ("shared_prefs/setting_power.xml", b'<map><string name="phone_safe_num">0123456789abcdef</string></map>')])
        self.rejected("jegotrip_backup_uuid_missing", reader.identity_from_zip, data)
        for value in ["123456789012345", "0123456789abcdef0123456789abcdef", "private-password", "vtd-token", ""]:
            self.rejected("jegotrip_backup_uuid_invalid", reader.read_preferences, prefs(value))

    def test_sba1_data_and_v1_metadata_are_detected_without_guessing_a_key(self):
        for leaf, data in [("com.cmi.jegotrip.dat", b"SBA1\x00\x02\x00\x90opaque"),
            ("com.cmi.jegotrip.xml", b"v1:::encoded-private-metadata"),
            ("com.cmi.jegotrip.extra", b"v1:::encoded-private-metadata")]:
            self.rejected("jegotrip_backup_protected_container", reader.identity_from_zip, archive([("dated/" + leaf, data)]))
        self.rejected("jegotrip_backup_protected_container", reader.identity_from_bytes, b"v1:::opaque", "setting_power.xml")

    def test_zip_path_traversal_absolute_windows_paths_and_symlinks_are_rejected(self):
        for name in ["../shared_prefs/setting_power.xml", "/shared_prefs/setting_power.xml", "C:/files/device.dat", "files\\device.dat", "one//files/device.dat", "one/./files/device.dat", "files/device.dat\x00suffix", "files/\x1fdevice.dat"]:
            self.rejected("jegotrip_backup_archive_unsafe", reader.identity_from_zip, archive([(name, prefs())]))
        link = zipfile.ZipInfo("files/device.dat")
        link.create_system = 3
        link.external_attr = (stat.S_IFLNK | 0o777) << 16
        self.rejected("jegotrip_backup_archive_unsafe", reader.identity_from_zip, archive([(link, UUID.encode())]))

    def test_declared_entities_malformed_or_non_map_xml_are_rejected(self):
        for data in [b'<!DOCTYPE map [<!ENTITY value "private-password">]><map><string name="phone_uuid">&value;</string></map>',
            b'<map><string name="phone_uuid">', b'<root/>']:
            self.rejected("jegotrip_backup_xml_invalid", reader.read_preferences, data)

    def test_selected_member_crc_and_local_size_bounds_are_checked(self):
        self.rejected("jegotrip_backup_identity_file_too_large", reader.identity_from_zip,
            archive([("shared_prefs/setting_power.xml", b" " * (reader.MAX_IDENTITY_FILE_BYTES + 1))]))
        self.rejected("jegotrip_backup_archive_invalid", reader.identity_from_zip, b"PK\x03\x04broken")
        data = bytearray(archive([("files/device.dat", UUID.encode())]))
        # Alter only the central directory CRC, leaving the original compressed
        # bytes intact. The selected file must not be accepted after a bad CRC.
        offset = data.index(b"PK\x01\x02")
        data[offset + 16] ^= 1
        self.rejected("jegotrip_backup_archive_invalid", reader.identity_from_zip, bytes(data))
        directory_bomb = bytearray(archive([("files/device.dat", UUID.encode())]))
        end = directory_bomb.rfind(b"PK\x05\x06")
        directory_bomb[end + 8:end + 12] = (reader.MAX_MEMBERS + 1).to_bytes(2, "little") * 2
        self.rejected("jegotrip_backup_archive_too_large", reader.identity_from_zip, bytes(directory_bomb))
        lying_count = bytearray(archive([("files/device.dat", UUID.encode())]))
        end = lying_count.rfind(b"PK\x05\x06")
        lying_count[end + 8:end + 12] = b"\x00" * 4
        self.rejected("jegotrip_backup_archive_invalid", reader.identity_from_zip, bytes(lying_count))
        expanded_bomb = bytearray(archive([("files/device.dat", UUID.encode())]))
        directory = expanded_bomb.index(b"PK\x01\x02")
        expanded_bomb[directory + 24:directory + 28] = (reader.MAX_EXPANDED_BYTES + 1).to_bytes(4, "little")
        self.rejected("jegotrip_backup_archive_too_large", reader.identity_from_zip, bytes(expanded_bomb))

    def test_export_json_requires_only_original_identity_fields_not_authorization(self):
        valid = '{"version":1,"source":"android_root_export","app_device_uuid":"' + UUID + '"}'
        self.assertEqual(reader.read_identity_export(valid.encode()), UUID)
        for data in [valid[:-1] + ',"trust_granted":true}', valid.replace('"version":1', '"version":true'),
            valid.replace('"source":"android_root_export"', '"source":"phone_safe_num"'),
            valid[:-1] + ',"app_device_uuid":"' + OTHER + '"}', '[]', '{"password":"private-password"}']:
            self.rejected("jegotrip_backup_export_invalid", reader.read_identity_export, data.encode())

    def test_export_cannot_be_written_into_publishable_source_paths(self):
        self.rejected("jegotrip_backup_output_must_be_private", reader.write_private_export, ROOT / "docs/uuid.json", UUID)

    def test_ui_local_reader_keeps_existing_backend_binding_and_consumer_boundaries(self):
        component = (ROOT / "frontend/src/pages/sim/JegotripPhoneBackupInput.tsx").read_text(encoding="utf-8")
        self.assertIn("scope.active", component)
        self.assertIn("selected !== fileGeneration.current", component)
        self.assertIn("scope.active = false", component)
        self.assertIn("key={JSON.stringify([props.open, props.disabled, contextKey])}", component)
        effect = component.split("useLayoutEffect(() => {", 1)[1].split("}, [", 1)[0]
        self.assertNotIn("setBusy(", effect)
        self.assertNotIn("setError(", effect)
        for remote in ["fetch(", "api.", "localStorage", "sessionStorage", "console."]:
            self.assertNotIn(remote, component)
        login = (ROOT / "frontend/src/pages/sim/AppAccountLoginPane.tsx").read_text(encoding="utf-8")
        self.assertIn("deviceMode: 'phone_app', deviceId: value, consent: false", login)
        self.assertIn("current.intent === 'check_device_trust' ? 'check_device_trust' : 'reuse_hosting'", login)
        device = (ROOT / "frontend/src/pages/sim/JegotripDeviceIdentityPane.tsx").read_text(encoding="utf-8")
        self.assertIn("expected_revision: row?.revision ?? 0", device)
        self.assertIn("window.confirm(", device)
        self.assertIn("JSON.stringify([selected, row?.revision, accountId, mode, value, phoneImport])", device)


if __name__ == "__main__":
    unittest.main()
