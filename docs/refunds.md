---
title: Refund Workflows
description: Track refunds without giving qpayd spend authority.
order: 40
---

# Refund workflows

qpayd does not spend customer funds. On-chain stores use watch-only
descriptors, and Lightning invoice creation should use limited backend
credentials. Refunds are tracked as case-management records linked to the
original invoice.

Read refund state for an invoice:

```sh
curl -sS https://pay.example.com/v1/stores/main/invoices/$INVOICE_ID/refund-summary \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN"
```

Create an invoice-scoped refund record:

```sh
curl -sS https://pay.example.com/v1/stores/main/invoices/$INVOICE_ID/refunds \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "amount_sats": 2000,
    "destination": "bc1q...",
    "reason": "overpayment"
  }'
```

Finalize it after the wallet or Lightning node that controls funds completes
the refund:

```sh
curl -sS -X POST https://pay.example.com/v1/stores/main/refunds/$REFUND_ID/finalize \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{ "tx_id": "..." }'
```

Cancel a pending refund:

```sh
curl -sS -X POST https://pay.example.com/v1/stores/main/refunds/$REFUND_ID/cancel \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN"
```

Pending and finalized refunds count against the invoice refundable balance.
Canceled refunds do not.

