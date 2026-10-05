#!/bin/sh
# Local and CI builds share exactly the same installer/OTA archive contract.
# TARGET may be aarch64-unknown-linux-musl (default) or x86_64-unknown-linux-musl.
set -eu
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec python3 "$HERE/package-release.py" "$@"
