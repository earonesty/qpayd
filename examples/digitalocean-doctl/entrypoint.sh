#!/bin/sh
set -eu

export BARKD_DATADIR="${BARKD_DATADIR:-/data/barkd}"
mkdir -p "$BARKD_DATADIR"

barkd \
  --datadir "$BARKD_DATADIR" \
  --host 127.0.0.1 \
  --port 3000 \
  -q &
barkd_pid="$!"
sweep_pid=""

cleanup() {
  if [ -n "$sweep_pid" ]; then
    kill "$sweep_pid" 2>/dev/null || true
    wait "$sweep_pid" 2>/dev/null || true
  fi
  kill "$barkd_pid" 2>/dev/null || true
  wait "$barkd_pid" 2>/dev/null || true
}
trap cleanup INT TERM EXIT

for _ in $(seq 1 100); do
  if curl -fsS http://127.0.0.1:3000/ping >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done
curl -fsS http://127.0.0.1:3000/ping >/dev/null

BARKD_TOKEN="$(barkd --datadir "$BARKD_DATADIR" secret show | tr -d '\r')"
export BARKD_AUTH_TOKEN="$BARKD_TOKEN"
export BARKD_SWEEP_AUTH_TOKEN="$BARKD_TOKEN"

if [ "${QPAYD_MIGRATE_ON_BOOT:-}" = "true" ]; then
  qpayd --config /etc/qpayd/qpayd.toml migrate
fi

qpayd --config /etc/qpayd/qpayd.toml sweep &
sweep_pid="$!"

qpayd --config /etc/qpayd/qpayd.toml serve
