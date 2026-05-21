---
title: Public Payment Links
description: Browser-safe invoice creation for fixed payment links.
order: 20
---

# Public payment links

Public payment links let browser code create fresh invoices without exposing a
store API token. A payment link is constrained by config: the browser can create
only that configured amount, currency, and metadata.

```toml
[[stores.payment_links]]
id = "donate-10"
amount = "10.00"
currency = "USD"
public_allowed_origins = ["https://example.com"]
metadata = { kind = "donation", site = "example.com" }
```

If no `public_allowed_origins` are configured, public browser calls are allowed
from any site. Set `public_allowed_origins` on `[server]`, `[[stores]]`, or a
specific `[[stores.payment_links]]` to restrict browser calls by `Origin`.
Server-side calls without an `Origin` header are still accepted.

Create an invoice:

```sh
curl -sS -X POST \
  -H "Idempotency-Key: donation-click-123" \
  https://pay.example.com/v1/public/stores/main/payment-links/donate-10/invoices
```

The response includes `bitcoin`, `lightning`, and QR URLs when those payment
methods are configured. Browser code can poll:

```sh
curl -sS https://pay.example.com/v1/public/stores/main/invoices/$INVOICE_ID
```

Fulfill orders from signed webhooks, not browser status alone.

## Checkout package

`@qpayd/checkout` provides a browser modal and public invoice polling:

```html
<button id="pay">Pay with Bitcoin</button>
<script type="module">
  import { openPaymentLink } from "https://cdn.jsdelivr.net/npm/@qpayd/checkout@0.4.0/src/index.js";

  document.querySelector("#pay").addEventListener("click", () => {
    openPaymentLink({
      baseUrl: "https://pay.example.com",
      storeId: "main",
      paymentLinkId: "donate-10",
      idempotencyKey: "cart-or-order-id"
    });
  });
</script>
```

