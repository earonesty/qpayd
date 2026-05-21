---
title: Fly.io Quickstart with barkd
description: Download a complete Fly.io qpayd + barkd sidecar deployment.
order: 80
---

# Fly.io quickstart with barkd

Download, set secrets, deploy.

```sh
APP="your-qpayd-app"
REGION="lax"

mkdir qpayd-fly-barkd
cd qpayd-fly-barkd

base="https://raw.githubusercontent.com/earonesty/qpayd/master/examples/fly-barkd"
curl -fsSLO "$base/Dockerfile"
curl -fsSLO "$base/entrypoint.sh"
curl -fsSLO "$base/fly.toml"
curl -fsSLO "$base/qpayd.toml"
chmod +x entrypoint.sh

perl -pi -e "s/YOUR_APP/$APP/g" fly.toml qpayd.toml
```

Create the app and volume:

```sh
fly apps create "$APP"
fly volumes create qpayd_data --app "$APP" --region "$REGION" --size 1
```

Set secrets:

```sh
export QPAYD_MAIN_API_TOKEN="$(openssl rand -hex 32)"
export QPAYD_MAIN_DESCRIPTOR='wpkh([00000000/84h/0h/0h]xpub.../0/*)'
export QPAYD_MAIN_TREASURY_DESCRIPTOR="$QPAYD_MAIN_DESCRIPTOR"

fly secrets set --app "$APP" \
  QPAYD_MAIN_API_TOKEN="$QPAYD_MAIN_API_TOKEN" \
  QPAYD_MAIN_DESCRIPTOR="$QPAYD_MAIN_DESCRIPTOR" \
  QPAYD_MAIN_TREASURY_DESCRIPTOR="$QPAYD_MAIN_TREASURY_DESCRIPTOR"
```

Deploy:

```sh
fly deploy --app "$APP"
```

Open admin:

```sh
open "https://$APP.fly.dev/admin"
```

Use `QPAYD_MAIN_API_TOKEN` to log in.

Create a test invoice:

```sh
curl -sS "https://$APP.fly.dev/v1/stores/main/invoices" \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"amount":"1.00","currency":"USD"}'
```

Watch logs:

```sh
fly logs --app "$APP"
```

## Dockerfile

```dockerfile
FROM rust:1.95-bookworm AS qpayd
WORKDIR /src
ARG QPAYD_REF=master
RUN git clone https://github.com/earonesty/qpayd.git . \
  && git checkout "$QPAYD_REF" \
  && cargo build --release --locked

FROM debian:bookworm-slim AS barkd
ARG BARKD_VERSION=0.1.4
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl \
  && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL "https://gitlab.com/ark-bitcoin/bark/-/releases/bark-${BARKD_VERSION}/downloads/barkd-${BARKD_VERSION}-linux-x86_64" \
  -o /usr/local/bin/barkd \
  && chmod +x /usr/local/bin/barkd

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl \
  && rm -rf /var/lib/apt/lists/* \
  && mkdir -p /data
COPY --from=qpayd /src/target/release/qpayd /usr/local/bin/qpayd
COPY --from=barkd /usr/local/bin/barkd /usr/local/bin/barkd
COPY qpayd.toml /etc/qpayd/qpayd.toml
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh
EXPOSE 8080
CMD ["entrypoint.sh"]
```

## fly.toml

```toml
app = "YOUR_APP"
primary_region = "lax"

[build]
  dockerfile = "Dockerfile"

[env]
  QPAYD_MIGRATE_ON_BOOT = "true"
  RUST_LOG = "qpayd=info,tower_http=info"

[http_service]
  internal_port = 8080
  force_https = true
  auto_stop_machines = "stop"
  auto_start_machines = true
  min_machines_running = 0
  processes = ["app"]

  [http_service.concurrency]
    type = "requests"
    soft_limit = 20
    hard_limit = 50

[[mounts]]
  source = "qpayd_data"
  destination = "/data"

[[vm]]
  memory = "256mb"
  cpu_kind = "shared"
  cpus = 1
```

## qpayd.toml

```toml
[server]
listen = "0.0.0.0:8080"
onchain_poll_seconds = 30

[server.admin]
enabled = true
store_id = "main"
asset_source = "https://cdn.jsdelivr.net/npm/@qpayd/admin@0.4.0/src/index.js"
asset_integrity = "sha384-UoC2NIFvzDHT33cpI06LUEsLaLvgk0omHv0u+NBuC2jZc2BQGoZQlalmYQWDt7Hu"

[database]
url = "sqlite:///data/qpayd.db"

[pricing]
kraken_url = "https://api.kraken.com/0/public/Ticker"
stale_after_seconds = 60

[[stores]]
id = "main"
name = "Main Store"
api_token_env = "QPAYD_MAIN_API_TOKEN"
invoice_expiry_minutes = 15
min_confirmations = 1
public_allowed_origins = ["https://YOUR_APP.fly.dev"]
admin_allowed_origins = ["https://YOUR_APP.fly.dev"]

[stores.onchain]
network = "bitcoin"
descriptor_env = "QPAYD_MAIN_DESCRIPTOR"
electrum_servers = ["ssl://electrum.blockstream.info:50002"]

[stores.lightning]
backend = "barkd"
url = "http://127.0.0.1:3000"
api_password_env = "BARKD_AUTH_TOKEN"

[stores.lightning_payout]
backend = "barkd"
url = "http://127.0.0.1:3000"
full_api_password_env = "BARKD_SWEEP_AUTH_TOKEN"

[stores.lightning_payout.sweep]
destination_descriptor_env = "QPAYD_MAIN_TREASURY_DESCRIPTOR"
min_balance_sats = 100000
target_balance_sats = 25000
interval_seconds = 3600

[[stores.payment_links]]
id = "donate-10"
amount = "10.00"
currency = "USD"
public_allowed_origins = ["https://YOUR_APP.fly.dev"]
metadata = { kind = "donation", source = "fly-barkd" }
```

## entrypoint.sh

```sh
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
```
