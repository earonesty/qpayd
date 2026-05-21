---
title: Fly.io Quickstart with barkd
description: Deploy qpayd with a barkd sidecar shape on Fly.io.
order: 80
---

# Fly.io quickstart with barkd

This guide is the target deployment shape for fast warm starts and a small
payment service. Keep app code out of the payment box, mount durable storage for
qpayd state, and keep Lightning backend credentials explicit.

High-level flow:

1. Create a qpayd app.
2. Attach persistent storage or configure Postgres.
3. Configure one or more stores.
4. Deploy barkd as the Lightning backend or sidecar.
5. Set qpayd secrets.
6. Create a test invoice.
7. Pay it and verify `invoice.settled`.

Minimum qpayd checks:

```sh
qpayd --config qpayd.toml check
qpayd --config qpayd.toml migrate
qpayd --config qpayd.toml serve
```

Use `admin_allowed_origins` if you expose the admin portal from a browser
origin. Use signed webhooks for fulfillment.

