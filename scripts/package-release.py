#!/usr/bin/env python3
"""One package format for local builds and the release matrix (stdlib only)."""
import argparse
import datetime
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("installer", ROOT / "deploy/installer.py")
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


def pack(binary, frontend, target, version, commit, output):
    machine = {"aarch64-unknown-linux-musl": "aarch64", "x86_64-unknown-linux-musl": "x86_64"}.get(target)
    if not machine:
        raise ValueError("unsupported package target: " + target)
    output = Path(output).resolve()
    with tempfile.TemporaryDirectory(prefix="simadmin-pack-") as temp:
        stage = Path(temp)
        shutil.copy2(binary, stage / "simadmin")
        (stage / "simadmin").chmod(0o755)
        shutil.copytree(frontend, stage / "www", symlinks=True)
        (stage / "devices").mkdir()
        for device in sorted((ROOT / "deploy/devices").iterdir()):
            if (device / "system").is_dir():
                shutil.copytree(device / "system", stage / "devices" / device.name / "system", symlinks=True)
        (stage / "system").mkdir()
        shutil.copy2(ROOT / "scripts/simadmin.service", stage / "system/simadmin.service")
        for name in ("install.sh", "installer.py"):
            shutil.copy2(ROOT / "deploy" / name, stage / name)
        files = installer.regular_tree(stage)
        frontend_hashes = sorted(installer.digest(path, "md5") for name, path in files.items() if name.startswith("www/"))
        metadata = {
            "installer_format": 1, "version": version, "commit": commit,
            "build_time": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "binary_md5": installer.digest(stage / "simadmin", "md5"),
            "frontend_md5": hashlib.md5("".join(h + "\n" for h in frontend_hashes).encode()).hexdigest(),
            "arch": target,
        }
        (stage / "meta.json").write_text(json.dumps(metadata, indent=2) + "\n")
        files = installer.regular_tree(stage)
        (stage / "SHA256SUMS").write_text("".join(
            installer.digest(path) + "  " + name + "\n" for name, path in sorted(files.items())))
        installer.verify(stage, machine)
        output.parent.mkdir(parents=True, exist_ok=True)
        # Do not leave a partially written package at the published name.
        fd, temporary = tempfile.mkstemp(prefix=".package-", dir=output.parent)
        os.close(fd)
        try:
            with tarfile.open(temporary, "w:gz") as archive:
                for path in sorted(stage.iterdir()):
                    archive.add(path, arcname=path.name)
            os.replace(temporary, output)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
        checksum = installer.digest(output)
        output.with_name(output.name + ".sha256").write_text(checksum + "  " + output.name + "\n")
        print(str(output))
        print("SHA256: " + checksum)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", default=os.environ.get("TARGET", "aarch64-unknown-linux-musl"))
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--frontend", type=Path, default=ROOT / "frontend/dist")
    parser.add_argument("--version", default=os.environ.get("VERSION") or (ROOT / "VERSION").read_text().strip())
    parser.add_argument("--commit", default=None)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    names = {"aarch64-unknown-linux-musl": "arm64", "x86_64-unknown-linux-musl": "amd64"}
    if args.target not in names:
        parser.error("unsupported target: " + args.target)
    binary = args.binary or ROOT / "backend/target" / args.target / "release/simadmin"
    output = args.output or ROOT / "release" / ("simadmin-linux-" + names[args.target] + ".tar.gz")
    commit = args.commit or subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    pack(binary, args.frontend, args.target, args.version, commit, output)


if __name__ == "__main__":
    main()
