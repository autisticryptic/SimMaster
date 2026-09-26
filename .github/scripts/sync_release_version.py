"""Synchronize release manifests without resolving or building Rust dependencies."""
import argparse
import json
from pathlib import Path
import re
import tomllib

from release_version import resolve_version

ROOT = Path(__file__).resolve().parents[2]
LOCK_BLOCK = re.compile(r"(?ms)^\[\[package\]\]\s*\n.*?(?=^\[\[package\]\]|\Z)")
PACKAGE_BLOCK = re.compile(r"(?ms)^\[package\]\s*\n.*?(?=^\[|\Z)")
VERSION_LINE = re.compile(r'(?m)^(version\s*=\s*)"[^"]*"')


def read_versions(root):
    root = Path(root)
    cargo = tomllib.loads((root / "backend/Cargo.toml").read_text(encoding="utf-8-sig"))
    lock = tomllib.loads((root / "backend/Cargo.lock").read_text(encoding="utf-8-sig"))
    web = json.loads((root / "frontend/package.json").read_text(encoding="utf-8-sig"))
    packages = [p for p in lock.get("package", []) if p.get("name") == "simadmin"]
    if cargo.get("package", {}).get("name") != "simadmin" or len(packages) != 1:
        raise ValueError("Expected one simadmin package in Cargo.toml and Cargo.lock")
    if web.get("name") != "simadmin-web":
        raise ValueError("Unexpected frontend package")
    return {
        "VERSION": (root / "VERSION").read_text(encoding="utf-8-sig").strip(),
        "backend/Cargo.toml": cargo["package"]["version"],
        "backend/Cargo.lock": packages[0]["version"],
        "frontend/package.json": web["version"],
    }


def check_versions(root, expected=None):
    versions = read_versions(root)
    version = resolve_version(expected if expected is not None else versions["VERSION"])["version"]
    mismatches = [name for name, value in versions.items() if value != version]
    if mismatches:
        raise ValueError("Version drift in: " + ", ".join(mismatches))
    return version


def replace_version(block, version):
    result, count = VERSION_LINE.subn(lambda match: match[1] + '"' + version + '"', block)
    if count != 1:
        raise ValueError("Expected exactly one version field in the selected package")
    return result


def sync_versions(root, version):
    root = Path(root)
    version = resolve_version(version)["version"]
    # Validate all inputs and calculate every replacement before writing any file.
    read_versions(root)
    cargo = (root / "backend/Cargo.toml").read_text(encoding="utf-8-sig")
    lock = (root / "backend/Cargo.lock").read_text(encoding="utf-8-sig")
    blocks = list(PACKAGE_BLOCK.finditer(cargo))
    if len(blocks) != 1:
        raise ValueError("Missing or ambiguous Cargo package section")
    block = blocks[0]
    new_cargo = cargo[:block.start()] + replace_version(block[0], version) + cargo[block.end():]
    blocks = [m for m in LOCK_BLOCK.finditer(lock)
              if re.search(r'(?m)^name\s*=\s*"simadmin"\s*$', m[0])]
    if len(blocks) != 1:
        raise ValueError("Missing or ambiguous simadmin lockfile entry")
    block = blocks[0]
    new_lock = lock[:block.start()] + replace_version(block[0], version) + lock[block.end():]
    web = json.loads((root / "frontend/package.json").read_text(encoding="utf-8-sig"))
    web["version"] = version
    outputs = {
        "VERSION": version + "\n",
        "backend/Cargo.toml": new_cargo,
        "backend/Cargo.lock": new_lock,
        "frontend/package.json": json.dumps(web, ensure_ascii=False, indent=2) + "\n",
    }
    for name, text in outputs.items():
        (root / name).write_text(text, encoding="utf8")
    check_versions(root, version)
    return version


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", nargs="?")
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args()
    if args.check:
        version = check_versions(args.root, args.version)
    elif args.version:
        version = sync_versions(args.root, args.version)
    else:
        parser.error("Provide a version to synchronize, or use --check")
    print(f"Release manifests consistent: {version}")


if __name__ == "__main__":
    main()
