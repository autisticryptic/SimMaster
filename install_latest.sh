#!/bin/sh
# Online transport only. Installation logic and units come from one pinned asset.
# Usage: REPO=owner/repository VERSION=1.2.3 sh install_latest.sh [--check|--activate]
set -eu
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }
case "${1:-}" in
  --help|-h)
    echo 'REPO=owner/repository VERSION=x.y.z sh install_latest.sh [--check|--activate]'
    echo 'VERSION is required (latest is deliberately unsupported). Default: files only.'
    exit 0 ;;
esac
for arg in "$@"; do
  case "$arg" in --check|--activate) ;; *) fail "unknown option: $arg" ;; esac
done
[ "${INSTALL_DIR:-/opt/simadmin}" = /opt/simadmin ] || fail 'only INSTALL_DIR=/opt/simadmin is supported'
[ "${PREFIX:-/opt/simadmin}" = /opt/simadmin ] || fail 'only PREFIX=/opt/simadmin is supported'
[ "${SERVICE_NAME:-simadmin}" = simadmin ] || fail 'only SERVICE_NAME=simadmin is supported'
[ -n "${REPO:-}" ] || fail 'set REPO to the trusted GitHub owner/repository; no legacy repository is assumed'
[ -n "${VERSION:-}" ] && [ "$VERSION" != latest ] || fail 'set VERSION to an explicit release version (not latest)'
[ -z "${ASSET_URL:-}${SERVICE_URL:-}${RAW_BASE:-}${REPO_BRANCH:-}" ] || fail 'legacy asset/raw-branch overrides are not supported'
[ "$(uname -s)" = Linux ] || fail 'only Linux is supported'
case "$(uname -m)" in
  aarch64|arm64) asset=simadmin-linux-arm64.tar.gz; target=aarch64-unknown-linux-musl ;;
  x86_64|amd64) asset=simadmin-linux-amd64.tar.gz; target=x86_64-unknown-linux-musl ;;
  *) fail "unsupported architecture: $(uname -m)" ;;
esac
for cmd in python3 curl mktemp; do command -v "$cmd" >/dev/null 2>&1 || fail "missing command: $cmd"; done
[ "$(id -u)" = 0 ] || fail 'run as root'
version=${VERSION#v}
python3 - "$REPO" "$version" <<'PY'
import re, sys
if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', sys.argv[1]):
    sys.exit('error: invalid REPO')
if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.+-]+)?', sys.argv[2]):
    sys.exit('error: invalid VERSION')
PY
base="https://github.com/$REPO/releases/download/v$version"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
# No silent third-party mirror fallback. HTTPS origin is the trust boundary;
# SHA256 detects corruption/mismatch, not a malicious publisher.
curl --proto '=https' --proto-redir '=https' --connect-timeout 20 --max-time 600 --retry 2 -fSL "$base/$asset" -o "$tmp/$asset"
curl --proto '=https' --proto-redir '=https' --connect-timeout 20 --max-time 120 --retry 2 -fSL "$base/SHA256SUMS.txt" -o "$tmp/SHA256SUMS.txt"
python3 - "$tmp" "$asset" "$version" "$target" <<'PY'
import hashlib, json, pathlib, re, sys, tarfile
root = pathlib.Path(sys.argv[1])
asset, version, arch = sys.argv[2:]
entries = []
for line in (root / 'SHA256SUMS.txt').read_text().splitlines():
    match = re.fullmatch(r'([0-9a-f]{64}) [ *](.+)', line)
    if not match:
        sys.exit('error: invalid release checksum file')
    if match[2] == asset:
        entries.append(match[1])
with (root / asset).open('rb') as stream:
    digest = hashlib.sha256()
    for chunk in iter(lambda: stream.read(1024 * 1024), b''):
        digest.update(chunk)
if len(entries) != 1 or digest.hexdigest() != entries[0]:
    sys.exit('error: release checksum missing, duplicated or mismatched')
package = root / 'package'
package.mkdir()
with tarfile.open(root / asset, 'r:gz') as archive:
    members = archive.getmembers()
    seen = set()
    allowed = {'simadmin', 'www', 'meta.json', 'devices', 'system', 'install.sh', 'installer.py', 'SHA256SUMS'}
    for member in members:
        path = pathlib.PurePosixPath(member.name)
        if (path.is_absolute() or '..' in path.parts or not path.parts or
                path.parts[0] not in allowed or str(path) in seen or
                '\\' in member.name or '\n' in member.name or
                not (member.isfile() or member.isdir())):
            sys.exit('error: unsafe/duplicate archive member: ' + member.name)
        seen.add(str(path))
    # Extract regular files ourselves: no links, ownership, device nodes or
    # archive-supplied permission bits are ever applied, even on Python <3.12.
    for member in members:
        destination = package / member.name
        if member.isdir():
            destination.mkdir(parents=True, exist_ok=True)
        else:
            destination.parent.mkdir(parents=True, exist_ok=True)
            with archive.extractfile(member) as source, destination.open('wb') as target:
                import shutil
                shutil.copyfileobj(source, target)
meta = json.loads((package / 'meta.json').read_text())
if meta.get('version') != version or meta.get('arch') != arch or meta.get('installer_format') != 1:
    sys.exit('error: pinned version/architecture/installer metadata mismatch')
if not (package / 'install.sh').is_file() or not (package / 'installer.py').is_file():
    sys.exit('error: release lacks the offline installer; old releases are unsupported')
PY
sh "$tmp/package/install.sh" "$@"
