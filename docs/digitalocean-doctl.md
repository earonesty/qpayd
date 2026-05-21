---
title: DigitalOcean Quickstart with doctl
description: DigitalOcean deployment notes for qpayd operators.
order: 90
---

# DigitalOcean quickstart with doctl

DigitalOcean can run qpayd several ways: a droplet, App Platform, or a
container pushed to a registry. Pick the boring shape first: one daemon, one
database, explicit secrets, and tested backups.

Operator checklist:

1. Create the server or app.
2. Create a volume or managed Postgres database.
3. Set qpayd secrets with the platform secret manager.
4. Run `qpayd check` and `qpayd migrate`.
5. Start `qpayd serve`.
6. Create a small test invoice.
7. Verify webhooks and backups.

Keep `qpayd.toml` in source control only if it contains no secrets. Put API
tokens, webhook secrets, wallet descriptors, and Lightning credentials in the
deployment secret store.

