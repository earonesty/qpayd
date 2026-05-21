---
title: Admin Portal
description: Release-pinned qpayd admin portal setup with Subresource Integrity.
---

# qpayd admin portal

This file is generated for `qpayd` {{QPAYD_VERSION}}.

qpayd can serve a minimal `/admin` bootstrap page. The daemon does not bundle
the admin UI. Instead, the bootstrap page loads the `@qpayd/admin` browser
package from a pinned asset URL and verifies the bytes with browser Subresource
Integrity.

## Configure qpayd

Use this release-pinned config:

```toml
[server.admin]
enabled = true
store_id = "main"
asset_source = "{{ADMIN_ASSET_SOURCE}}"
asset_integrity = "{{ADMIN_ASSET_INTEGRITY}}"

[[stores]]
id = "main"
admin_allowed_origins = ["https://pay.example.com"]
admin_token_env = "QPAYD_MAIN_ADMIN_TOKEN"
```

Change `store_id`, `admin_allowed_origins`, and `admin_token_env` for your
deployment. The origin must be the browser origin that serves the admin page. If
qpayd serves `/admin` from the same origin as the API, use that origin. If
`admin_token_env` is omitted, admin API requests use the store `api_token_env`.

## Security model

- qpayd serves only a bootstrap page at `/admin`.
- The admin UI is loaded from the pinned `asset_source`.
- `asset_integrity` makes the browser reject changed bytes.
- The admin token is entered in the browser and sent directly to qpayd.
- qpayd still enforces `Authorization: Bearer ...` on admin API calls.
- Remote admin assets must use `https`.
- Do not use `@latest` in `asset_source`.

## Verify integrity manually

```sh
curl -fsSL "{{ADMIN_ASSET_SOURCE}}" \
  | openssl dgst -sha384 -binary \
  | openssl base64 -A
```

The output should match the part after `sha384-`:

```text
{{ADMIN_ASSET_INTEGRITY_VALUE}}
```

## Asset details

```text
qpayd version: {{QPAYD_VERSION}}
@qpayd/admin version: {{ADMIN_VERSION}}
asset source: {{ADMIN_ASSET_SOURCE}}
asset integrity: {{ADMIN_ASSET_INTEGRITY}}
```
