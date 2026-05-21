#!/usr/bin/env bash
set -euo pipefail

BARKD_VERSION="${BARKD_VERSION:-0.1.4}"
BARKD_URL="${BARKD_URL:-https://gitlab.com/ark-bitcoin/bark/-/releases/bark-${BARKD_VERSION}/downloads/barkd-${BARKD_VERSION}-linux-x86_64}"

tmp="$(mktemp -d)"
container=""
cleanup() {
  if [ -n "$container" ]; then
    docker rm -f "$container" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT

curl -fsSL "$BARKD_URL" -o "$tmp/barkd"
chmod +x "$tmp/barkd"

cat >"$tmp/Dockerfile" <<'DOCKERFILE'
FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*
COPY barkd /usr/local/bin/barkd
RUN chmod +x /usr/local/bin/barkd && mkdir -p /data
EXPOSE 3000
ENTRYPOINT ["barkd"]
DOCKERFILE

image="qpayd-barkd-smoke:${BARKD_VERSION}"
docker build -q -t "$image" "$tmp" >/dev/null

container="qpayd-barkd-smoke-$$"
docker run -d --rm \
  --name "$container" \
  -p 127.0.0.1::3000 \
  "$image" \
  --datadir /data \
  --host 0.0.0.0 \
  --port 3000 \
  -q >/dev/null

port="$(docker inspect -f '{{(index (index .NetworkSettings.Ports "3000/tcp") 0).HostPort}}' "$container")"
base_url="http://127.0.0.1:${port}"

for _ in $(seq 1 100); do
  if curl -fsS "${base_url}/ping" >/dev/null; then
    break
  fi
  sleep 0.2
done

curl -fsS "${base_url}/ping" | grep -qx "pong"
token="$(docker exec "$container" barkd --datadir /data secret show | tr -d '\r')"
test -n "$token"
curl -fsS -H "Authorization: Bearer ${token}" "${base_url}/api/v1/wallet" \
  | grep -q '"fingerprint"'

echo "barkd smoke ok (${BARKD_VERSION})"
