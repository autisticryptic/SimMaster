#!/bin/sh

set -u

test_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
recovery_script="$test_dir/../simadmin-modem-recovery.sh"
mock_script="$test_dir/fixtures/modem-recovery-command-mock.sh"
test_root="$(mktemp -d)"
trap 'rm -rf "$test_root"' EXIT INT TERM

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

assert_status() {
  expected="$1"
  actual="$(cat "$case_root/state/modem-recovery-status")"
  [ "$actual" = "$expected" ] ||
    fail "$case_name status: expected $expected, got $actual"
}

prepare_case() {
  case_name="$1"
  mode="$2"
  case_root="$test_root/$case_name"
  mkdir -p \
    "$case_root/bin" \
    "$case_root/dev" \
    "$case_root/state" \
    "$case_root/sys/net" \
    "$case_root/sys/wwan"
  : > "$case_root/mapped-ports"
  : > "$case_root/udev-events"
  : > "$case_root/systemctl-events"
  : > "$case_root/qmicli-events"
  printf '%s\n' "$mode" > "$case_root/mode"
  printf '0\n' > "$case_root/reannounce-round"

  for command_name in mmcli qmicli systemctl timeout sleep logger udevadm; do
    ln -s "$mock_script" "$case_root/bin/$command_name"
  done
}

add_modem_fixture() {
  index="$1"
  : > "$case_root/sys/net/wwan${index}"
  : > "$case_root/sys/wwan/wwan${index}at0"
  : > "$case_root/sys/wwan/wwan${index}qmi0"
  : > "$case_root/dev/wwan${index}qmi0"
}

run_recovery() {
  RECOVERY_TEST_FIXTURE="$case_root" \
  MMCLI_BIN="$case_root/bin/mmcli" \
  QMICLI_BIN="$case_root/bin/qmicli" \
  SYSTEMCTL_BIN="$case_root/bin/systemctl" \
  TIMEOUT_BIN="$case_root/bin/timeout" \
  SLEEP_BIN="$case_root/bin/sleep" \
  LOGGER_BIN="$case_root/bin/logger" \
  UDEVADM_BIN="$case_root/bin/udevadm" \
  STATE_DIR="$case_root/state" \
  SYS_CLASS_NET_DIR="$case_root/sys/net" \
  SYS_CLASS_WWAN_DIR="$case_root/sys/wwan" \
  QMI_DEV_DIR="$case_root/dev" \
  STARTUP_TIMEOUT_SECONDS=3 \
  CHECK_INTERVAL_SECONDS=1 \
  STALE_CONFIRMATIONS=2 \
  QMI_TIMEOUT_SECONDS=1 \
  REANNOUNCE_SETTLE_SECONDS=0 \
  POST_REANNOUNCE_TIMEOUT_SECONDS=2 \
  POST_RESTART_TIMEOUT_SECONDS=2 \
  POST_RECOVERY_STABLE_CONFIRMATIONS=1 \
    sh "$recovery_script" > "$case_root/output" 2>&1
}

prepare_case healthy healthy
add_modem_fixture 0
printf 'wwan0qmi0\n' > "$case_root/mapped-ports"
run_recovery || fail "healthy case returned failure"
assert_status healthy
[ ! -s "$case_root/udev-events" ] || fail "healthy case emitted udev events"
[ ! -s "$case_root/systemctl-events" ] || fail "healthy case restarted a service"
[ ! -s "$case_root/qmicli-events" ] || fail "healthy case unnecessarily probed QMI"

prepare_case reannounce reannounce
add_modem_fixture 0
run_recovery || fail "targeted reannounce case returned failure"
assert_status recovered
[ ! -s "$case_root/systemctl-events" ] ||
  fail "first reannounce success still restarted ModemManager"
grep -Fq "remove $case_root/sys/wwan/wwan0qmi0" "$case_root/udev-events" ||
  fail "targeted reannounce did not remove the QMI control port"
grep -Fq "add $case_root/sys/net/wwan0" "$case_root/udev-events" ||
  fail "targeted reannounce did not add the net port before recovery"

prepare_case restart restart
add_modem_fixture 0
run_recovery || fail "single-restart recovery case returned failure"
assert_status recovered
[ "$(grep -c '^restart ModemManager.service$' "$case_root/systemctl-events")" -eq 1 ] ||
  fail "recovery did not issue exactly one ModemManager restart"

prepare_case multi multi
add_modem_fixture 0
add_modem_fixture 1
printf 'wwan1qmi0\n' > "$case_root/mapped-ports"
run_recovery || fail "multi-modem scoped recovery case returned failure"
assert_status recovered
grep -Fq 'wwan0qmi0' "$case_root/mapped-ports" ||
  fail "stale modem was not mapped"
if grep -Fq 'wwan1' "$case_root/udev-events"; then
  fail "healthy physical modem received a udev recovery event"
fi

prepare_case bounded-failure never
add_modem_fixture 0
if run_recovery; then
  fail "bounded failure case unexpectedly succeeded"
fi
assert_status recovery-failed
[ "$(grep -c '^restart ModemManager.service$' "$case_root/systemctl-events")" -eq 1 ] ||
  fail "bounded failure did not stop after one ModemManager restart"

printf 'PASS: simadmin modem recovery tests\n'
