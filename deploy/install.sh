#!/bin/sh
# SimAdmin deployment installer.
#
# Usage (from the unpacked zip):
#   sh install.sh              # install everything
#   sh install.sh --no-ims     # skip the IMS/VoLTE secondary-QMI runtime
#
# Installs:
#   /opt/simadmin/simadmin          the service binary
#   /opt/simadmin/www/              the web UI
#   kernel module + systemd unit + udev rule for the IMS QMI endpoint (optional)

set -e

PREFIX=/opt/simadmin
WITH_IMS=1
[ "${1:-}" = "--no-ims" ] && WITH_IMS=0

say() { printf '==> %s\n' "$*"; }

[ "$(id -u)" = "0" ] || { echo "run as root"; exit 1; }

say "installing binary and web UI to $PREFIX"
install -d "$PREFIX" "$PREFIX/www"
install -m 0755 simadmin "$PREFIX/simadmin"
rm -rf "$PREFIX/www"
install -d "$PREFIX/www"
cp -r www/. "$PREFIX/www/"

if [ "$WITH_IMS" = "1" ]; then
  KVER=$(uname -r)

  # --- kernel module -------------------------------------------------------
  # Exposes spare Qualcomm DATA*_CNTL channels as real QMI ports so IMS/VoLTE
  # can run on its own endpoint instead of fighting ModemManager for the
  # primary one. Without this the IMS bearer either fails with
  # interface-in-use-config-match or wedges the baseband.
  if [ -f "kernel/rpmsg_wwan_ctrl_multi.ko" ]; then
    say "installing prebuilt kernel module for $KVER"
    install -d "/lib/modules/$KVER/extra/simadmin"
    install -m 0644 kernel/rpmsg_wwan_ctrl_multi.ko \
      "/lib/modules/$KVER/extra/simadmin/"
    depmod -a "$KVER" || true
  elif [ -d kernel/src ]; then
    say "building kernel module from source"
    if [ ! -e "/lib/modules/$KVER/build/Makefile" ]; then
      echo "    kernel headers missing at /lib/modules/$KVER/build — skipping."
      echo "    install them, then run: cd kernel/src && make && make install"
    elif ! command -v gcc >/dev/null 2>&1 || ! command -v make >/dev/null 2>&1; then
      echo "    gcc/make missing — skipping."
      echo "    apt-get install -y --no-install-recommends gcc make"
      echo "    then: cd kernel/src && make && make install"
    else
      ( cd kernel/src && make && make install )
    fi
  fi

  # --- udev rule ------------------------------------------------------------
  if [ -f system/99-simadmin-secondary-qmi.rules ]; then
    say "installing udev rule (keeps ModemManager off the IMS endpoint)"
    install -d /etc/udev/rules.d
    install -m 0644 system/99-simadmin-secondary-qmi.rules /etc/udev/rules.d/
    udevadm control --reload-rules 2>/dev/null || true
  fi

  # --- systemd unit ---------------------------------------------------------
  if [ -f system/simadmin-secondary-qmi.service ] && command -v systemctl >/dev/null 2>&1; then
    say "installing systemd unit"
    install -m 0644 system/simadmin-secondary-qmi.service /etc/systemd/system/
    systemctl daemon-reload
    systemctl enable simadmin-secondary-qmi.service || true
  fi

  say "loading module now (safe if already loaded)"
  modprobe rpmsg_wwan_ctrl_multi 2>/dev/null || \
    insmod "/lib/modules/$KVER/extra/simadmin/rpmsg_wwan_ctrl_multi.ko" 2>/dev/null || \
    echo "    could not load; check: modinfo rpmsg_wwan_ctrl_multi"
fi

cat <<EOF

Done.

  binary : $PREFIX/simadmin
  web UI : $PREFIX/www

Start the service:
  $PREFIX/simadmin serve --port 3000

IMS/VoLTE endpoint status:
  $PREFIX/simadmin secondary-qmi-init --dry-run     # what it would do
  $PREFIX/simadmin secondary-qmi-init               # prepare it now
  for p in /sys/class/wwan/*; do echo "\$(basename \$p) \$(cat \$p/type)"; done

A spare port showing type=QMI (e.g. wwan0qmi1) means the module is working.
After loading the module ModemManager may briefly report no modems while it
re-enumerates; it recovers within ~15s.
EOF
