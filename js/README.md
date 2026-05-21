# @qpayd/js

Browser helpers for qpayd payment flows.

The package does not need a build step. Import the ESM module and open a modal
from a configured public payment link:

```html
<button id="pay">Pay with Bitcoin</button>
<script type="module">
  import { openPaymentLink } from "./js/src/index.js";

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

Pass a stable `idempotencyKey` for the checkout attempt so browser retries
return the same qpayd invoice.

If invoice creation fails, `openPaymentLink` shows a small error modal by
default. Pass `showErrors: false` to receive the thrown error directly.

Backends can also create invoices with the qpayd admin API and pass the invoice
JSON into `openInvoiceModal({ client, invoice })`.

The modal is customer UX only. Fulfill orders from qpayd signed webhooks.
