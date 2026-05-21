---
title: Split a Public App from a Private Payment Sidecar
description: Keep browser flows public and payment credentials private.
order: 70
---

# Split a public app from a private payment sidecar

The useful boundary is simple:

- Public pages can create configured payment-link invoices.
- Store API tokens stay in private server-side code or the admin panel.
- qpayd owns invoice state and signed webhook delivery.
- Lightning sweep credentials stay separate from invoice credentials.

Static pages should use public payment links. Server-side apps that need custom
amounts can call the admin invoice API from a private environment, then pass the
invoice JSON to `@qpayd/checkout`.

For Lightning, keep receive credentials and sweep credentials separate. qpayd
can run invoice creation and sweeping in one deployment for convenience, or run
the sweep command on a different box near the Lightning backend.

Webhooks should be verified before fulfillment. Use event replay when your app
was down or returned a non-2xx response.

