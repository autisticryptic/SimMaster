#!/bin/sh

set -u

tool="${0##*/}"
fixture="${RECOVERY_TEST_FIXTURE:?RECOVERY_TEST_FIXTURE is required}"
state_file="$fixture/mapped-ports"
mode_file="$fixture/mode"
event_log="$fixture/udev-events"
systemctl_log="$fixture/systemctl-events"
round_file="$fixture/reannounce-round"

mapped_port_for_id() {
  line_number=$(($1 + 1))
  sed -n "${line_number}p" "$state_file"
}

append_mapped_port() {
  port="$1"
  if ! grep -Fxq "$port" "$state_file"; then
    printf '%s\n' "$port" >> "$state_file"
  fi
}

case "$tool" in
  mmcli)
    if [ "${1:-}" = "-L" ]; then
      modem_id=0
      while IFS= read -r port; do
        [ -n "$port" ] || continue
        printf '    /org/freedesktop/ModemManager1/Modem/%s [TEST] %s\n' \
          "$modem_id" "$port"
        modem_id=$((modem_id + 1))
      done < "$state_file"
      exit 0
    fi

    if [ "${1:-}" = "-m" ]; then
      port="$(mapped_port_for_id "${2:-999}")"
      [ -n "$port" ] || exit 1
      printf '%s\n' \
        "  System   |           primary port: $port" \
        "           |                  ports: $port (qmi)" \
        "  SIM      |       primary sim path: /org/freedesktop/ModemManager1/SIM/${2}"
      exit 0
    fi
    exit 1
    ;;
  qmicli)
    printf '%s\n' "$*" >> "$fixture/qmicli-events"
    printf '%s\n' \
      "Card state: 'present'" \
      "Application type:  'usim (2)'" \
      "Application state: 'ready'"
    ;;
  timeout)
    shift
    exec "$@"
    ;;
  sleep | logger)
    exit 0
    ;;
  systemctl)
    printf '%s\n' "$*" >> "$systemctl_log"
    ;;
  udevadm)
    operation="${1:-}"
    shift || true
    case "$operation" in
      info)
        path=""
        for argument in "$@"; do
          case "$argument" in
            --path=*) path="${argument#--path=}" ;;
          esac
        done
        name="${path##*/}"
        case "$name" in
          wwan0*) uid="qcom-a" ;;
          wwan1*) uid="qcom-b" ;;
          *) exit 1 ;;
        esac
        case "$path" in
          "$fixture/sys/net"/*) devtype="wwan" ;;
          "$fixture/sys/wwan"/*) devtype="wwan_port" ;;
          *) exit 1 ;;
        esac
        printf '%s\n' \
          "DEVTYPE=$devtype" \
          "ID_MM_QCOM_SOC=1" \
          "ID_MM_PHYSDEV_UID=$uid" \
          "ID_MM_CANDIDATE=1"
        ;;
      trigger)
        action=""
        path=""
        for argument in "$@"; do
          case "$argument" in
            --action=*) action="${argument#--action=}" ;;
            *) path="$argument" ;;
          esac
        done
        printf '%s %s\n' "$action" "$path" >> "$event_log"

        name="${path##*/}"
        if [ "$action" = "add" ] && [ "$path" = "$fixture/sys/net/wwan0" ]; then
          current_round="$(cat "$round_file")"
          printf '%s\n' "$((current_round + 1))" > "$round_file"
        fi
        if [ "$action" = "add" ] && [ "$name" = "wwan0qmi0" ]; then
          mode="$(cat "$mode_file")"
          current_round="$(cat "$round_file")"
          case "$mode" in
            reannounce | multi) append_mapped_port "wwan0qmi0" ;;
            restart)
              if [ "$current_round" -ge 2 ]; then
                append_mapped_port "wwan0qmi0"
              fi
              ;;
          esac
        fi
        ;;
      settle)
        exit 0
        ;;
      *) exit 1 ;;
    esac
    ;;
  *)
    printf 'Unexpected recovery mock command: %s\n' "$tool" >&2
    exit 1
    ;;
esac
