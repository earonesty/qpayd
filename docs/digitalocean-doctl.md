---
title: DigitalOcean Quickstart with doctl
description: Download a complete DigitalOcean Droplet qpayd + barkd deployment.
order: 90
---

# DigitalOcean quickstart with doctl

Download, create a Droplet, copy files, start Docker Compose.

```sh
DROPLET="qpayd"
REGION="nyc3"
SIZE="s-1vcpu-1gb"
DOMAIN="pay.example.com"
ACME_EMAIL="you@example.com"
SSH_KEY_ID="$(doctl compute ssh-key list --format ID --no-header | head -n1)"

mkdir qpayd-doctl
cd qpayd-doctl

base="https://raw.githubusercontent.com/earonesty/qpayd/master/examples/digitalocean-doctl"
curl -fsSLO "$base/Dockerfile"
curl -fsSLO "$base/entrypoint.sh"
curl -fsSLO "$base/compose.yaml"
curl -fsSLO "$base/Caddyfile"
curl -fsSLO "$base/cloud-init.yaml"
curl -fsSLO "$base/qpayd.toml"
chmod +x entrypoint.sh

perl -pi -e "s/YOUR_DOMAIN/$DOMAIN/g" qpayd.toml
```

Create `.env`:

```sh
export QPAYD_MAIN_API_TOKEN="$(openssl rand -hex 32)"
export QPAYD_MAIN_DESCRIPTOR='wpkh([00000000/84h/0h/0h]xpub.../0/*)'
export QPAYD_MAIN_TREASURY_DESCRIPTOR="$QPAYD_MAIN_DESCRIPTOR"

cat >.env <<EOF
DOMAIN=$DOMAIN
ACME_EMAIL=$ACME_EMAIL
QPAYD_MAIN_API_TOKEN=$QPAYD_MAIN_API_TOKEN
QPAYD_MAIN_DESCRIPTOR=$QPAYD_MAIN_DESCRIPTOR
QPAYD_MAIN_TREASURY_DESCRIPTOR=$QPAYD_MAIN_TREASURY_DESCRIPTOR
EOF
```

Create the Droplet:

```sh
doctl compute droplet create "$DROPLET" \
  --region "$REGION" \
  --image ubuntu-24-04-x64 \
  --size "$SIZE" \
  --ssh-keys "$SSH_KEY_ID" \
  --user-data-file cloud-init.yaml \
  --wait

IP="$(doctl compute droplet get "$DROPLET" --format PublicIPv4 --no-header)"
echo "$IP"
```

Point DNS at `$IP`.

Copy and start:

```sh
ssh root@"$IP" 'mkdir -p /opt/qpayd'
scp Dockerfile entrypoint.sh compose.yaml Caddyfile qpayd.toml .env root@"$IP":/opt/qpayd/
ssh root@"$IP" 'cd /opt/qpayd && docker compose up -d --build'
```

Open admin:

```sh
open "https://$DOMAIN/admin"
```

Use `QPAYD_MAIN_API_TOKEN` to log in.

Create a test invoice:

```sh
curl -sS "https://$DOMAIN/v1/stores/main/invoices" \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"amount":"1.00","currency":"USD"}'
```

Logs:

```sh
ssh root@"$IP" 'cd /opt/qpayd && docker compose logs -f'
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

## compose.yaml

```yaml
services:
  qpayd:
    build: .
    restart: unless-stopped
    env_file:
      - .env
    environment:
      QPAYD_MIGRATE_ON_BOOT: "true"
      RUST_LOG: "qpayd=info,tower_http=info"
    volumes:
      - qpayd_data:/data
    expose:
      - "8080"

  caddy:
    image: caddy:2
    restart: unless-stopped
    depends_on:
      - qpayd
    env_file:
      - .env
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile:ro
      - caddy_data:/data
      - caddy_config:/config

volumes:
  qpayd_data:
  caddy_data:
  caddy_config:
```

## Caddyfile

```text
{$DOMAIN} {
  reverse_proxy qpayd:8080
}
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
public_allowed_origins = ["https://YOUR_DOMAIN"]
admin_allowed_origins = ["https://YOUR_DOMAIN"]

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
public_allowed_origins = ["https://YOUR_DOMAIN"]
metadata = { kind = "donation", source = "digitalocean-doctl" }
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

## cloud-init.yaml

```yaml
#cloud-config
package_update: true
packages:
  - ca-certificates
  - curl
  - docker.io
  - docker-compose-plugin
runcmd:
  - systemctl enable --now docker
  - mkdir -p /opt/qpayd
```
