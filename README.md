# qpayd

`qpayd` is a small self-hosted Bitcoin and Lightning payment daemon. It creates
invoices, locks fiat prices to BTC using configured rate sources, serves checkout
pages, tracks payment state, and emits signed webhooks.

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
public_url = "https://pay.example.com"
onchain_poll_seconds = 30

[database]
url = "sqlite:///data/qpayd.db"

[pricing]
kraken_url = "https://api.kraken.com/0/public/Ticker"
stale_after_seconds = 60

[[stores]]
id = "main"
name = "Main Store"
api_token_env = "QPAYD_MAIN_API_TOKEN"
invoice_expiry_minutes = 15
min_confirmations = 1

[stores.onchain]
network = "bitcoin"
descriptor_env = "QPAYD_MAIN_DESCRIPTOR"
electrum_servers = ["ssl://electrum.blockstream.info:50002"]

[stores.lightning]
backend = "phoenixd"
url = "http://127.0.0.1:9740"
api_password_env = "PHOENIXD_PASSWORD"
```

Then set the token:

```sh
export QPAYD_MAIN_API_TOKEN="$(openssl rand -hex 32)"
export QPAYD_MAIN_DESCRIPTOR="wpkh([00000000/84h/0h/0h]xpub.../0/*)"
```

Check and migrate:

```sh
qpayd --config qpayd.toml check
qpayd --config qpayd.toml migrate
```

Run:

```sh
qpayd --config qpayd.toml serve
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
  "rate_source": "kraken",
  "rate": "100000",
  "metadata": {
    "site": "example.com",
    "order_id": "ord_123"
  },
  "checkout_url": "https://pay.example.com/i/main/b8e2b1fd-1ef3-4b1c-bf1c-5a2d60cccb53",
  "expires_at": "2026-05-20T18:00:00Z",
  "created_at": "2026-05-20T17:45:00Z",
  "updated_at": "2026-05-20T17:45:00Z"
}
```

Open `checkout_url` to show the payment page.

## Read An Invoice

```sh
curl -sS https://pay.example.com/v1/stores/main/invoices/$INVOICE_ID \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN"
```

## Webhook Signatures

Webhook requests are signed with:

```text
QPayd-Signature: t=<unix_timestamp>,v1=<hmac_sha256>
```

The signature payload is:

```text
<unix_timestamp>.<raw_request_body>
```

Verify the `v1` value with the store webhook secret.

## Commands

```sh
qpayd --config qpayd.toml check
qpayd --config qpayd.toml migrate
qpayd --config qpayd.toml serve
qpayd --config qpayd.toml sync-once
```

## License

MIT
