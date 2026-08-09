#!/usr/bin/env bash
set -euo pipefail

if [[ ${EUID:-$(id -u)} -ne 0 ]]; then
    echo "run as root" >&2
    exit 1
fi

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
ASTERISK_ETC=${ASTERISK_ETC:-/etc/asterisk}
ENV_FILE=${SIMADMIN_ASTERISK_ENV_FILE:-/etc/asterisk/simadmin-lab.env}
BIND_IP=${SIMADMIN_ASTERISK_BIND_IP:-0.0.0.0}
BIND_PORT=${SIMADMIN_ASTERISK_BIND_PORT:-8060}

if [[ ! -x /usr/sbin/asterisk ]]; then
    echo "asterisk is not installed" >&2
    exit 1
fi
if [[ ! $BIND_PORT =~ ^[0-9]+$ ]] || (( BIND_PORT < 1 || BIND_PORT > 65535 )); then
    echo "invalid SIMADMIN_ASTERISK_BIND_PORT" >&2
    exit 1
fi

umask 077
if [[ -f $ENV_FILE ]]; then
    # shellcheck disable=SC1090
    source "$ENV_FILE"
fi

random_secret() {
    od -An -N24 -tx1 /dev/urandom | tr -d ' \n'
}

SIMADMIN_TRUNK_SECRET=${SIMADMIN_TRUNK_SECRET:-${SIMADMIN_ASTERISK_TEST_SECRET:-$(random_secret)}}
LINPHONE_SECRET=${LINPHONE_SECRET:-${SIMADMIN_LINPHONE_SECRET:-$(random_secret)}}
WINDOWS_HOST_IP=${SIMADMIN_WINDOWS_HOST_IP:-$(ip route show default | awk 'NR == 1 { print $3 }')}

for value in "$SIMADMIN_TRUNK_SECRET" "$LINPHONE_SECRET"; do
    if [[ ! $value =~ ^[A-Za-z0-9._~-]+$ ]]; then
        echo "lab secrets may contain only A-Z, a-z, 0-9, dot, underscore, tilde, and dash" >&2
        exit 1
    fi
done
if [[ ! $WINDOWS_HOST_IP =~ ^[0-9a-fA-F:.]+$ ]]; then
    echo "invalid Windows host address" >&2
    exit 1
fi

if ! getent group asterisk >/dev/null 2>&1; then
    groupadd --system asterisk
fi
if ! id asterisk >/dev/null 2>&1; then
    useradd --system \
        --gid asterisk \
        --home-dir /var/lib/asterisk \
        --no-create-home \
        --shell /usr/sbin/nologin \
        asterisk
fi
for directory in \
    /var/cache/asterisk \
    /var/lib/asterisk \
    /var/log/asterisk \
    /var/spool/asterisk; do
    install -d -o asterisk -g asterisk -m 0755 "$directory"
    chown -R asterisk:asterisk "$directory"
done

cat >"$ENV_FILE" <<EOF
SIMADMIN_ASTERISK_TEST_HOST=$(hostname -I | awk '{ print $1 }')
SIMADMIN_ASTERISK_TEST_PORT=$BIND_PORT
SIMADMIN_ASTERISK_TEST_LOCAL_PORT=5062
SIMADMIN_ASTERISK_TEST_USERNAME=41000
SIMADMIN_ASTERISK_TEST_SECRET=$SIMADMIN_TRUNK_SECRET
SIMADMIN_LINPHONE_USERNAME=6108
SIMADMIN_LINPHONE_SECRET=$LINPHONE_SECRET
SIMADMIN_WINDOWS_HOST_IP=$WINDOWS_HOST_IP
EOF
chmod 0600 "$ENV_FILE"

sed \
    -e "s|@BIND_IP@|$BIND_IP|g" \
    -e "s|@BIND_PORT@|$BIND_PORT|g" \
    -e "s|@SIMADMIN_TRUNK_SECRET@|$SIMADMIN_TRUNK_SECRET|g" \
    -e "s|@LINPHONE_SECRET@|$LINPHONE_SECRET|g" \
    -e "s|@WINDOWS_HOST_IP@|$WINDOWS_HOST_IP|g" \
    "$SCRIPT_DIR/pjsip_simadmin_lab.conf.in" \
    >"$ASTERISK_ETC/pjsip_simadmin_lab.conf"
chown root:asterisk "$ASTERISK_ETC/pjsip_simadmin_lab.conf"
chmod 0640 "$ASTERISK_ETC/pjsip_simadmin_lab.conf"
install -m 0644 "$SCRIPT_DIR/extensions_simadmin_lab.conf" \
    "$ASTERISK_ETC/extensions_simadmin_lab.conf"

ensure_include() {
    local file=$1
    local include=$2
    if ! grep -Fqx "$include" "$file"; then
        printf '\n%s\n' "$include" >>"$file"
    fi
}

ensure_include "$ASTERISK_ETC/pjsip.conf" '#include pjsip_simadmin_lab.conf'
ensure_include "$ASTERISK_ETC/extensions.conf" '#include extensions_simadmin_lab.conf'

install -m 0644 "$SCRIPT_DIR/asterisk-simadmin-lab.service" \
    /etc/systemd/system/asterisk-simadmin-lab.service
systemctl daemon-reload
systemctl enable asterisk-simadmin-lab.service
systemctl restart asterisk-simadmin-lab.service

ready=false
for _ in {1..30}; do
    version=$(/usr/sbin/asterisk \
        -C /etc/asterisk/asterisk.conf \
        -rx 'core show version' 2>&1 || true)
    endpoint=$(/usr/sbin/asterisk \
        -C /etc/asterisk/asterisk.conf \
        -rx 'pjsip show endpoint 41000' 2>&1 || true)
    if [[ $version == Asterisk\ * ]] && grep -q 'Endpoint:  41000' <<<"$endpoint"; then
        ready=true
        break
    fi
    sleep 0.2
done
if [[ $ready != true ]]; then
    systemctl status asterisk-simadmin-lab.service --no-pager >&2 || true
    echo "Asterisk lab endpoint did not become ready" >&2
    exit 1
fi

/usr/sbin/asterisk -C /etc/asterisk/asterisk.conf -rx 'core show version'
/usr/sbin/asterisk -C /etc/asterisk/asterisk.conf -rx 'pjsip show endpoint 41000'
/usr/sbin/asterisk -C /etc/asterisk/asterisk.conf -rx 'pjsip show endpoint 6108'
echo "lab credentials: $ENV_FILE"
