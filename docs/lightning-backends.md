---
title: Lightning Backends
description: Phoenixd and barkd configuration notes.
order: 100
---

# Lightning backends

qpayd supports Phoenixd and barkd for Lightning invoice creation and
reconciliation.

Phoenixd:

```toml
[stores.lightning]
backend = "phoenixd"
url = "http://127.0.0.1:9740"
api_password_env = "PHOENIXD_LIMITED_PASSWORD"
```

barkd:

```toml
[stores.lightning]
backend = "barkd"
url = "http://127.0.0.1:3000"
api_password_env = "BARKD_AUTH_TOKEN"
```

Sweeping uses separate config:

```toml
[stores.lightning_sweep]
backend = "barkd"
url = "http://127.0.0.1:3000"
full_api_password_env = "BARKD_SWEEP_AUTH_TOKEN"
destination_descriptor_env = "QPAYD_MAIN_TREASURY_DESCRIPTOR"
min_balance_sats = 100000
target_balance_sats = 25000
interval_seconds = 3600
```

Run the sweep service separately when you want a stronger security split:

```sh
qpayd --config qpayd.toml sweep
```

Hot-wallet refund execution uses per-store config blocks and full wallet
credentials. Keep those credentials with the wallet host when splitting public
receive from wallet operations:

```toml
[[stores.hot_wallets]]
id = "lightning-refunds"
enabled = true
refund_execution_enabled = false
backend = "barkd"
url = "http://127.0.0.1:3000"
full_api_password_env = "BARKD_FULL_AUTH_TOKEN"
max_refund_sats = 100000
daily_refund_limit_sats = 500000
allowed_refund_destination_types = ["lightning_invoice", "lnurl"]
```
