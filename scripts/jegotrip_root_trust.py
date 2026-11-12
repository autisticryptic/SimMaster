#!/usr/bin/env python3
"""Fast read-only export of the owner's existing Jegotrip device identity.

No VTD, login, business switch, restore, force-stop, adb root, installation or
credential export. The production hosting engine remains Rust.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import xml.etree.ElementTree as ET
import zipfile
import zlib

from jegotrip_backup_identity import BackupError, identity_export, identity_from_zip, read_preferences, require_uuid, safe_member, MAX_IDENTITY_FILE_BYTES

PACKAGE = "com.cmi.jegotrip"
PROJECT = Path(__file__).resolve().parents[1]
SDK_PREF = "shared_prefs/com.justalk.cloud.avatar.ZpandDevice.xml"
APP_PREF = "shared_prefs/setting_power.xml"


class ExportError(Exception):
    """Fixed local error code, never raw ADB output or private source values."""


def execute(adb: str, arguments: list[str], maximum: int = 512 * 1024) -> bytes:
    try:
        result = subprocess.run([adb, *arguments], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=45)
    except (OSError, subprocess.TimeoutExpired):
        raise ExportError("root_export_transport_failed") from None
    if result.returncode:
        raise ExportError("root_export_command_failed")
    if len(result.stdout) > maximum:
        raise ExportError("root_export_output_too_large")
    return result.stdout


def device_list(data: bytes) -> list[dict]:
    result = []
    for line in data.decode("utf-8", errors="replace").splitlines():
        parts = line.split()
        if len(parts) < 2 or parts[0] == "List":
            continue
        if not re.fullmatch(r"[A-Za-z0-9._:-]{1,128}", parts[0]):
            raise ExportError("root_export_device_list_invalid")
        result.append({"serial": parts[0], "state": parts[1],
            "properties": dict(item.split(":", 1) for item in parts[2:] if ":" in item)})
    if len(result) > 64:
        raise ExportError("root_export_device_list_invalid")
    return result


def select_device(devices: list[dict], serial: str | None, model: str | None) -> dict:
    if serial and not re.fullmatch(r"[A-Za-z0-9._:-]{1,128}", serial):
        raise ExportError("root_export_selection_invalid")
    if model and not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", model):
        raise ExportError("root_export_selection_invalid")
    matches = [item for item in devices if item["state"] == "device"]
    if serial:
        matches = [item for item in matches if item["serial"] == serial]
    else:
        # Default never selects an emulator or an existing network-connected
        # device. Multiple physical devices require an explicit model/serial.
        matches = [item for item in matches if not item["serial"].startswith("emulator-") and ":" not in item["serial"]]
    if model:
        matches = [item for item in matches if item["properties"].get("model") == model]
    if len(matches) != 1:
        raise ExportError("root_export_device_selection_required")
    return matches[0]


def sdk_uuid(data: bytes) -> str | None:
    if not data:
        return None
    if len(data) > MAX_IDENTITY_FILE_BYTES or re.search(rb"<!\s*(?:DOCTYPE|ENTITY)", data, re.I):
        raise ExportError("root_export_sdk_preferences_invalid")
    try:
        root = ET.fromstring(data)
    except ET.ParseError:
        raise ExportError("root_export_sdk_preferences_invalid") from None
    values = [child for child in root if child.attrib.get("name") == "uuid"]
    if root.tag != "map" or len(values) != 1 or values[0].tag != "string" or len(values[0]):
        raise ExportError("root_export_sdk_preferences_invalid")
    try:
        return require_uuid(values[0].text or "")
    except BackupError:
        raise ExportError("root_export_sdk_preferences_invalid") from None


def collect_phone(adb: str, serial: str | None = None, model: str | None = None, runner=execute) -> dict:
    selected = select_device(device_list(runner(adb, ["devices", "-l"])), serial, model)
    target = selected["serial"]

    def shell(command: str, root: bool = False, maximum: int = 512 * 1024) -> bytes:
        remote = "su -c " + shlex.quote(command) if root else command
        return runner(adb, ["-s", target, "shell", "-T", remote], maximum)

    current_user = shell("am get-current-user").decode("utf-8", errors="strict").strip()
    if not re.fullmatch(r"\d{1,5}", current_user):
        raise ExportError("root_export_android_user_invalid")
    user = int(current_user)
    package = shell(f"pm path --user {user} {PACKAGE}")
    if not package.startswith(b"package:"):
        raise ExportError("root_export_package_missing")
    root_id = shell("id", root=True)
    if not re.search(rb"\buid=0\(", root_id):
        raise ExportError("root_export_root_required")
    base = f"/data/user/{user}/{PACKAGE}/"
    # Never follow an App-created symlink to another privileged directory/file.
    prefs = shlex.quote(base + "shared_prefs")
    directory = shlex.quote(base.rstrip("/"))
    shell(f"test -d {directory} && test ! -L {directory} && test -d {prefs} && test ! -L {prefs}", root=True)

    def preference(relative: str, optional: bool = False) -> bytes:
        path = shlex.quote(base + relative)
        read = f"test ! -L {path} && test -f {path} && test -r {path} && head -c {MAX_IDENTITY_FILE_BYTES + 1} {path}"
        command = f"if [ -e {path} ] || [ -L {path} ]; then {read}; fi" if optional else read
        return shell(command, root=True, maximum=MAX_IDENTITY_FILE_BYTES + 1)

    first = preference(APP_PREF)
    try:
        app_uuid = read_preferences(first)
    except BackupError:
        raise ExportError("root_export_app_preferences_invalid") from None
    if app_uuid is None:
        raise ExportError("root_export_app_uuid_missing")
    sdk_first = preference(SDK_PREF, optional=True)
    independent_sdk_uuid = sdk_uuid(sdk_first)
    package_dump = shell(f"dumpsys package {PACKAGE} | head -c 524289", maximum=524289).decode("utf-8", errors="replace")
    version = re.search(r"\bversionName=([^\s]+)", package_dump)
    version_code = re.search(r"\bversionCode=(\d{1,10})(?!\d)", package_dump)
    second = preference(APP_PREF)
    sdk_second = preference(SDK_PREF, optional=True)
    if first != second or sdk_first != sdk_second:
        raise ExportError("root_export_identity_changed_during_read")
    return {"app_device_uuid": app_uuid, "sdk_device_uuid": independent_sdk_uuid,
        "app_version": version[1] if version and re.fullmatch(r"\d+(?:\.\d+){0,3}", version[1]) else None,
        "app_version_code": int(version_code[1]) if version_code else None,
        "android_user": user, "device_fingerprint": hashlib.sha256(target.encode()).hexdigest()[:12],
        "source": "connected_root_phone", "source_identity_sha256": hashlib.sha256(first).hexdigest(), "identity_stable": True}


def collect_offline(path: Path) -> dict:
    try:
        with path.open("rb") as stream:
            data = stream.read(32 * 1024 * 1024 + 1)
        app_uuid, sources = identity_from_zip(data)
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            entries = archive.infolist()  # Already bounded/validated by identity_from_zip.
            identity_roots = set()
            for entry in entries:
                name = safe_member(entry)
                for suffix in [APP_PREF, "files/device.dat"]:
                    if name == suffix or name.endswith("/" + suffix):
                        identity_roots.add(name[:-len(suffix)])
                if name in {"setting_power.xml", "device.dat"}:
                    identity_roots.add("")
            matching = [entry for entry in entries if entry.filename == SDK_PREF or entry.filename.endswith("/" + SDK_PREF)]
            if (len(matching) > 1 or len(identity_roots) != 1
                or matching and (matching[0].file_size > MAX_IDENTITY_FILE_BYTES
                    or matching[0].filename[:-len(SDK_PREF)] not in identity_roots)):
                raise ExportError("root_export_sdk_preferences_invalid")
            independent = sdk_uuid(archive.read(matching[0])) if matching else None
    except BackupError as error:
        raise ExportError(str(error)) from None
    except (OSError, zipfile.BadZipFile, RuntimeError, NotImplementedError, ValueError, EOFError, UnicodeError, zlib.error):
        raise ExportError("root_export_offline_source_invalid") from None
    return {"app_device_uuid": app_uuid, "sdk_device_uuid": independent, "app_version": None, "app_version_code": None,
        "android_user": None, "device_fingerprint": None, "source": "offline_raw_root_zip",
        "source_identity_sha256": hashlib.sha256(data).hexdigest(), "identity_stable": True, "matching_sources": sources}


def validate_output(folder: Path) -> Path:
    raw = folder.absolute()
    if any(parent.is_symlink() or getattr(parent, "is_junction", lambda: False)() for parent in [raw, *raw.parents]):
        raise ExportError("root_export_output_must_be_private")
    resolved = raw.resolve()
    if not resolved.is_relative_to((PROJECT / ".local").resolve()) or not resolved.is_dir():
        raise ExportError("root_export_output_must_be_private")
    if any(resolved.iterdir()):
        raise ExportError("root_export_output_already_exists")
    if os.name == "nt":
        # Check the private directory BEFORE any ADB access. Only fixed local
        # ACL inspection is executed; the quoted path cannot inject a command.
        quoted = str(resolved).replace("'", "''")
        command = "$ErrorActionPreference='Stop'; $p='" + quoted + "'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value; $a=Get-Acl -LiteralPath $p; $r=@($a.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier]) | Where-Object {$_.AccessControlType -eq 'Allow'}); if (-not $a.AreAccessRulesProtected -or -not ($r | Where-Object {$_.IdentityReference.Value -eq $sid}) -or @($r | Where-Object {$_.IdentityReference.Value -notin @($sid,'S-1-5-18')}).Count -ne 0) {exit 2}; Write-Output PRIVATE"
        encoded = base64.b64encode(command.encode("utf-16-le")).decode("ascii")
        try:
            check = subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-EncodedCommand", encoded],
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20)
        except (OSError, subprocess.TimeoutExpired):
            raise ExportError("root_export_private_acl_required") from None
        if check.returncode or check.stdout.strip() != b"PRIVATE":
            raise ExportError("root_export_private_acl_required")
    else:
        os.chmod(resolved, 0o700)
    return resolved


def write_material(folder: Path, material: dict) -> dict:
    folder = validate_output(folder)  # Recheck reparse points/ACL after collection, before writing.
    allowed = {"app_device_uuid", "sdk_device_uuid", "app_version", "app_version_code", "android_user",
        "device_fingerprint", "source", "source_identity_sha256", "identity_stable", "matching_sources"}
    if (not set(material).issubset(allowed) or material.get("source") not in {"connected_root_phone", "offline_raw_root_zip"}
        or material.get("identity_stable") is not True):
        raise ExportError("root_export_material_invalid")
    try:
        require_uuid(material["app_device_uuid"])
        if material.get("sdk_device_uuid") is not None:
            require_uuid(material["sdk_device_uuid"])
    except (BackupError, KeyError, AttributeError, TypeError):
        raise ExportError("root_export_material_invalid") from None
    export = identity_export(material["app_device_uuid"])
    # The UI consumes exactly this minimal schema, never a cached trust bool,
    # username/password/token or the SDK UUID from the separate private context.
    for name, data in [("identity.json", export), ("sdk-context.private.json", material)]:
        try:
            fd = os.open(folder / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "w", encoding="utf-8") as output:
                json.dump(data, output, ensure_ascii=False, indent=2)
                output.write("\n")
        except OSError:
            raise ExportError("root_export_output_not_written") from None
    result = {"export_written": True, "identity_file": "identity.json", "sdk_context_present": material["sdk_device_uuid"] is not None,
        "app_and_sdk_uuid_differ": None if material["sdk_device_uuid"] is None else material["sdk_device_uuid"] != material["app_device_uuid"],
        "app_version": material["app_version"], "identity_stable": material["identity_stable"], "server_trust_verified": False,
        "provider_request_sent": False, "phone_or_app_write_sent": False, "credentials_exported": False}
    with (folder / "result.redacted.json").open("x", encoding="utf-8") as output:
        json.dump(result, output, indent=2)
        output.write("\n")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description="快速只读提取本人 root 手机已存在的 App UUID/独立SDK标识，不申请或导入授信。请使用配套PowerShell入口保护输出目录。")
    parser.add_argument("--adb")
    parser.add_argument("--serial")
    parser.add_argument("--model")
    parser.add_argument("--offline-backup", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        output = validate_output(args.output)
        if args.offline_backup:
            if args.serial or args.model or args.adb:
                raise ExportError("root_export_selection_invalid")
            material = collect_offline(args.offline_backup)
        else:
            adb = args.adb or shutil.which("adb")
            if not adb or not Path(adb).is_file():
                raise ExportError("root_export_adb_missing")
            material = collect_phone(adb, args.serial, args.model)
        print(json.dumps(write_material(output, material), ensure_ascii=False, indent=2))
        return 0
    except (ExportError, UnicodeError) as error:
        code = str(error) if isinstance(error, ExportError) else "root_export_response_invalid"
        print(json.dumps({"error": code, "provider_request_sent": False, "phone_or_app_write_sent": False}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
