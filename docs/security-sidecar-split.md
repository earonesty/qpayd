---
title: Split Lightning Funds from the Payment Server
description: Move Phoenixd and sweeping off the public qpayd machine.
order: 70
---

# Split Lightning funds from the payment server

Goal: keep full Phoenixd credentials off the public qpayd server.

The public server keeps the limited Phoenixd password for invoice creation. The
private machine keeps Phoenixd state, the full Phoenixd password, and the qpayd
sweep process.

## 1. Back up Phoenixd

Stop Phoenixd on the current box:

```sh
systemctl stop phoenixd
```

Back up the Phoenixd data directory:

```sh
tar -C /var/lib -czf phoenixd-backup.tgz phoenixd
sha256sum phoenixd-backup.tgz > phoenixd-backup.tgz.sha256
```

Copy both files to the private machine.

## 2. Restore Phoenixd on the private machine

```sh
mkdir -p /var/lib/phoenixd
tar -C /var/lib -xzf phoenixd-backup.tgz
```

Start Phoenixd bound to a private interface:

```sh
phoenixd \
  --http-bind-ip=10.0.0.10 \
  --http-bind-port=9740 \
  --http-password="$PHOENIXD_FULL_PASSWORD" \
  --http-password-limited-access="$PHOENIXD_LIMITED_PASSWORD"
```

Firewall it so only the qpayd machine can reach `10.0.0.10:9740`.

## 3. Point qpayd invoice creation at private Phoenixd

On the public qpayd server:

```toml
[stores.lightning]
backend = "phoenixd"
url = "http://10.0.0.10:9740"
api_password_env = "PHOENIXD_LIMITED_PASSWORD"
```

Only set the limited Phoenixd password on the public qpayd server:

```sh
PHOENIXD_LIMITED_PASSWORD=...
```

Do not set `PHOENIXD_FULL_PASSWORD` on the public qpayd server.

## 4. Turn off sweep on the public qpayd server

Remove or stop:

```sh
qpayd --config qpayd.toml sweep
```

Remove payout config and full Phoenixd secrets from the public qpayd deployment:

```toml
# remove this from the public server
[stores.lightning_payout]
```

## 5. Run sweep on the Phoenixd machine

On the private Phoenixd machine, keep the same store config plus
`lightning_payout.sweep`:

```toml
[stores.lightning_payout]
backend = "phoenixd"
url = "http://10.0.0.10:9740"
full_api_password_env = "PHOENIXD_FULL_PASSWORD"

[stores.lightning_payout.sweep]
destination_descriptor_env = "QPAYD_MAIN_TREASURY_DESCRIPTOR"
min_balance_sats = 100000
target_balance_sats = 25000
interval_seconds = 3600
```

Run:

```sh
qpayd --config qpayd.toml sweep
```

After this split, the public payment server handles invoice creation,
reconciliation, admin API, and webhooks. The private Phoenixd machine handles
wallet state and sweeping.
