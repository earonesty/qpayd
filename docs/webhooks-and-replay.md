---
title: Webhooks and Replay
description: Signed payment events, retry queue, and replay operations.
order: 30
---

# Webhooks and replay

qpayd records events in the configured database and delivers them from a retry
queue. Invoice creation does not depend on the receiver being online.

## Configure the webhook secret

Generate a secret:

```sh
qpayd generate-secret
```

Set it in the qpayd environment:

```sh
export QPAYD_MAIN_WEBHOOK_SECRET="paste-generated-secret"
```

Configure the store:

```toml
[[stores]]
id = "main"
webhook_url = "https://example.com/webhooks/qpayd"
webhook_secret_env = "QPAYD_MAIN_WEBHOOK_SECRET"
```

On Fly:

```sh
fly secrets set --app "$APP" QPAYD_MAIN_WEBHOOK_SECRET="paste-generated-secret"
```

With Docker Compose, put it in `.env`:

```sh
QPAYD_MAIN_WEBHOOK_SECRET=paste-generated-secret
```

Use a different webhook secret for each store.

## Events

Webhook event types:

```text
invoice.created
invoice.payment_detected
invoice.partially_paid
invoice.settled
invoice.expired
invoice.paid_late
refund.created
refund.finalized
refund.canceled
```

Webhook requests are signed with:

- `qpayd-event-id`
- `qpayd-event-type`
- `qpayd-signature`

The signature is HMAC-SHA256 over the raw request body using the store webhook
secret. Verify it before fulfillment.

## Replay

Replay an event:

```sh
curl -sS -X POST https://pay.example.com/v1/stores/main/events/$EVENT_ID/replay \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN"
```

List recent events:

```sh
curl -sS https://pay.example.com/v1/stores/main/events \
  -H "Authorization: Bearer $QPAYD_MAIN_API_TOKEN"
```
