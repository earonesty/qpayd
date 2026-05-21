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

Set `admin_allowed_origins` for the store when hosting the admin panel on a
different browser origin than the API:

```toml
[[stores]]
id = "main"
admin_allowed_origins = ["https://admin.example.com"]
```

qpayd can serve a minimal `/admin` bootstrap page that loads a pinned admin
asset. The daemon does not bundle the admin UI. Use the generated release doc
above for the exact `asset_source` and `asset_integrity` values.

```toml
[server.admin]
enabled = true
store_id = "main"
asset_source = "https://cdn.jsdelivr.net/npm/@qpayd/admin@VERSION/src/index.js"
asset_integrity = "sha384-..."
```

Remote admin assets require `asset_integrity` and must use `https`.
