#!/bin/sh
# SimAdmin deployment installer.
#
# Usage (from the unpacked zip):
#   sh install.sh              # install everything
#   sh install.sh --no-device-resources  # skip device-specific services
#
# Installs:
#   /opt/simadmin/simadmin          the service binary
#   /opt/simadmin/www/              the web UI
#   resources selected by the detected device driver (optional)
#
# Nothing is installed into the kernel, and no udev rule is packaged. Both used
# to be here and both were mistakes -- see the two comment blocks below.

set -e

PREFIX=/opt/simadmin
LOG_DIR=/var/log/simadmin
WITH_DEVICE_RESOURCES=1
[ "${1:-}" = "--no-device-resources" ] && WITH_DEVICE_RESOURCES=0

say() { printf '==> %s\n' "$*"; }

[ "$(id -u)" = "0" ] || { echo "run as root"; exit 1; }

say "installing binary and web UI to $PREFIX"
install -d "$PREFIX" "$PREFIX/www"
install -m 0755 simadmin "$PREFIX/simadmin"
rm -rf "$PREFIX/www"
install -d "$PREFIX/www"
cp -r www/. "$PREFIX/www/"

# Diagnostic log directory. The service creates this itself on first write, but
# doing it here pins the mode before any log exists: with redaction turned off
# the file holds IMSIs and message bodies, so it must not be world-readable.
say "preparing diagnostic log directory at $LOG_DIR"
install -d -m 0750 "$LOG_DIR"

if [ "$WITH_DEVICE_RESOURCES" = "1" ]; then
  say "installing resources selected by the detected device driver"
  "$PREFIX/simadmin" install-device-resources --staging-dir "$(pwd)"
fi

cat <<EOF

Done.

  binary : $PREFIX/simadmin
  web UI : $PREFIX/www
  logs   : $LOG_DIR

Start the service:
  $PREFIX/simadmin serve --port 3000

Device-native bearer status:
  $PREFIX/simadmin device-init --dry-run     # report the detected driver's plan
  $PREFIX/simadmin device-init               # prepare it now
EOF
