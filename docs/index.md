---
title: Docs
description: Operator guides and API workflows for qpayd.
order: 0
---

# qpayd docs

qpayd is a small payment daemon, but a real deployment still has moving parts:
wallet descriptors, Lightning backends, webhooks, database backups, and public
browser flows.

Start here:

- [Wallet descriptors](./wallet-descriptors.html) for linking a Bitcoin wallet safely.
- [Public payment links](./public-payment-links.html) for browser checkout without API tokens.
- [Webhooks and replay](./webhooks-and-replay.html) for fulfillment.
- [Admin portal](./admin-portal.html) for invoice review and refund operations.
- [Refund workflows](./refunds.html) for receive-only refund tracking.

Deployment guides:

- [Fly.io Quickstart with barkd](./fly-sidecar-barkd.html)
- [DigitalOcean Quickstart with doctl](./digitalocean-doctl.html)
- [Move from SQLite to Postgres](./sqlite-to-postgres.html)
- [Split Lightning funds from the payment server](./security-sidecar-split.html)
