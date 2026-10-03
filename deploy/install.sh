#!/bin/sh
# Run from any working directory; the installer and payload travel together.
set -eu
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
command -v python3 >/dev/null 2>&1 || {
  echo 'error: Python 3.8+ is required; no files or services were changed' >&2
  exit 1
}
exec python3 "$HERE/installer.py" "$@"
