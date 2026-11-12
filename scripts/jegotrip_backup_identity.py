#!/usr/bin/env python3
"""Offline identity-only reader for the owner's raw Android root export.

This is an analysis/import helper, not the production hosting engine. It never
executes an APK, restores phone data, contacts a provider, or exports account
passwords, cached authentication, session tokens or SDK credentials.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import struct
import sys
import xml.etree.ElementTree as ET
import zipfile
import zlib

MAX_ARCHIVE_BYTES = 32 * 1024 * 1024
MAX_EXPANDED_BYTES = 64 * 1024 * 1024
MAX_MEMBERS = 4096
MAX_IDENTITY_FILE_BYTES = 256 * 1024
UUID = re.compile(r"[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\Z")
PROJECT_ROOT = Path(__file__).resolve().parents[1]


class BackupError(Exception):
    """A fixed local error code; never contains an app value or source path."""


def require_uuid(value: str) -> str:
    value = value.strip()
    if not UUID.fullmatch(value):
        raise BackupError("jegotrip_backup_uuid_invalid")
    return value  # Preserve the original case; never generate a replacement.


def check_plaintext(data: bytes) -> str:
    if len(data) > MAX_IDENTITY_FILE_BYTES:
        raise BackupError("jegotrip_backup_identity_file_too_large")
    if data.startswith((b"SBA1", b"v1:::")):
        raise BackupError("jegotrip_backup_protected_container")
    try:
        return data.decode("utf-8-sig", errors="strict")
    except UnicodeError:
        raise BackupError("jegotrip_backup_format_unsupported") from None


def read_preferences(data: bytes) -> str | None:
    text = check_plaintext(data)
    # ElementTree does not retrieve external entities. Reject declarations too,
    # rather than accepting entity expansion or treating a cache as a credential.
    if re.search(r"<!\s*(?:DOCTYPE|ENTITY)", text, re.IGNORECASE):
        raise BackupError("jegotrip_backup_xml_invalid")
    try:
        root = ET.fromstring(text)
    except ET.ParseError:
        raise BackupError("jegotrip_backup_xml_invalid") from None
    if root.tag != "map":
        raise BackupError("jegotrip_backup_xml_invalid")
    fields = [element for element in root if element.attrib.get("name") == "phone_uuid"]
    if not fields:
        return None
    if len(fields) != 1 or fields[0].tag != "string" or len(fields[0]):
        raise BackupError("jegotrip_backup_uuid_conflict")
    return require_uuid(fields[0].text or "")


def read_identity_export(data: bytes) -> str:
    text = check_plaintext(data)
    def unique_object(pairs: list[tuple[str, object]]) -> dict:
        result = {}
        for key, value in pairs:
            if key in result:
                raise BackupError("jegotrip_backup_export_invalid")
            result[key] = value
        return result
    try:
        value = json.loads(text, object_pairs_hook=unique_object)
    except (ValueError, RecursionError):
        raise BackupError("jegotrip_backup_export_invalid") from None
    if not isinstance(value, dict) or set(value) != {"version", "source", "app_device_uuid"}:
        raise BackupError("jegotrip_backup_export_invalid")
    if type(value["version"]) is not int or value["version"] != 1 or value["source"] != "android_root_export" or not isinstance(value["app_device_uuid"], str):
        raise BackupError("jegotrip_backup_export_invalid")
    return require_uuid(value["app_device_uuid"])


def safe_member(entry: zipfile.ZipInfo) -> str:
    name = entry.orig_filename
    parts = name.rstrip("/").split("/")
    if name != entry.filename or not name or "\\" in name or ":" in name or name.startswith("/") or any(ord(char) < 32 for char in name) or any(part in {"", ".", ".."} for part in parts):
        raise BackupError("jegotrip_backup_archive_unsafe")
    mode = entry.external_attr >> 16
    if stat.S_ISLNK(mode) or entry.flag_bits & 1:
        raise BackupError("jegotrip_backup_archive_unsafe")
    return name.rstrip("/")


def bound_zip_directory(data: bytes) -> None:
    """Bound the actual directory before ZipFile allocates member objects."""
    end = data.rfind(b"PK\x05\x06", max(0, len(data) - 65557))
    if end < 0 or end + 22 > len(data):
        raise BackupError("jegotrip_backup_archive_invalid")
    _, disk, directory_disk, disk_count, total_count, size, offset, comment = struct.unpack_from("<4s4H2IH", data, end)
    if disk or directory_disk or disk_count != total_count:
        raise BackupError("jegotrip_backup_archive_unsafe")
    if total_count > MAX_MEMBERS:
        raise BackupError("jegotrip_backup_archive_too_large")
    # ZIP64/multi-disk/self-extracting/trailing containers are not the supported
    # bounded raw-root export. Do not guess offsets or accept an ambiguous tail.
    if end + 22 + comment != len(data) or offset + size != end or total_count and not data.startswith(b"PK\x03\x04"):
        raise BackupError("jegotrip_backup_archive_invalid")
    position = offset
    count = 0
    while position < end:
        if position + 46 > end or data[position:position + 4] != b"PK\x01\x02":
            raise BackupError("jegotrip_backup_archive_invalid")
        lengths = struct.unpack_from("<3H", data, position + 28)
        position += 46 + sum(lengths)
        count += 1
        if count > MAX_MEMBERS:
            raise BackupError("jegotrip_backup_archive_too_large")
        if position > end:
            raise BackupError("jegotrip_backup_archive_invalid")
    if position != end or count != total_count:
        raise BackupError("jegotrip_backup_archive_invalid")


def identity_from_zip(data: bytes) -> tuple[str, int]:
    if len(data) > MAX_ARCHIVE_BYTES:
        raise BackupError("jegotrip_backup_archive_too_large")
    bound_zip_directory(data)
    candidates: list[str] = []
    roots: set[str] = set()
    try:
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            entries = archive.infolist()
            if len(entries) > MAX_MEMBERS or sum(entry.file_size for entry in entries) > MAX_EXPANDED_BYTES:
                raise BackupError("jegotrip_backup_archive_too_large")
            names: set[str] = set()
            for entry in entries:
                name = safe_member(entry)
                if name in names:
                    raise BackupError("jegotrip_backup_archive_unsafe")
                names.add(name)
                if entry.is_dir():
                    continue
                leaf = PurePosixPath(name).name
                # Swift-style containers are not raw App files. Reading their
                # first bytes does not decrypt/restore them or expose metadata.
                if leaf in {"com.cmi.jegotrip.dat", "com.cmi.jegotrip.extdat", "com.cmi.jegotrip.xml", "com.cmi.jegotrip.extra"}:
                    with archive.open(entry) as stream:
                        if stream.read(5).startswith((b"SBA1", b"v1:::")):
                            raise BackupError("jegotrip_backup_protected_container")
                prefs = name == "setting_power.xml" or name.endswith("/shared_prefs/setting_power.xml") or name == "shared_prefs/setting_power.xml"
                device = name == "device.dat" or name.endswith("/files/device.dat") or name == "files/device.dat"
                if not prefs and not device:
                    continue
                if entry.file_size > MAX_IDENTITY_FILE_BYTES:
                    raise BackupError("jegotrip_backup_identity_file_too_large")
                content = archive.read(entry)  # Full selected-member CRC check.
                value = read_preferences(content) if prefs else require_uuid(check_plaintext(content))
                if value is not None:
                    candidates.append(value)
                    suffix = "shared_prefs/setting_power.xml" if prefs else "files/device.dat"
                    roots.add(name[:-len(suffix)].rstrip("/") if name.endswith(suffix) else "")
    except BackupError:
        raise
    except (zipfile.BadZipFile, RuntimeError, NotImplementedError, OSError, ValueError, EOFError, UnicodeError, zlib.error):
        raise BackupError("jegotrip_backup_archive_invalid") from None
    if not candidates:
        raise BackupError("jegotrip_backup_uuid_missing")
    if len(set(candidates)) != 1 or len(roots) > 1:
        raise BackupError("jegotrip_backup_uuid_conflict")
    return candidates[0], len(candidates)


def identity_from_bytes(data: bytes, filename: str) -> tuple[str, int]:
    if data.startswith(b"PK\x03\x04") or filename.lower().endswith(".zip"):
        return identity_from_zip(data)
    if filename == "setting_power.xml":
        value = read_preferences(data)
        if value is None:
            raise BackupError("jegotrip_backup_uuid_missing")
        return value, 1
    if filename == "device.dat":
        return require_uuid(check_plaintext(data)), 1
    if filename.lower().endswith(".json"):
        return read_identity_export(data), 1
    # Detect protected metadata even if the file name is misleading.
    check_plaintext(data)
    raise BackupError("jegotrip_backup_format_unsupported")


def identity_export(value: str) -> dict:
    return {"version": 1, "source": "android_root_export", "app_device_uuid": require_uuid(value)}


def write_private_export(path: Path, value: str) -> None:
    path = path.resolve()
    private_root = (PROJECT_ROOT / ".local").resolve()
    if not path.is_relative_to(private_root) or path.suffix.lower() != ".json":
        raise BackupError("jegotrip_backup_output_must_be_private")
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = (json.dumps(identity_export(value), ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as stream:
            stream.write(encoded)
    except OSError:
        raise BackupError("jegotrip_backup_output_not_written") from None


def main() -> int:
    parser = argparse.ArgumentParser(description="本地读取原始 root 备份中的 App UUID；不恢复、不登录、不导出密码或 token。")
    parser.add_argument("source", type=Path)
    parser.add_argument("--output", type=Path, help="可选：写入项目 .local/ 下新的 UUID-only JSON，供网页本地读取；不会覆盖现有文件")
    args = parser.parse_args()
    try:
        with args.source.open("rb") as stream:
            data = stream.read(MAX_ARCHIVE_BYTES + 1)
        if len(data) > MAX_ARCHIVE_BYTES:
            raise BackupError("jegotrip_backup_archive_too_large")
        value, count = identity_from_bytes(data, args.source.name)
        if args.output is not None:
            write_private_export(args.output, value)
        # Do not print the UUID, filename, phone number or any App values.
        print(json.dumps({"app_uuid_found": True, "matching_sources": count, "source_sha256": hashlib.sha256(data).hexdigest(), "private_export_written": args.output is not None, "server_trust_verified": False, "provider_request_sent": False}, ensure_ascii=False))
        return 0
    except (BackupError, OSError) as error:
        code = str(error) if isinstance(error, BackupError) else "jegotrip_backup_source_unreadable"
        print(json.dumps({"error": code, "provider_request_sent": False}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
