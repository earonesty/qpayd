# qpayd

`qpayd` is a small self-hosted Bitcoin and Lightning payment daemon. It creates
invoices, locks fiat prices to BTC using configured rate sources, tracks payment
state, and emits signed webhooks.

Project page: <https://earonesty.github.io/qpayd/>

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
  -e QPAYD_Q32_API_TOKEN=change-me \
  qpayd serve --config /etc/qpayd/qpayd.toml
```

## Configure

Create `qpayd.toml`:

```toml
[server]
listen = "0.0.0.0:8080"
onchain_poll_seconds = 30
public_allowed_origins = ["https://example.com"]

[database]
url = "sqlite:///data/qpayd.db"

[pricing]
kraken_url = "https://api.kraken.com/0/public/Ticker"
stale_after_seconds = 60

[[stores]]
id = "main"
name = "Main Store"
api_token_env = "QPAYD_MAIN_API_TOKEN"
webhook_url = "https://example.com/webhooks/qpayd"
webhook_secret_env = "QPAYD_MAIN_WEBHOOK_SECRET"
invoice_expiry_minutes = 15
min_confirmations = 1

[stores.onchain]
network = "bitcoin"
descriptor_env = "QPAYD_MAIN_DESCRIPTOR"
electrum_servers = ["ssl://electrum.blockstream.info:50002"]

[stores.lightning]
backend = "phoenixd"
url = "http://127.0.0.1:9740"
api_password_env = "PHOENIXD_LIMITED_PASSWORD"

[stores.lightning_sweep]
backend = "phoenixd"
url = "http://127.0.0.1:9740"
full_api_password_env = "PHOENIXD_FULL_PASSWORD"
destination_descriptor_env = "QPAYD_MAIN_TREASURY_DESCRIPTOR"
min_balance_sats = 100000
target_balance_sats = 25000
interval_seconds = 3600

[[stores.payment_links]]
id = "donate-10"
amount = "10.00"
currency = "USD"
public_allowed_origins = ["https://example.com"]
metadata = { kind = "donation", source = "static-site" }
```

With Phoenixd configured, qpayd creates BOLT11 invoices and polls Phoenixd for
incoming payment status during reconciliation:

```toml
[stores.lightning]
backend = "phoenixd"
url = "http://127.0.0.1:9740"
api_password_env = "PHOENIXD_LIMITED_PASSWORD"
```

Barkd can be used instead of Phoenixd:

```toml
[stores.lightning]
backend = "barkd"
url = "http://127.0.0.1:3000"
api_password_env = "BARKD_AUTH_TOKEN"
```

Set `BARKD_AUTH_TOKEN` to the bearer token shown by:

```sh
barkd --datadir /var/lib/barkd secret show
```

Run the sweep service separately from the payment daemon:

```sh
qpayd --config qpayd.toml sweep
```

With `lightning_sweep` configured, the sweep service periodically checks the
Lightning balance and sends funds above `min_balance_sats` to the configured
treasury descriptor, leaving `target_balance_sats` on the Lightning backend.
Use a limited Phoenixd password for invoice creation and a separate full-access
password only for sweeping.

Run a manual sweep check with:

```sh
qpayd --config qpayd.toml sweep-once
```

Then set the token:

```sh
export QPAYD_MAIN_API_TOKEN="$(openssl rand -hex 32)"
export QPAYD_MAIN_WEBHOOK_SECRET="$(openssl rand -hex 32)"
export QPAYD_MAIN_DESCRIPTOR="wpkh([00000000/84h/0h/0h]xpub.../0/*)"
export QPAYD_MAIN_TREASURY_DESCRIPTOR="wpkh([00000000/84h/0h/0h]xpub.../0/*)"
export PHOENIXD_LIMITED_PASSWORD="$(openssl rand -hex 32)"
export PHOENIXD_FULL_PASSWORD="$(openssl rand -hex 32)"
# If using barkd instead of Phoenixd:
export BARKD_AUTH_TOKEN="$(barkd --datadir /var/lib/barkd secret show)"
```

## Link A Bitcoin Wallet

`qpayd` needs a watch-only wallet descriptor. The descriptor lets `qpayd`
generate one receive address per invoice and watch those addresses for payment.
It does not let `qpayd` spend coins.

Use a normal singlesig Bitcoin wallet for the first store. Export the account
extended public key from the wallet, then wrap it as a descriptor.

Native SegWit wallets usually use:

```text
wpkh(YOUR_XPUB/0/*)
```

Taproot wallets use:

```text
tr(YOUR_XPUB/0/*)
```

If your wallet shows a master fingerprint and account path, include them:

```text
wpkh([FINGERPRINT/84h/0h/0h]YOUR_XPUB/0/*)
```

Set the descriptor as an environment variable:

```sh
export QPAYD_MAIN_DESCRIPTOR='wpkh(xpub.../0/*)'
```

Or as a deployment secret:

```sh
flyctl secrets set QPAYD_MAIN_DESCRIPTOR='wpkh(xpub.../0/*)' --app your-app
```

### Blockstream Apps

Use a singlesig Bitcoin account.

1. Open the wallet account.
2. Open the account or wallet settings.
3. Find watch-only or extended public key export.
4. Copy the account xpub.
5. Use `wpkh(YOUR_XPUB/0/*)` for a native SegWit account.

Do not use a Green 2FA/multisig account for the first setup. Use singlesig so
the exported xpub maps directly to invoice receive addresses.

### Sparrow

1. Open the wallet.
2. Open Settings.
3. Copy the wallet descriptor, or copy the account xpub and script type.
4. Use the descriptor directly if it ends with `/0/*`.

For a native SegWit singlesig wallet, the descriptor should look like:

```text
wpkh([abcd1234/84h/0h/0h]xpub.../0/*)
```

### Test The Wallet Link

Before taking real payments, create a small invoice and pay it from another
wallet you control. This confirms three things:

- qpayd can generate receive addresses from your wallet export.
- qpayd can see payments to those addresses.
- your store receives a `settled` webhook when the payment confirms.

```sh
curl -sS http://127.0.0.1:8080/v1/stores/main/invoices \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"amount":"1.00","currency":"USD"}'
```

Check the response:

```json
{
  "onchain_address_index": 0,
  "onchain_address": "bc1q..."
}
```

Send a small test payment to `onchain_address`. After the transaction confirms,
run reconciliation:

```sh
qpayd --config qpayd.toml sync-once
```

Then read the invoice:

```sh
curl -sS http://127.0.0.1:8080/v1/stores/main/invoices/$INVOICE_ID \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN"
```

The invoice should move to `settled`. If it does not, stop and fix the wallet
descriptor before using qpayd for payments.

Check and migrate:

```sh
qpayd --config qpayd.toml check
qpayd --config qpayd.toml migrate
```

Run:

```sh
qpayd --config qpayd.toml serve
```

To run migrations automatically before the server starts, set:

```sh
export QPAYD_MIGRATE_ON_BOOT=true
```

Reconcile on-chain payments:

```sh
qpayd --config qpayd.toml sync-once
```

## Create An Invoice

```sh
curl -sS https://pay.example.com/v1/stores/main/invoices \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "amount": "25.00",
    "currency": "USD",
    "metadata": {
      "site": "example.com",
      "order_id": "ord_123"
    }
  }'
```

Response:

```json
{
  "id": "b8e2b1fd-1ef3-4b1c-bf1c-5a2d60cccb53",
  "store_id": "main",
  "status": "new",
  "amount": "25.00",
  "currency": "USD",
  "btc_amount_sats": 25000,
  "onchain_address": "bc1p...",
  "onchain_address_index": 0,
  "onchain_script_pubkey": "5120...",
  "lightning_bolt11": "lnbc...",
  "lightning_payment_hash": "...",
  "bitcoin": {
    "address": "bc1p...",
    "uri": "bitcoin:bc1p...?amount=0.00025000",
    "qr_svg_url": "/v1/public/stores/main/invoices/b8e2b1fd-1ef3-4b1c-bf1c-5a2d60cccb53/qr/bitcoin.svg"
  },
  "lightning": {
    "bolt11": "lnbc...",
    "uri": "lightning:lnbc...",
    "qr_svg_url": "/v1/public/stores/main/invoices/b8e2b1fd-1ef3-4b1c-bf1c-5a2d60cccb53/qr/lightning.svg"
  },
  "min_confirmations": 1,
  "rate_source": "kraken",
  "rate": "100000",
  "metadata": {
    "site": "example.com",
    "order_id": "ord_123"
  },
  "expires_at": "2026-05-20T18:00:00Z",
  "created_at": "2026-05-20T17:45:00Z",
  "updated_at": "2026-05-20T17:45:00Z"
}
```

## Public Payment Links

Public payment links let browser code create fresh invoices without exposing a
store API token. A payment link is constrained by config: the browser can create
only that configured amount, currency, and metadata.

```toml
[[stores.payment_links]]
id = "donate-10"
amount = "10.00"
currency = "USD"
public_allowed_origins = ["https://example.com"]
metadata = { kind = "donation", site = "example.com" }
```

If no `public_allowed_origins` are configured, public browser calls are allowed
from any site. Set `public_allowed_origins` on `[server]`, `[[stores]]`, or a
specific `[[stores.payment_links]]` to restrict browser calls by `Origin`.
Server-side calls without an `Origin` header are still accepted.

Create an invoice from browser code:

```sh
curl -sS -X POST \
  https://pay.example.com/v1/public/stores/main/payment-links/donate-10/invoices
```

Response:

```json
{
  "id": "b8e2b1fd-1ef3-4b1c-bf1c-5a2d60cccb53",
  "store_id": "main",
  "status": "new",
  "amount": "10.00",
  "currency": "USD",
  "btc_amount_sats": 10000,
  "bitcoin": {
    "address": "bc1q...",
    "uri": "bitcoin:bc1q...?amount=0.00010000",
    "qr_svg_url": "/v1/public/stores/main/invoices/b8e2b1fd-1ef3-4b1c-bf1c-5a2d60cccb53/qr/bitcoin.svg"
  },
  "lightning": {
    "bolt11": "lnbc...",
    "uri": "lightning:lnbc...",
    "qr_svg_url": "/v1/public/stores/main/invoices/b8e2b1fd-1ef3-4b1c-bf1c-5a2d60cccb53/qr/lightning.svg"
  },
  "min_confirmations": 1,
  "expires_at": "2026-05-20T18:00:00Z"
}
```

The browser can show the QR code from `qr_svg_url`, copy `bitcoin.address` or
`lightning.bolt11`, open `bitcoin.uri` or `lightning.uri`, and poll the public
invoice endpoint for status:

```sh
curl -sS \
  https://pay.example.com/v1/public/stores/main/invoices/$INVOICE_ID
```

The browser status is only customer UX. Fulfill orders from signed webhooks.

## JavaScript Modal

The `js/` folder contains a small browser client that makes payment links feel
like a hosted checkout while keeping the UI in your site:

```html
<button id="pay">Pay with Bitcoin</button>
<script type="module">
  import { openPaymentLink } from "/js/src/index.js";

  document.querySelector("#pay").addEventListener("click", () => {
    openPaymentLink({
      baseUrl: "https://pay.example.com",
      storeId: "main",
      paymentLinkId: "donate-10"
    });
  });
</script>
```

## Read An Invoice

```sh
curl -sS https://pay.example.com/v1/stores/main/invoices/$INVOICE_ID \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN"
```

## Payment Statuses

Fulfill orders when an invoice reaches `settled`.

Use `payment_detected` as a pending on-chain state: qpayd has seen enough
unconfirmed sats, but the payment is not confirmed yet. Use `partially_paid`
for customer support or retry flows. Use `expired` to release inventory. Use
`paid_late` for manual handling after the invoice window has closed.

## Webhooks

When `webhook_url` is configured, qpayd records each event in the configured
database and delivers it from a retry queue. Invoice creation does not depend on
the receiver being online.

Events currently emitted:

```text
invoice.created
invoice.payment_detected
invoice.partially_paid
invoice.settled
invoice.expired
invoice.paid_late
```

Webhook requests are signed with:

```text
QPayd-Signature: t=<unix_timestamp>,v1=<hmac_sha256>
```

The signature payload is:

```text
<unix_timestamp>.<raw_request_body>
```

Verify the `v1` value with the store webhook secret.

### List Events

```sh
curl -sS "https://pay.example.com/v1/stores/main/events?limit=50" \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN"
```

### Replay An Event

```sh
curl -sS -X POST https://pay.example.com/v1/stores/main/events/$EVENT_ID/replay \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN"
```

Replay queues a fresh webhook delivery for the stored event.

## Commands

```sh
qpayd --config qpayd.toml check
qpayd --config qpayd.toml migrate
qpayd --config qpayd.toml serve
qpayd --config qpayd.toml sync-once
```

## Database

SQLite and Postgres database URLs are supported:

```toml
[database]
url = "sqlite:///data/qpayd.db"
```

```toml
[database]
url = "postgres://postgres:postgres@localhost/qpayd"
```

Postgres tables are prefixed with `qpayd_` so qpayd can use a shared database
without creating generic table names.

Migrations are sequential and recorded in the database. A new database runs all
missing migrations in order. A database that already has migration `1` recorded
will only run future migration `2`, `3`, and so on.

SQLite records migrations in `schema_migrations`. Postgres records migrations
in `qpayd_schema_migrations`.

## Releases

GitHub Releases are published from `v*` tags after CI passes. The release
contains a Linux x86_64 binary tarball and `SHA256SUMS`.

```sh
git tag v0.1.0
git push origin v0.1.0
```

Use pull requests for changes so generated release notes have useful history.

## License

MIT
