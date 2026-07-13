#!/bin/sh

set -eu

INSTALL_DIR="${INSTALL_DIR:-/opt/simadmin}"
SERVICE_NAME="${SERVICE_NAME:-simadmin}"
KEEP_USER_DATA="${KEEP_USER_DATA:-0}"

MODEM_RECOVERY_SERVICE_NAME="${MODEM_RECOVERY_SERVICE_NAME:-simadmin-modem-recovery}"
MODEM_RECOVERY_SCRIPT="${MODEM_RECOVERY_SCRIPT:-/usr/local/bin/simadmin-modem-recovery.sh}"
NM_CONF="${NM_CONF:-/etc/NetworkManager/conf.d/99-simadmin-unmanaged-modem.conf}"
MM_DEBUG_CONF="${MM_DEBUG_CONF:-/etc/systemd/system/ModemManager.service.d/99-simadmin-debug.conf}"
OTA_STAGING_DIR="${OTA_STAGING_DIR:-/tmp/ota_staging}"
DEVICE_CONFIG_PATH="${DEVICE_CONFIG_PATH:-/data/config.json}"

usage() {
  printf '%s\n' \
    'SimAdmin uninstall script' \
    '' \
    'Usage:' \
    '  sh uninstall.sh [options]' \
    '' \
    'Options:' \
    '  --purge                Remove everything, including user data (default)' \
    '  --keep-user-data       Keep data.db, SQLite sidecar files, and config.json' \
    '  --install-dir PATH     Installed directory (default: /opt/simadmin)' \
    '  --service-name NAME    Main systemd service name (default: simadmin)' \
    '  -h, --help             Show this help' \
    '' \
    'Environment:' \
    '  INSTALL_DIR=/opt/simadmin' \
    '  SERVICE_NAME=simadmin' \
    '  KEEP_USER_DATA=1       Same as --keep-user-data'
}

require_root() {
  if [ "$(id -u)" -ne 0 ]; then
    echo "error: please run as root" >&2
    exit 1
  fi
}

command_exists() {
  command -v "$1" >/dev/null 2>&1
}

normalize_keep_user_data() {
  case "$KEEP_USER_DATA" in
    1|true|TRUE|yes|YES|y|Y) KEEP_USER_DATA=1 ;;
    *) KEEP_USER_DATA=0 ;;
  esac
}

parse_args() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --purge)
        KEEP_USER_DATA=0
        ;;
      --keep-user-data)
        KEEP_USER_DATA=1
        ;;
      --install-dir)
        shift
        if [ "$#" -eq 0 ]; then
          echo "error: --install-dir requires a value" >&2
          exit 1
        fi
        INSTALL_DIR="$1"
        ;;
      --install-dir=*)
        INSTALL_DIR="${1#*=}"
        ;;
      --service-name)
        shift
        if [ "$#" -eq 0 ]; then
          echo "error: --service-name requires a value" >&2
          exit 1
        fi
        SERVICE_NAME="$1"
        ;;
      --service-name=*)
        SERVICE_NAME="${1#*=}"
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      *)
        echo "error: unknown option: $1" >&2
        usage >&2
        exit 1
        ;;
    esac
    shift
  done
}

assert_safe_install_dir() {
  case "$INSTALL_DIR" in
    ""|"/"|"/opt"|"/usr"|"/usr/local"|"/etc"|"/var"|"/tmp"|"/data"|"/home"|"/root")
      echo "error: unsafe INSTALL_DIR: ${INSTALL_DIR}" >&2
      exit 1
      ;;
    *"/.."*)
      echo "error: INSTALL_DIR must not contain '..': ${INSTALL_DIR}" >&2
      exit 1
      ;;
    /*)
      ;;
    *)
      echo "error: INSTALL_DIR must be an absolute path: ${INSTALL_DIR}" >&2
      exit 1
      ;;
  esac
}

assert_safe_service_name() {
  name="$1"
  value="$2"

  case "$value" in
    ""|*/*|*..*|*[!abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_.@-]*)
      echo "error: unsafe ${name}: ${value}" >&2
      exit 1
      ;;
  esac
}

remove_path() {
  path="$1"
  case "$path" in
    ""|"/")
      echo "error: refusing to remove unsafe path: ${path}" >&2
      exit 1
      ;;
  esac

  if [ -e "$path" ] || [ -L "$path" ]; then
    echo "==> removing ${path}"
    rm -rf -- "$path"
  fi
}

stop_disable_service() {
  unit="$1"

  if ! command_exists systemctl; then
    return 0
  fi

  echo "==> stopping ${unit}"
  systemctl stop "$unit" >/dev/null 2>&1 || true

  echo "==> disabling ${unit}"
  systemctl disable "$unit" >/dev/null 2>&1 || true
}

remove_systemd_unit() {
  unit="$1"
  remove_path "/etc/systemd/system/multi-user.target.wants/${unit}"
  remove_path "/etc/systemd/system/${unit}"
}

cleanup_systemd() {
  if ! command_exists systemctl; then
    return 0
  fi

  echo "==> reloading systemd"
  systemctl daemon-reload >/dev/null 2>&1 || true
  systemctl reset-failed "${SERVICE_NAME}.service" >/dev/null 2>&1 || true
  systemctl reset-failed "${MODEM_RECOVERY_SERVICE_NAME}.service" >/dev/null 2>&1 || true
}

restart_networkmanager_if_active() {
  if ! command_exists systemctl; then
    return 0
  fi

  if systemctl is-active --quiet NetworkManager.service; then
    echo "==> restarting NetworkManager"
    systemctl restart NetworkManager.service || true
  fi
}

restart_modemmanager_if_active() {
  if ! command_exists systemctl; then
    return 0
  fi

  if systemctl is-active --quiet ModemManager.service; then
    echo "==> restarting ModemManager"
    systemctl restart ModemManager.service || true
  fi
}

remove_install_files_keep_data() {
  remove_path "${INSTALL_DIR}/simadmin"
  remove_path "${INSTALL_DIR}/www"
  remove_path "${INSTALL_DIR}/lpac"
  remove_path "${INSTALL_DIR}/meta.json"

  if [ -d "$INSTALL_DIR" ]; then
    if rmdir "$INSTALL_DIR" >/dev/null 2>&1; then
      echo "==> removed empty install dir ${INSTALL_DIR}"
    else
      echo "==> kept user data under ${INSTALL_DIR}"
    fi
  fi

  if [ -f "$DEVICE_CONFIG_PATH" ]; then
    echo "==> kept user config ${DEVICE_CONFIG_PATH}"
  fi
}

remove_install_files_purge() {
  remove_path "$INSTALL_DIR"
  remove_path "$DEVICE_CONFIG_PATH"
}

main() {
  parse_args "$@"
  normalize_keep_user_data
  require_root
  assert_safe_install_dir
  assert_safe_service_name SERVICE_NAME "$SERVICE_NAME"
  assert_safe_service_name MODEM_RECOVERY_SERVICE_NAME "$MODEM_RECOVERY_SERVICE_NAME"

  echo "==> uninstalling SimAdmin"
  if [ "$KEEP_USER_DATA" -eq 1 ]; then
    echo "==> mode: keep user data"
  else
    echo "==> mode: purge all data"
  fi

  stop_disable_service "${SERVICE_NAME}.service"
  stop_disable_service "${MODEM_RECOVERY_SERVICE_NAME}.service"

  remove_systemd_unit "${SERVICE_NAME}.service"
  remove_systemd_unit "${MODEM_RECOVERY_SERVICE_NAME}.service"
  cleanup_systemd

  remove_path "$MODEM_RECOVERY_SCRIPT"
  remove_path "$NM_CONF"
  remove_path "$MM_DEBUG_CONF"
  remove_path "$OTA_STAGING_DIR"

  if [ "$KEEP_USER_DATA" -eq 1 ]; then
    remove_install_files_keep_data
  else
    remove_install_files_purge
  fi

  restart_networkmanager_if_active
  restart_modemmanager_if_active

  echo "==> done"
}

main "$@"
