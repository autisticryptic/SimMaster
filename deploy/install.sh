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
#   systemd units for the IMS QMI endpoint and modem recovery (optional)
#
# Nothing is installed into the kernel, and no udev rule is packaged. Both used
# to be here and both were mistakes -- see the two comment blocks below.

set -e

PREFIX=/opt/simadmin
LOG_DIR=/var/log/simadmin
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

# Diagnostic log directory. The service creates this itself on first write, but
# doing it here pins the mode before any log exists: with redaction turned off
# the file holds IMSIs and message bodies, so it must not be world-readable.
say "preparing diagnostic log directory at $LOG_DIR"
install -d -m 0750 "$LOG_DIR"

if [ "$WITH_IMS" = "1" ]; then
  # --- no kernel module ------------------------------------------------------
  # This used to install and modprobe an out-of-tree `rpmsg_wwan_ctrl_multi`.
  # Do not bring that back. While loaded, its id_table keeps auto-binding spare
  # DATA*_CNTL channels at every boot, racing the modem's Data Services Memory
  # bring-up; that crashes the DSP (smd_dsm_memcpy.c:297) and latches bam-dmux
  # at runtime_status=error, which kills every wwanN interface until a full
  # system reflash. `secondary-qmi-init` now actively purges the module for
  # exactly this reason -- installing it here would fight our own fix.
  #
  # It is also unnecessary: DATA6 is bound through the in-tree
  # `rpmsg_wwan_ctrl` with driver_override, which is how the beta8 reference
  # build ran DATA6 and IMS side by side with no out-of-tree code at all.
  # See docs/QCM410_BAM_DMUX_MODEM_CRASH.md.

  # --- no udev rule ---------------------------------------------------------
  # The rule that keeps ModemManager off SimAdmin's IMS endpoints is generated
  # at runtime by `secondary-qmi-init` into /run/udev/rules.d, naming the port
  # that actually appeared. A packaged rule would have to guess the port name,
  # and the name differs per platform (wwan0qmi1 on one baseband, wwan0at2 on
  # another) -- a wrong guess is either dead weight or, worse, hides a port
  # ModemManager legitimately owns.

  # --- systemd unit ---------------------------------------------------------
  if [ -f system/simadmin-secondary-qmi.service ] && command -v systemctl >/dev/null 2>&1; then
    say "installing systemd unit"
    install -m 0644 system/simadmin-secondary-qmi.service /etc/systemd/system/
    systemctl daemon-reload
    systemctl enable simadmin-secondary-qmi.service || true
  fi

  # --- bounded runtime modem recovery -------------------------------------
  if [ -f system/simadmin-modem-recovery.sh ] &&
     [ -f system/simadmin-modem-recovery.service ] &&
     [ -f system/simadmin-modem-recovery.timer ] &&
     command -v systemctl >/dev/null 2>&1; then
    say "installing periodic modem recovery monitor"
    install -m 0755 system/simadmin-modem-recovery.sh /usr/local/bin/
    install -m 0644 system/simadmin-modem-recovery.service /etc/systemd/system/
    install -m 0644 system/simadmin-modem-recovery.timer /etc/systemd/system/
    systemctl daemon-reload
    systemctl enable --now simadmin-modem-recovery.timer || true
  fi
fi

cat <<EOF

Done.

  binary : $PREFIX/simadmin
  web UI : $PREFIX/www
  logs   : $LOG_DIR

Start the service:
  $PREFIX/simadmin serve --port 3000

IMS/VoLTE endpoint status:
  $PREFIX/simadmin secondary-qmi-init --dry-run     # what it would do
  $PREFIX/simadmin secondary-qmi-init               # prepare it now
  for p in /sys/class/wwan/*; do echo "\$(basename \$p) \$(cat \$p/type)"; done

secondary-qmi-init binds DATA6 through the in-tree rpmsg_wwan_ctrl driver and
accepts the endpoint only after a QMI probe confirms the wds service. A spare
port whose type reads QMI is the one it took. ModemManager may briefly report
no modems while it re-enumerates; it recovers within ~15s.
EOF
