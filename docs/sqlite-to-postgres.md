---
title: Move from SQLite to Postgres
description: Operational notes for moving qpayd state to Postgres.
order: 60
---

# Move from SQLite to Postgres

SQLite is a good default for a single small qpayd node. Move to Postgres when
you need managed backups, shared operational access, or a database independent
from the payment VM.

qpayd supports:

```toml
[database]
url = "sqlite://qpayd.db"
```

and:

```toml
[database]
url = "postgres://postgres:postgres@localhost:5432/qpayd"
```

Postgres tables are prefixed with `qpayd_` so qpayd can use a shared database
without claiming generic table names.

## Migration checklist

1. Stop invoice creation.
2. Back up the SQLite database.
3. Provision Postgres.
4. Set the Postgres database URL.
5. Run `qpayd --config qpayd.toml migrate`.
6. Start qpayd and create a small test invoice.
7. Verify invoice reads, webhooks, and address index continuity.

Schema migrations are sequential. Fresh installs run every migration in order;
existing installs only run migrations that have not been recorded yet.

