# qpayd browser packages

qpayd publishes separate browser packages for customer checkout and merchant
admin workflows.

## @qpayd/checkout

Browser helpers for qpayd payment flows. The package does not need a build step.
Import the ESM module and open a modal from a configured public payment link:

```html
<button id="pay">Pay with Bitcoin</button>
<script type="module">
  import { openPaymentLink } from "@qpayd/checkout";

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

### Checkout modal styling

The checkout modal injects its default stylesheet as `#qpayd-modal-styles`.
Merchant CSS loaded after the modal opens can override the supported selectors
below. Pass `className` to scope merchant overrides to a specific checkout
instance:

```js
openInvoiceModal({
  client,
  invoice,
  className: "merchant-checkout"
});
```

Supported checkout styling classes:

- `.qpayd-modal-root`
- `.qpayd-backdrop`
- `.qpayd-modal`
- `.qpayd-head`
- `.qpayd-summary`
- `.qpayd-tabs`
- `.qpayd-panel`
- `.qpayd-value`
- `.qpayd-actions`
- `.qpayd-note`
- `.qpayd-error`
- `.qpayd-error-panel`

Supported state and behavior attributes for scoped styling:

- `[data-qpayd-close]`
- `[data-qpayd-copy]`
- `[data-qpayd-error]`
- `[data-qpayd-expiry]`
- `[data-qpayd-fiat]`
- `[data-qpayd-method]`
- `[data-qpayd-panel]`
- `[data-qpayd-qr]`
- `[data-qpayd-sats]`
- `[data-qpayd-status]`
- `[data-qpayd-uri]`
- `[data-qpayd-value]`
- `[data-status]`

Other modal markup is internal and may change between releases.

The modal is customer UX only. Fulfill orders from qpayd signed webhooks.

## @qpayd/admin

The admin panel is a browser-only UI that talks directly to the qpayd admin API.
Configure the qpayd base URL and store id when mounting it; the login form asks
for the store API token.

```html
<main id="qpayd-admin"></main>
<script type="module">
  import { mountQPaydAdmin } from "@qpayd/admin";

  mountQPaydAdmin("#qpayd-admin", {
    baseUrl: "https://pay.example.com",
    storeId: "main"
  });
</script>
```

Set `admin_allowed_origins` for the store in qpayd config when hosting the
admin panel on a different browser origin than the API:

```toml
[[stores]]
id = "main"
admin_allowed_origins = ["https://admin.example.com"]
```

The panel lists invoices, opens invoice details, creates invoice-scoped refund
records, scans QR codes for refund destinations, cancels pending refunds, and
finalizes refunds after an operator sends funds from the wallet or Lightning
node that controls them.
