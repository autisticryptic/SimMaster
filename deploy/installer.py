#!/usr/bin/env python3
"""Verified, files-only by default installer. No package manager or hardware actions.

Only the CLI uses real systemctl/root. Tests inject a temporary root and runner.
The transaction owns a small explicit set of paths, never config or databases.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import time

ARCHES = {"aarch64": "aarch64-unknown-linux-musl", "arm64": "aarch64-unknown-linux-musl",
          "x86_64": "x86_64-unknown-linux-musl", "amd64": "x86_64-unknown-linux-musl"}
PAYLOAD = ("simadmin", "www", "meta.json", "devices")
TOP_LEVEL = set(PAYLOAD) | {"install.sh", "installer.py", "system", "SHA256SUMS"}
SERVICE = "simadmin.service"


def fail(message):
    raise RuntimeError(message)


def digest(path, algorithm="sha256"):
    h = hashlib.new(algorithm)
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def regular_tree(package):
    files = {}
    for base, dirs, names in os.walk(package, followlinks=False):
        for name in dirs + names:
            path = Path(base) / name
            rel = path.relative_to(package).as_posix()
            if path.is_symlink() or not (path.is_dir() or path.is_file()):
                fail("package contains a link or special file: " + rel)
            if rel.split("/")[0] not in TOP_LEVEL or "\n" in rel or "\\" in rel:
                fail("unexpected package path: " + rel)
            if path.is_file():
                files[rel] = path
    return files


def verify_elf(binary, expected_arch):
    """Reject mislabelled/truncated executables without running target code."""
    with binary.open("rb") as stream:
        header = stream.read(64)
    if len(header) != 64 or header[:4] != b"\x7fELF":
        fail("invalid or truncated ELF executable header")
    if header[4:7] != b"\x02\x01\x01":
        fail("ELF must be 64-bit little-endian version 1")
    file_type, machine, version = struct.unpack_from("<HHI", header, 16)
    header_size, entry_size, entry_count = struct.unpack_from("<HHH", header, 52)
    program_offset = struct.unpack_from("<Q", header, 32)[0]
    if file_type not in (2, 3) or version != 1 or header_size != 64:
        fail("invalid ELF executable type/version/header size")
    expected_machine = {"x86_64-unknown-linux-musl": 62, "aarch64-unknown-linux-musl": 183}[expected_arch]
    if machine != expected_machine:
        fail("ELF architecture mismatch: expected {} (e_machine={}), found {}".format(
            expected_arch, expected_machine, machine))
    if (entry_size != 56 or not 0 < entry_count < 65535 or program_offset < 64 or
            program_offset + entry_size * entry_count > binary.stat().st_size):
        fail("invalid or truncated ELF program headers")


def verify(package, machine):
    expected_arch = ARCHES.get(machine)
    if not expected_arch:
        fail("unsupported architecture: " + machine)
    files = regular_tree(package)
    required = {"simadmin", "www/index.html", "meta.json", "install.sh", "installer.py",
                "system/simadmin.service", "SHA256SUMS"}
    if not required <= files.keys() or not (package / "devices").is_dir():
        fail("incomplete installer package (old OTA archives are not supported)")
    if set(p.name for p in (package / "system").iterdir()) != {SERVICE}:
        fail("unexpected main service resources")
    manifest = {}
    for line in files["SHA256SUMS"].read_text().splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        if not match:
            fail("invalid package SHA256SUMS")
        checksum, name = match.groups()
        if name in manifest or name not in files or name == "SHA256SUMS":
            fail("invalid/duplicate manifest path: " + name)
        manifest[name] = checksum
    if set(manifest) != files.keys() - {"SHA256SUMS"}:
        fail("SHA256SUMS must cover every package file")
    for name, checksum in manifest.items():
        if digest(files[name]) != checksum:
            fail("checksum mismatch: " + name)
    meta = json.loads(files["meta.json"].read_text())
    if meta.get("installer_format") != 1 or meta.get("arch") != expected_arch:
        fail("unsupported metadata format or architecture mismatch")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.+-]+)?", meta.get("version", "")):
        fail("invalid metadata version")
    if not re.fullmatch(r"[0-9a-f]{7,40}", meta.get("commit", "")):
        fail("metadata must identify the source commit")
    verify_elf(files["simadmin"], expected_arch)
    if digest(files["simadmin"], "md5") != meta.get("binary_md5"):
        fail("binary metadata checksum mismatch")
    frontend_hashes = sorted(digest(path, "md5") for name, path in files.items() if name.startswith("www/"))
    frontend_md5 = hashlib.md5("".join(h + "\n" for h in frontend_hashes).encode()).hexdigest()
    if frontend_md5 != meta.get("frontend_md5"):
        fail("frontend metadata checksum mismatch")
    return meta


def command(args, **kwargs):
    return subprocess.run(args, check=True, text=True, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, timeout=60, **kwargs).stdout.strip()


def service_state(run, property_name):
    return run(["systemctl", "show", SERVICE, "--property=" + property_name, "--value"])


def safe_destination(root, relative):
    path = root
    for part in PurePosixPath(relative).parts:
        path = path / part
        if path.is_symlink():
            fail("refusing symlink in install destination: " + str(path))
    return path


def check_unit(path, expected=None):
    text = path.read_text()
    if expected is not None:
        def directives(value):
            return [line.strip() for line in value.splitlines()
                    if line.strip() and not line.lstrip().startswith(("#", ";"))]
        if directives(text) != directives(expected.read_text()):
            # In particular, never overwrite device/live-network authorization
            # gates set to 0 with the packaged values of 1. Preserve every user
            # setting by refusing custom units, rather than guessing which ones
            # matter to the backend or weakening them during an upgrade.
            fail("custom service unit settings differ; review/preserve them manually")
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("ExecStart=") and line != "ExecStart=/opt/simadmin/simadmin":
            fail("custom ExecStart is unsupported; review the unit manually")
        if line.startswith("WorkingDirectory=") and line != "WorkingDirectory=/opt/simadmin":
            fail("custom WorkingDirectory is unsupported")
        if line.startswith("EnvironmentFile=") or (line.startswith("Environment=") and "SIMADMIN_CONFIG" in line):
            fail("custom service configuration environment is unsupported")
    if "ExecStart=/opt/simadmin/simadmin" not in text.splitlines():
        fail("main unit is missing the canonical ExecStart")


def install(package, root, run=command, machine=None, activate=False, check_only=False, sleep=time.sleep):
    """Install to root (an explicit dependency, not a public relocation option)."""
    root = Path(root)
    package = Path(package).resolve()
    machine = machine or platform.machine()
    # Copy first: validation and application operate on the same private snapshot.
    # This also avoids reading changing files from a removable/network package.
    regular_tree(package)
    with tempfile.TemporaryDirectory(prefix="simadmin-verify-") as temporary:
        stage = Path(temporary) / "package"
        shutil.copytree(package, stage)
        meta = verify(stage, machine)
        prefix = safe_destination(root, "opt/simadmin")
        unit = safe_destination(root, "etc/systemd/system/" + SERVICE)
        for name in PAYLOAD:
            safe_destination(root, "opt/simadmin/" + name)
        for parent in (prefix, unit.parent):
            if parent.exists() and not parent.is_dir():
                fail("install parent is not a directory: " + str(parent))
        unit_directories = ("etc/systemd/system", "run/systemd/system",
                            "usr/lib/systemd/system", "lib/systemd/system")
        for directory in unit_directories:
            for name in ("simadmin.service.d", "service.d"):
                dropin = root / directory / name
                if dropin.exists() and any(dropin.iterdir()):
                    fail("custom systemd drop-ins require manual installation/config validation")
        if prefix.exists() and any(prefix.glob(".install-*")):
            fail("unfinished installer backup found; resolve it before retrying")
        packaged_unit = stage / "system" / SERVICE
        check_unit(packaged_unit)
        if unit.exists():
            check_unit(unit, expected=packaged_unit)
        else:
            for directory in unit_directories[1:]:
                existing = root / directory / SERVICE
                if existing.is_file():
                    check_unit(existing, expected=packaged_unit)
        # Match the service's executable-directory fallback, NOT staging's path.
        config = root / ("data/config.yaml" if (root / "data").is_dir() else "opt/simadmin/config.yaml")
        binary = stage / "simadmin"
        binary.chmod(0o755)
        version = run([str(binary), "--version"])
        if version != "simadmin " + meta["version"]:
            fail("binary version differs from metadata: " + version)
        backend = run([str(binary), "modem-backend-mode", "--config", str(config)])
        if backend not in ("native", "modemmanager"):
            fail("cannot validate configured modem backend")
        if backend == "modemmanager":
            run([str(binary), "modem-backend-mode", "--config", str(config), "--require-mm"])
        print("Verified version={version} commit={commit} arch={arch}; backend=".format(**meta) + backend, flush=True)
        print("Config preflight: " + str(config), flush=True)
        if check_only:
            return
        active = service_state(run, "ActiveState")
        enabled = service_state(run, "UnitFileState")
        if active not in ("active", "inactive", "failed"):
            fail("service is in a transitional/unknown state: " + active)
        if enabled not in ("enabled", "disabled", "", "not-found"):
            fail("unsupported unit state (masked/custom unit?): " + enabled)
        if active == "active" and not activate:
            fail("service is running; stop it in a maintenance window or explicitly use --activate")
        prefix.mkdir(parents=True, exist_ok=True)
        unit.parent.mkdir(parents=True, exist_ok=True)
        # Each temporary directory shares a filesystem with its destination.
        # Backups are only installer-owned code/resources, never user data.
        work = Path(tempfile.mkdtemp(prefix=".install-", dir=prefix))
        try:
            unit_work = Path(tempfile.mkdtemp(prefix=".simadmin-install-", dir=unit.parent))
        except OSError:
            shutil.rmtree(work)
            raise
        changes = []
        touched = False
        enable_attempted = False
        start_attempted = False
        committed = False
        rollback_ok = True
        try:
            for name in PAYLOAD:
                source = stage / name
                target = work / name
                if source.is_dir():
                    shutil.copytree(source, target)
                else:
                    shutil.copy2(source, target)
                for path in [target] + (list(target.rglob("*")) if target.is_dir() else []):
                    path.chmod(0o755 if path.is_dir() or path.name == "simadmin" or path.suffix == ".sh" else 0o644)
            shutil.copy2(stage / "system" / SERVICE, unit_work / "new")
            (unit_work / "new").chmod(0o644)
            # All verification, config parsing and staging completed before stop.
            touched = True
            if active == "active":
                run(["systemctl", "stop", SERVICE])
            replacements = [(work / n, prefix / n, work / (n + ".old")) for n in PAYLOAD]
            replacements.append((unit_work / "new", unit, unit_work / "old"))
            for source, target, backup in replacements:
                had_old = target.exists()
                changes.append((target, backup, had_old))
                if had_old:
                    os.replace(target, backup)
                os.replace(source, target)
            run(["systemctl", "daemon-reload"])
            if activate:
                print("Maintenance activation: starting SimAdmin may activate its configured modem backend.", flush=True)
                enable_attempted = True
                run(["systemctl", "enable", SERVICE])
                start_attempted = True
                run(["systemctl", "start", SERVICE])
                for _ in range(5):
                    sleep(1)
                    if service_state(run, "ActiveState") != "active":
                        fail("main service did not remain active after start")
            committed = True
        except BaseException:
            if touched:
                print("Installation failed; restoring previous code/unit (not config/data).", file=sys.stderr)
                try:
                    if start_attempted:
                        run(["systemctl", "stop", SERVICE])
                    # Disable before restoring/removing the new unit, so even
                    # a failed fresh install can remove its enablement links.
                    if enable_attempted and enabled != "enabled":
                        run(["systemctl", "disable", SERVICE])
                    for target, backup, had_old in reversed(changes):
                        if had_old and not backup.exists():
                            continue  # original rename itself failed
                        if target.is_dir():
                            shutil.rmtree(target)
                        elif target.exists():
                            target.unlink()
                        if had_old:
                            os.replace(backup, target)
                    run(["systemctl", "daemon-reload"])
                    if active == "active":
                        run(["systemctl", "start", SERVICE])
                except BaseException as error:
                    rollback_ok = False
                    print("ROLLBACK INCOMPLETE: {}. Keep backups: {} {}".format(error, work, unit_work), file=sys.stderr)
            raise
        finally:
            if committed or rollback_ok:
                shutil.rmtree(work)
                shutil.rmtree(unit_work)
        print("Installed. " + ("Main service active (not a hardware readiness check)." if activate else
                             "Activation deferred; main service was not enabled or started."), flush=True)
        print("Device resources are staged at " + str(prefix / "devices") + "; no device services were changed.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="verify package/config only; no service or install changes")
    parser.add_argument("--activate", action="store_true", help="explicit maintenance: stop/restart and enable main service")
    args = parser.parse_args()
    if args.check and args.activate:
        parser.error("--check and --activate are mutually exclusive")
    for name, fixed in (("INSTALL_DIR", "/opt/simadmin"), ("PREFIX", "/opt/simadmin"), ("SERVICE_NAME", "simadmin")):
        if os.environ.get(name, fixed) != fixed:
            fail(name + " is unsupported; units require " + fixed)
    if any(os.environ.get(name) for name in ("SIMADMIN_CONFIG", "SIMADMIN_CONFIG_DB")):
        fail("custom config environment is unsupported; installer uses the standard service config path")
    if os.geteuid() != 0:
        fail("run as root")
    # Serialize online/offline invocations; the lock itself is not user data.
    import fcntl
    with open("/run/lock/simadmin-install.lock", "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        def interrupted(signum, frame):
            raise RuntimeError("installation interrupted")
        signal.signal(signal.SIGTERM, interrupted)
        signal.signal(signal.SIGINT, interrupted)
        install(Path(__file__).resolve().parent, Path("/"), activate=args.activate, check_only=args.check)


if __name__ == "__main__":
    try:
        main()
    except (Exception, KeyboardInterrupt) as error:
        print("error: " + str(error), file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError):
            print(error.stderr or "", file=sys.stderr)
        sys.exit(1)
