"""Resolve explicit release versions; beta builds must never become latest."""
import os
from pathlib import Path
import re


VERSION_PATTERN = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
)


def resolve_version(current, requested="auto", event="push", release_type=""):
    current = current.strip()
    requested = requested.strip()
    if event not in ("push", "workflow_dispatch"):
        raise ValueError(f"Unsupported release event: {event}")
    if event == "push":
        version, source = current, "VERSION file (push)"
    elif requested in ("", "auto"):
        version, source = current, "VERSION file"
    else:
        version, source = requested, "workflow input"
    match = VERSION_PATTERN.fullmatch(version)
    if not match:
        raise ValueError("Expected an explicit SemVer, e.g. 1.1.4 or 1.1.4-beta1")
    suffix = match.group(4)
    if suffix and any(part.isdigit() and len(part) > 1 and part[0] == "0"
                      for part in suffix.split(".")):
        raise ValueError("Numeric prerelease identifiers must not contain leading zeros")
    # A suffix takes precedence over the dispatch UI's default/latest choice.
    prerelease = bool(suffix) or (
        event == "workflow_dispatch" and release_type == "预发布版（pre-release）"
    )
    return {
        "version": version,
        "current_version": current,
        "version_source": source,
        "prerelease": str(prerelease).lower(),
        "make_latest": str(not prerelease).lower(),
        "release_type": "pre-release" if prerelease else "latest release",
    }


def main():
    values = resolve_version(
        Path("VERSION").read_text(encoding="utf-8-sig"),
        os.environ.get("INPUT_VERSION", "auto"),
        os.environ.get("EVENT_NAME", "push"),
        os.environ.get("INPUT_RELEASE_TYPE", ""),
    )
    with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf8") as output:
        for key, value in values.items():
            output.write(f"{key}={value}\n")
    print(f"Version: {values['version']} ({values['version_source']})")
    print(f"Release type: {values['release_type']}; latest: {values['make_latest']}")


if __name__ == "__main__":
    main()
