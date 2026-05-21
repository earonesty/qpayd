---
title: Admin Portal
description: Browser admin panel, token login, and pinned asset bootstrap.
order: 50
---

# Admin portal

The admin portal is release-pinned because Bitcoin operators care about exact
bytes and checksums. Use the generated release docs for copy-paste config:

- [Latest admin-portal.html](https://github.com/earonesty/qpayd/releases/latest/download/admin-portal.html)
- [Latest admin-portal.md](https://github.com/earonesty/qpayd/releases/latest/download/admin-portal.md)

`@qpayd/admin` is a browser-only UI that talks directly to the qpayd admin API.
Configure the qpayd base URL when mounting it. Add `storeId` to pin the panel to
one store, or omit it and let qpayd route the signed-in token to its stores.

```html
<main id="qpayd-admin"></main>
<script type="module">
  import { mountQPaydAdmin } from "@qpayd/admin";

  mountQPaydAdmin("#qpayd-admin", {
    baseUrl: "https://pay.example.com"
  });
</script>
```

Set `admin_allowed_origins` for the store when hosting the admin panel on a
different browser origin than the API:

```toml
[[stores]]
id = "main"
admin_allowed_origins = ["https://admin.example.com"]
admin_token_env = "QPAYD_MAIN_ADMIN_TOKEN"
payout_token_env = "QPAYD_MAIN_PAYOUT_TOKEN"
```

If `admin_token_env` is configured, admin API requests must use that token. If
it is omitted, admin API requests use the store `api_token_env`.

If `payout_token_env` is configured, payout and refund actions use that token.
Set `admin_token_can_payout = true` on a store when one admin token should also
be able to run payout actions.

qpayd can serve a minimal `/admin` bootstrap page that loads a pinned admin
asset. The daemon does not bundle the admin UI. Use the generated release doc
above for the exact `asset_source` and `asset_integrity` values.

```toml
[server.admin]
enabled = true
asset_source = "https://cdn.jsdelivr.net/npm/@qpayd/admin@VERSION/src/index.js"
asset_integrity = "sha384-..."
```

Set `store_id = "main"` to pin the hosted `/admin` page to one store. If
`store_id` is omitted, the page does not publish store ids in the HTML. After
login, qpayd returns the stores and scopes available to the submitted token.

Remote admin assets require `asset_integrity` and must use `https`.
