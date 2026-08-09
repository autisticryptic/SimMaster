#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd -- "$SCRIPT_DIR/../.." && pwd)
ENV_FILE=${SIMADMIN_ASTERISK_ENV_FILE:-/etc/asterisk/simadmin-lab.env}
TEST_FILTER=services::trunk::driver::tests::live_asterisk_digest_register_and_linphone_call
REGISTRATION_TIMEOUT_SECS=${SIMADMIN_ASTERISK_REGISTRATION_TIMEOUT_SECS:-300}
TEST_PID=

if [[ ! -r $ENV_FILE ]]; then
    echo "Asterisk lab environment is not readable: $ENV_FILE" >&2
    exit 1
fi
if [[ ! -S /run/asterisk/asterisk.ctl ]]; then
    echo "Asterisk lab service is not running" >&2
    exit 1
fi
if [[ ! $REGISTRATION_TIMEOUT_SECS =~ ^[0-9]+$ ]] \
    || (( REGISTRATION_TIMEOUT_SECS < 1 )); then
    echo "SIMADMIN_ASTERISK_REGISTRATION_TIMEOUT_SECS must be a positive integer" >&2
    exit 1
fi

cleanup() {
    if [[ -n $TEST_PID ]] && kill -0 "$TEST_PID" 2>/dev/null; then
        kill "$TEST_PID" 2>/dev/null || true
        wait "$TEST_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# The generated lab file contains shell assignments, not export statements.
# Export them only for this controlled test process.
set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

for variable in \
    SIMADMIN_ASTERISK_TEST_HOST \
    SIMADMIN_ASTERISK_TEST_PORT \
    SIMADMIN_ASTERISK_TEST_LOCAL_PORT \
    SIMADMIN_ASTERISK_TEST_USERNAME \
    SIMADMIN_ASTERISK_TEST_SECRET; do
    if [[ -z ${!variable:-} ]]; then
        echo "Asterisk lab environment is missing $variable" >&2
        exit 1
    fi
done

cd "$REPO_ROOT/backend"
cargo test "$TEST_FILTER" -- --ignored --nocapture &
TEST_PID=$!

registered=false
for ((attempt = 0; attempt < REGISTRATION_TIMEOUT_SECS * 10; attempt++)); do
    if /usr/sbin/asterisk -C /etc/asterisk/asterisk.conf \
        -rx 'pjsip show contacts' 2>/dev/null \
        | grep -q "Contact:  ${SIMADMIN_ASTERISK_TEST_USERNAME}/"; then
        registered=true
        break
    fi
    if ! kill -0 "$TEST_PID" 2>/dev/null; then
        wait "$TEST_PID"
    fi
    sleep 0.1
done

if [[ $registered != true ]]; then
    echo "SimAdmin Trunk did not register with the Asterisk lab" >&2
    exit 1
fi

# Keep the originating Local channel alive through the re-INVITE and DTMF
# assertions. A short Wait would hang up the dialog before those phases run.
/usr/sbin/asterisk -C /etc/asterisk/asterisk.conf \
    -rx 'channel originate Local/41000@simadmin-lab-test application Wait 30'

wait "$TEST_PID"
TEST_PID=
