# qpayd

`qpayd` is a small self-hosted Bitcoin and Lightning payment daemon. It creates
invoices, locks fiat prices to BTC using configured rate sources, tracks payment
state, and emits signed webhooks.

Project page: <https://earonesty.github.io/qpayd/>

Docs: <https://earonesty.github.io/qpayd/docs/>

## Install

```sh
cargo install --path .
```

Or run it with Docker:

```sh
docker build -t qpayd .
docker run --rm -p 8080:8080 \
  -v "$PWD/qpayd.toml:/etc/qpayd/qpayd.toml:ro" \
  -v "$PWD/data:/data" \
  -e QPAYD_MAIN_API_TOKEN=change-me \
  qpayd serve --config /etc/qpayd/qpayd.toml
```

## Configure

Start from the example config:

```sh
cp qpayd.example.toml qpayd.toml
qpayd --config qpayd.toml check
qpayd --config qpayd.toml migrate
qpayd --config qpayd.toml serve
```

qpayd supports:

- watch-only Bitcoin wallet descriptors
- Electrum fallback/rotation
- Phoenixd or barkd Lightning backends
- SQLite or Postgres
- multiple stores
- public payment links
- signed webhooks with retry and replay
- receive-only refund tracking
- optional browser admin portal

## Minimal API Use

Create an invoice:

```sh
curl -sS http://127.0.0.1:8080/v1/stores/main/invoices \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"amount":"10.00","currency":"USD"}'
```

Read an invoice:

```sh
curl -sS http://127.0.0.1:8080/v1/stores/main/invoices/$INVOICE_ID \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN"
```

Use `Idempotency-Key` on invoice creation when retrying a cart/order request.

## Browser Packages

Customer checkout:

```js
import { openPaymentLink } from "@qpayd/checkout";
```

Merchant admin panel:

```js
import { mountQPaydAdmin } from "@qpayd/admin";
```

Both packages are published through npm trusted publishing from GitHub Actions.

## Docs

Operator guides live in [`docs/`](docs/) and are rendered to the public site:

- [Wallet descriptors](docs/wallet-descriptors.md)
- [Public payment links](docs/public-payment-links.md)
- [Webhooks and replay](docs/webhooks-and-replay.md)
- [Refund workflows](docs/refunds.md)
- [Admin portal](docs/admin-portal.md)
- [Move from SQLite to Postgres](docs/sqlite-to-postgres.md)
- [Split a public app from a private payment sidecar](docs/security-sidecar-split.md)
- [Lightning backends](docs/lightning-backends.md)

## Commands

```sh
qpayd serve --config qpayd.toml
qpayd sync-once --config qpayd.toml
qpayd sweep --config qpayd.toml
qpayd sweep-once --config qpayd.toml
qpayd check --config qpayd.toml
qpayd migrate --config qpayd.toml
qpayd generate-token
qpayd generate-secret
```

## Development

```sh
cargo fmt -- --check
cargo test
cargo clippy --all-targets -- -D warnings
npm --prefix js run check
npm --prefix site run build
./scripts/smoke-barkd.sh
```

## License

MIT
