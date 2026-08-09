#!/bin/sh

set -u

TAG="${SIMADMIN_RECOVERY_TAG:-SimAdmin-ModemRecovery}"
MMCLI_BIN="${MMCLI_BIN:-mmcli}"
QMICLI_BIN="${QMICLI_BIN:-qmicli}"
SYSTEMCTL_BIN="${SYSTEMCTL_BIN:-systemctl}"
TIMEOUT_BIN="${TIMEOUT_BIN:-timeout}"
SLEEP_BIN="${SLEEP_BIN:-sleep}"
LOGGER_BIN="${LOGGER_BIN:-logger}"
UDEVADM_BIN="${UDEVADM_BIN:-udevadm}"

STATE_DIR="${STATE_DIR:-/run/simadmin}"
SYS_CLASS_NET_DIR="${SYS_CLASS_NET_DIR:-/sys/class/net}"
SYS_CLASS_WWAN_DIR="${SYS_CLASS_WWAN_DIR:-/sys/class/wwan}"
QMI_DEV_DIR="${QMI_DEV_DIR:-/dev}"

STARTUP_TIMEOUT_SECONDS="${STARTUP_TIMEOUT_SECONDS:-120}"
CHECK_INTERVAL_SECONDS="${CHECK_INTERVAL_SECONDS:-5}"
STALE_CONFIRMATIONS="${STALE_CONFIRMATIONS:-3}"
QMI_TIMEOUT_SECONDS="${QMI_TIMEOUT_SECONDS:-15}"
REANNOUNCE_SETTLE_SECONDS="${REANNOUNCE_SETTLE_SECONDS:-2}"
POST_REANNOUNCE_TIMEOUT_SECONDS="${POST_REANNOUNCE_TIMEOUT_SECONDS:-90}"
POST_RESTART_TIMEOUT_SECONDS="${POST_RESTART_TIMEOUT_SECONDS:-90}"
POST_RECOVERY_STABLE_CONFIRMATIONS="${POST_RECOVERY_STABLE_CONFIRMATIONS:-3}"

STATUS_FILE="${STATE_DIR}/modem-recovery-status"
IN_PROGRESS_FILE="${STATE_DIR}/modem-recovery-in-progress"

log() {
  message="$*"
  printf '%s\n' "$message"
  "$LOGGER_BIN" -t "$TAG" -- "$message" >/dev/null 2>&1 || true
}

set_status() {
  mkdir -p "$STATE_DIR"
  printf '%s\n' "$1" > "$STATUS_FILE"
}

cleanup() {
  rm -f "$IN_PROGRESS_FILE"
}

list_modem_ids() {
  "$MMCLI_BIN" -L 2>/dev/null |
    sed -n 's|^.*/Modem/\([^[:space:]]*\).*|\1|p'
}

mm_has_sim() {
  printf '%s\n' "$1" | grep -Eq \
    'primary sim path:[[:space:]]*/org/freedesktop/ModemManager1/SIM/'
}

mm_has_any_sim() {
  modem_ids="$(list_modem_ids)"
  for modem_id in $modem_ids; do
    snapshot="$("$MMCLI_BIN" -m "$modem_id" 2>&1 || true)"
    if mm_has_sim "$snapshot"; then
      return 0
    fi
  done
  return 1
}

modem_port_has_sim() {
  target_port="$1"
  case "$target_port" in
    *[!A-Za-z0-9_.-]*) return 1 ;;
  esac

  modem_ids="$(list_modem_ids)"
  for modem_id in $modem_ids; do
    snapshot="$("$MMCLI_BIN" -m "$modem_id" 2>&1 || true)"
    if mm_has_sim "$snapshot" &&
      printf '%s\n' "$snapshot" | grep -Eq \
        "(^|[[:space:],])${target_port}([[:space:],(]|$)"; then
      return 0
    fi
  done
  return 1
}

candidate_properties() {
  "$UDEVADM_BIN" info --query=property --path="$1" 2>/dev/null || true
}

qcom_candidate_properties() {
  candidate_props="$(candidate_properties "$1")"
  printf '%s\n' "$candidate_props" | grep -qx 'ID_MM_QCOM_SOC=1' || return 1
  printf '%s\n' "$candidate_props" | grep -qx 'ID_MM_CANDIDATE=1' || return 1
  printf '%s\n' "$candidate_props"
}

qmi_is_ready() {
  qmi_port="$1"
  [ -e "$QMI_DEV_DIR/$qmi_port" ] || return 1

  qmi_output="$(
    "$TIMEOUT_BIN" "$QMI_TIMEOUT_SECONDS" "$QMICLI_BIN" \
      -d "$QMI_DEV_DIR/$qmi_port" --device-open-proxy \
      --uim-get-card-status 2>&1 || true
  )"
  printf '%s\n' "$qmi_output" | grep -Eq \
    "Card state:[[:space:]]*'present'" || return 1
  printf '%s\n' "$qmi_output" | grep -Eq \
    "Application type:[[:space:]]*'usim" || return 1
  printf '%s\n' "$qmi_output" | grep -Eq \
    "Application state:[[:space:]]*'ready'"
}

list_ready_qmi_ports() {
  for qmi_path in "$SYS_CLASS_WWAN_DIR"/*qmi*; do
    [ -e "$qmi_path" ] || continue
    qmi_props="$(qcom_candidate_properties "$qmi_path")" || continue
    printf '%s\n' "$qmi_props" | grep -qx 'DEVTYPE=wwan_port' || continue
    qmi_port="$(basename "$qmi_path")"
    if qmi_is_ready "$qmi_port"; then
      printf '%s\n' "$qmi_port"
    fi
  done
}

list_stale_ready_qmi_ports() {
  ready_ports="$1"
  for ready_port in $ready_ports; do
    if ! modem_port_has_sim "$ready_port"; then
      printf '%s\n' "$ready_port"
    fi
  done
}

candidate_uid() {
  uid_props="$(qcom_candidate_properties "$1")" || return 1
  printf '%s\n' "$uid_props" |
    sed -n 's/^ID_MM_PHYSDEV_UID=//p' |
    sed -n '1p'
}

uids_for_ports() {
  ports="$1"
  discovered_uids=""
  for port in $ports; do
    uid="$(candidate_uid "$SYS_CLASS_WWAN_DIR/$port")" || continue
    [ -n "$uid" ] || continue
    if ! printf '%s\n' "$discovered_uids" | grep -Fxq "$uid"; then
      if [ -n "$discovered_uids" ]; then
        discovered_uids="${discovered_uids}
${uid}"
      else
        discovered_uids="$uid"
      fi
    fi
  done
  printf '%s\n' "$discovered_uids"
}

uid_is_selected() {
  selected_uids="$1"
  candidate="$2"
  printf '%s\n' "$selected_uids" | grep -Fxq "$candidate"
}

list_qcom_net_candidates() {
  selected_uids="$1"
  for candidate_path in "$SYS_CLASS_NET_DIR"/*; do
    [ -e "$candidate_path" ] || continue
    props="$(qcom_candidate_properties "$candidate_path")" || continue
    printf '%s\n' "$props" | grep -qx 'DEVTYPE=wwan' || continue
    uid="$(printf '%s\n' "$props" | sed -n 's/^ID_MM_PHYSDEV_UID=//p' | sed -n '1p')"
    if [ -n "$uid" ] && uid_is_selected "$selected_uids" "$uid"; then
      printf '%s\n' "$candidate_path"
    fi
  done
}

list_qcom_control_candidates() {
  selected_uids="$1"
  for candidate_path in "$SYS_CLASS_WWAN_DIR"/*; do
    [ -e "$candidate_path" ] || continue
    props="$(qcom_candidate_properties "$candidate_path")" || continue
    printf '%s\n' "$props" | grep -qx 'DEVTYPE=wwan_port' || continue
    uid="$(printf '%s\n' "$props" | sed -n 's/^ID_MM_PHYSDEV_UID=//p' | sed -n '1p')"
    if [ -n "$uid" ] && uid_is_selected "$selected_uids" "$uid"; then
      printf '%s\n' "$candidate_path"
    fi
  done
}

trigger_paths() {
  action="$1"
  paths="$2"
  trigger_failed=0
  for candidate_path in $paths; do
    if ! "$UDEVADM_BIN" trigger --action="$action" "$candidate_path"; then
      trigger_failed=1
    fi
  done
  return "$trigger_failed"
}

reannounce_stale_qcom_ports() {
  stale_ports="$1"
  selected_uids="$(uids_for_ports "$stale_ports")"
  [ -n "$selected_uids" ] || {
    log "QCOM port reannounce skipped: stale QMI ports have no physical-device UID"
    return 1
  }

  net_candidates="$(list_qcom_net_candidates "$selected_uids")"
  control_candidates="$(list_qcom_control_candidates "$selected_uids")"
  if [ -z "$net_candidates" ] || [ -z "$control_candidates" ]; then
    log "QCOM port reannounce skipped: no complete net/control candidate set"
    return 1
  fi

  reannounce_failed=0
  trigger_paths remove "$control_candidates" || reannounce_failed=1
  trigger_paths remove "$net_candidates" || reannounce_failed=1
  "$UDEVADM_BIN" settle || reannounce_failed=1
  "$SLEEP_BIN" "$REANNOUNCE_SETTLE_SECONDS"

  # Always attempt the add half, even when one remove event failed.
  trigger_paths add "$net_candidates" || reannounce_failed=1
  trigger_paths add "$control_candidates" || reannounce_failed=1
  "$UDEVADM_BIN" settle || reannounce_failed=1

  if [ "$reannounce_failed" -ne 0 ]; then
    log "QCOM port reannounce was incomplete; no destructive fallback will run"
    return 1
  fi

  net_count="$(printf '%s\n' "$net_candidates" | grep -c .)"
  control_count="$(printf '%s\n' "$control_candidates" | grep -c .)"
  log "QCOM ports reannounced for stale physical devices: net=${net_count} control=${control_count}"
}

wait_for_ports_mapped_stable() {
  expected_ports="$1"
  timeout_seconds="$2"
  required="$3"
  elapsed=0
  stable_count=0

  while [ "$elapsed" -lt "$timeout_seconds" ]; do
    all_mapped=1
    for expected_port in $expected_ports; do
      if ! modem_port_has_sim "$expected_port"; then
        all_mapped=0
        break
      fi
    done

    if [ "$all_mapped" -eq 1 ]; then
      stable_count=$((stable_count + 1))
      if [ "$stable_count" -ge "$required" ]; then
        return 0
      fi
    else
      stable_count=0
    fi

    "$SLEEP_BIN" "$CHECK_INTERVAL_SECONDS"
    elapsed=$((elapsed + CHECK_INTERVAL_SECONDS))
  done
  return 1
}

trap cleanup EXIT INT TERM
mkdir -p "$STATE_DIR"
set_status "observing"
log "Cold-start modem observation started"

elapsed=0
stale_count=0
stale_key=""
stale_ports=""
while [ "$elapsed" -lt "$STARTUP_TIMEOUT_SECONDS" ]; do
  ready_ports="$(list_ready_qmi_ports)"
  stale_ports="$(list_stale_ready_qmi_ports "$ready_ports")"

  if [ -z "$stale_ports" ]; then
    if [ -n "$ready_ports" ] || mm_has_any_sim; then
      set_status "healthy"
      log "Every ready QMI SIM is represented by ModemManager; recovery is not needed"
      exit 0
    fi
    stale_count=0
    stale_key=""
  elif [ "$stale_ports" = "$stale_key" ]; then
    stale_count=$((stale_count + 1))
  else
    stale_key="$stale_ports"
    stale_count=1
  fi

  if [ -n "$stale_ports" ]; then
    log "QMI-ready ports are absent from ModemManager (${stale_count}/${STALE_CONFIRMATIONS}): $(printf '%s' "$stale_ports" | tr '\n' ' ')"
    if [ "$stale_count" -ge "$STALE_CONFIRMATIONS" ]; then
      break
    fi
  fi

  "$SLEEP_BIN" "$CHECK_INTERVAL_SECONDS"
  elapsed=$((elapsed + CHECK_INTERVAL_SECONDS))
done

if [ "$stale_count" -lt "$STALE_CONFIRMATIONS" ]; then
  set_status "no-safe-action"
  log "No persistent QMI-ready/ModemManager mismatch was confirmed; leaving every baseband untouched"
  exit 0
fi

touch "$IN_PROGRESS_FILE"
set_status "reannouncing-qcom-ports"
log "Confirmed persistent QMI-ready/ModemManager mismatch; reannouncing only the affected QCOM port groups"
if ! reannounce_stale_qcom_ports "$stale_ports"; then
  set_status "reannounce-failed"
  exit 1
fi

if wait_for_ports_mapped_stable \
  "$stale_ports" "$POST_REANNOUNCE_TIMEOUT_SECONDS" \
  "$POST_RECOVERY_STABLE_CONFIRMATIONS"; then
  set_status "recovered"
  log "ModemManager recovered after targeted QCOM port reannounce"
  exit 0
fi

set_status "restarting-modemmanager"
log "Targeted reannounce did not recover ModemManager; restarting ModemManager once"
if ! "$SYSTEMCTL_BIN" restart ModemManager.service; then
  set_status "restart-command-failed"
  log "ModemManager restart failed; MPSS and the operating system will not be restarted"
  exit 1
fi

set_status "reannouncing-after-restart"
if ! reannounce_stale_qcom_ports "$stale_ports"; then
  set_status "post-restart-reannounce-failed"
  exit 1
fi

if ! wait_for_ports_mapped_stable \
  "$stale_ports" "$POST_RESTART_TIMEOUT_SECONDS" \
  "$POST_RECOVERY_STABLE_CONFIRMATIONS"; then
  set_status "recovery-failed"
  log "ModemManager did not recover after bounded safe actions; MPSS and the operating system will not be restarted"
  exit 1
fi

set_status "recovered"
log "ModemManager recovered after one restart and targeted QCOM port reannounce"
exit 0
