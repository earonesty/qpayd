#!/bin/sh
set -eu

if [ -z "${PHOENIXD_LIMITED_PASSWORD:-}" ]; then
  echo "PHOENIXD_LIMITED_PASSWORD is required" >&2
  exit 1
fi
if [ -z "${PHOENIXD_FULL_PASSWORD:-}" ]; then
  echo "PHOENIXD_FULL_PASSWORD is required" >&2
  exit 1
fi

export HOME=/data/phoenixd
mkdir -p "$HOME"

{
  printf 'I understand\n'
  printf 'I understand\n'
  printf '\n'
} | phoenixd \
    --http-bind-ip=127.0.0.1 \
    --http-bind-port=9740 \
    --http-password="$PHOENIXD_FULL_PASSWORD" \
    --http-password-limited-access="$PHOENIXD_LIMITED_PASSWORD" \
    --silent &
phoenixd_pid="$!"
sweep_pid=""

cleanup() {
  if [ -n "$sweep_pid" ]; then
    kill "$sweep_pid" 2>/dev/null || true
    wait "$sweep_pid" 2>/dev/null || true
  fi
  kill "$phoenixd_pid" 2>/dev/null || true
  wait "$phoenixd_pid" 2>/dev/null || true
}
trap cleanup INT TERM EXIT

ready=0
for _ in $(seq 1 60); do
  if phoenix-cli --http-password="$PHOENIXD_LIMITED_PASSWORD" getinfo >/dev/null 2>&1 \
    && phoenix-cli --http-password="$PHOENIXD_FULL_PASSWORD" getbalance >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 1
done
if [ "$ready" -ne 1 ]; then
  echo "phoenixd did not become ready" >&2
  exit 1
fi

qpayd --config /etc/qpayd/qpayd.toml sweep &
sweep_pid="$!"

qpayd --config /etc/qpayd/qpayd.toml serve
