---
title: Refund Workflows
description: Track refunds without giving qpayd spend authority.
order: 40
---

# Refund workflows

qpayd records refund requests and links them to the original invoice. The actual
refund payment is sent from the wallet or Lightning node that controls the
money.

For on-chain stores, qpayd normally has a watch-only descriptor. For Lightning,
invoice creation should use limited credentials. Keep full spending credentials
in the wallet, node, or private sweep deployment.

Read refund state for an invoice:

```sh
curl -sS https://pay.example.com/v1/stores/main/invoices/$INVOICE_ID/refund-summary \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN"
```

Create an invoice-scoped refund record:

```sh
curl -sS https://pay.example.com/v1/stores/main/invoices/$INVOICE_ID/refunds \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN" \
  -H "Idempotency-Key: refund_order_123" \
  -H "Content-Type: application/json" \
  -d '{
    "amount_sats": 2000,
    "destination": "bc1q...",
    "reason": "overpayment"
  }'
```

Finalize it after the refund payment is sent:

```sh
curl -sS -X POST https://pay.example.com/v1/stores/main/refunds/$REFUND_ID/finalize \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{ "tx_id": "...", "payment_proof": "..." }'
```

Mark it failed if the operator or refund executor cannot complete the payment:

```sh
curl -sS -X POST https://pay.example.com/v1/stores/main/refunds/$REFUND_ID/fail \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{ "failure_reason": "expired lightning invoice" }'
```

Cancel a pending refund:

```sh
curl -sS -X POST https://pay.example.com/v1/stores/main/refunds/$REFUND_ID/cancel \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN"
```

Pending and finalized refunds count against the invoice refundable balance.
Canceled and failed refunds do not. Refund responses include optional
`destination_type`, `idempotency_key`, `payment_proof`, and `failure_reason`
fields.
