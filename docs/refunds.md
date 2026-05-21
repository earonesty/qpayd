---
title: Refund Workflows
description: Track refunds and configure scoped payout authority.
order: 40
---

# Refund workflows

qpayd records refund requests and links them to the original invoice. Refunds can
be finalized manually after payment, or executed by a configured per-store payout
backend.

For on-chain stores, qpayd normally has a watch-only descriptor. For Lightning,
invoice creation should use limited credentials. Keep payout credentials on the
wallet host or a private qpayd deployment that handles sweeps and refunds.

Read refund state for an invoice:

```sh
curl -sS https://pay.example.com/v1/stores/main/invoices/$INVOICE_ID/refund-summary \
  -H "Authorization: Bearer $QPAYD_MAIN_ADMIN_TOKEN"
```

Create an invoice-scoped refund record:

```sh
curl -sS https://pay.example.com/v1/stores/main/invoices/$INVOICE_ID/refunds \
  -H "Authorization: Bearer $QPAYD_MAIN_PAYOUT_TOKEN" \
  -H "Idempotency-Key: refund_order_123" \
  -H "Content-Type: application/json" \
  -d '{
    "amount_sats": 2000,
    "destination": "bc1q...",
    "reason": "overpayment"
  }'
```

When refund execution is enabled, qpayd claims pending refunds, sends them to
the matching payout backend, and finalizes successful payments with the returned
transaction id or payment proof.

Finalize it after the refund payment is sent:

```sh
curl -sS -X POST https://pay.example.com/v1/stores/main/refunds/$REFUND_ID/finalize \
  -H "Authorization: Bearer $QPAYD_MAIN_PAYOUT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{ "tx_id": "...", "payment_proof": "..." }'
```

Mark it failed if the operator or refund executor cannot complete the payment:

```sh
curl -sS -X POST https://pay.example.com/v1/stores/main/refunds/$REFUND_ID/fail \
  -H "Authorization: Bearer $QPAYD_MAIN_PAYOUT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{ "failure_reason": "expired lightning invoice" }'
```

Cancel a pending refund:

```sh
curl -sS -X POST https://pay.example.com/v1/stores/main/refunds/$REFUND_ID/cancel \
  -H "Authorization: Bearer $QPAYD_MAIN_PAYOUT_TOKEN"
```

Pending and finalized refunds count against the invoice refundable balance.
Canceled and failed refunds do not. Refund responses include `approval_status`
plus optional `destination_type`, `idempotency_key`, `payment_proof`, and
`failure_reason` fields.

Use a scoped payout token globally, or override it per store:

```toml
[auth]
payout_token_env = "QPAYD_PAYOUT_TOKEN"

[[stores]]
id = "main"
payout_token_env = "QPAYD_MAIN_PAYOUT_TOKEN"
```

If `payout_token_env` is configured, refund create, finalize, fail, and cancel
requests must use that token. If it is omitted, refund mutations use
`admin_token_env` when configured, otherwise `api_token_env`.

Set `admin_token_can_payout = true` on a store when the admin token should also
be accepted for payout actions. This enables broader admin privilege; keep it
`false` unless the deployment needs one operator token.

```toml
[[stores]]
id = "main"
admin_token_env = "QPAYD_MAIN_ADMIN_TOKEN"
payout_token_env = "QPAYD_MAIN_PAYOUT_TOKEN"
admin_token_can_payout = true
```

Verify the setting with one small refund request against
`POST /v1/stores/main/invoices/$INVOICE_ID/refunds`: use the admin token and
confirm qpayd returns `2xx` when `admin_token_can_payout = true`. Set it back to
`false`, reload qpayd, repeat the same request, and confirm qpayd returns
`401 Unauthorized`.

Approve a refund that crosses `manual_approval_threshold_sats`:

```sh
curl -sS -X POST https://pay.example.com/v1/stores/main/refunds/$REFUND_ID/approve \
  -H "Authorization: Bearer $QPAYD_MAIN_ADMIN_TOKEN"
```

Approved refunds can then be finalized or executed with the payout token.

Verify the approval path with a small amount first:

```sh
curl -sS https://pay.example.com/v1/stores/main/refunds/$REFUND_ID \
  -H "Authorization: Bearer $QPAYD_MAIN_ADMIN_TOKEN"
```

Confirm the response shows `"approval_status":"approved"`, then finalize or run
the refund executor and confirm the refund reaches `"status":"succeeded"`.

Run refund execution once:

```sh
qpayd --config qpayd.toml refunds-once
```

Then query the refund and confirm it is `"status":"processing"`,
`"status":"succeeded"`, or `"status":"failed"`. A successful payout should
include a `tx_id` for Bitcoin refunds or `payment_proof` for Lightning refunds.
A refund left in `"status":"processing"` has started payout execution and should
be inspected before retrying manually.

Run refund execution continuously:

```sh
qpayd --config qpayd.toml refunds
```

For an end-to-end check, create a small invoice, pay it, create a tiny refund,
run the executor, then query the refund and confirm it is `"status":"succeeded"`
or `"status":"failed"` with the expected payout fields.

The normal `serve` command also starts the refund executor when at least one
store has refund execution enabled. Operators that split public receive traffic
from wallet operations should run the `refunds` command on the wallet host and
leave refund execution disabled on the public server.

## Refund Execution Config

qpayd has per-store payout config for refund execution. Lightning payouts share
the same wallet access used for sweeps. Bitcoin payouts use a bitcoind RPC wallet
with spend access.

Tiny sites can run receive, sweeps, and refunds together. Stores that use sweeps
or refund execution should run those commands on the wallet host or a private
server.

```toml
[stores.lightning_payout]
backend = "phoenixd" # or "barkd"
url = "http://127.0.0.1:PORT"
full_api_password_env = "QPAYD_LIGHTNING_REFUND_PASSWORD"

[stores.lightning_payout.refunds]
enabled = true
max_refund_sats = 100000
daily_refund_limit_sats = 500000
manual_approval_threshold_sats = 250000
poll_seconds = 30

[stores.bitcoin_payout]
backend = "bitcoind"
url = "http://127.0.0.1:PORT"
wallet = "refunds"
rpc_auth_env = "QPAYD_BITCOIN_REFUND_RPC_AUTH"

[stores.bitcoin_payout.refunds]
enabled = true
max_refund_sats = 100000
daily_refund_limit_sats = 500000
manual_approval_threshold_sats = 250000
poll_seconds = 30
```

`max_refund_sats` applies to one refund. `daily_refund_limit_sats` applies to
successful refunds finalized by qpayd during the current UTC day.
`manual_approval_threshold_sats` makes larger refunds wait for an admin approval
before the payout token can execute or finalize them.

Backend-specific wallet configuration is covered in
[Lightning backends](./lightning-backends.md).
