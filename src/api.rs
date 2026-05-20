use std::sync::Arc;

use anyhow::Context;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use bitcoin::secp256k1::Secp256k1;
use chrono::{Duration, Utc};
use miniscript::{Descriptor, DescriptorPublicKey};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    config::Config,
    events,
    invoice::{Invoice, InvoiceStatus},
    pricing::RateSource,
    storage::Store,
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Arc<dyn Store>,
    pub pricing: Arc<dyn RateSource>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/healthz", get(healthz))
        .route(
            "/p/{store_id}/{payment_link_id}",
            get(payment_link).post(create_payment_link_invoice),
        )
        .route("/v1/stores/{store_id}/invoices", post(create_invoice))
        .route(
            "/v1/stores/{store_id}/invoices/{invoice_id}",
            get(get_invoice),
        )
        .route("/v1/stores/{store_id}/events", get(list_events))
        .route("/v1/stores/{store_id}/events/{event_id}", get(get_event))
        .route(
            "/v1/stores/{store_id}/events/{event_id}/replay",
            post(replay_event),
        )
        .route("/i/{store_id}/{invoice_id}", get(checkout))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>qpayd</title>
  <style>
    :root { color-scheme: light dark; font-family: system-ui, sans-serif; }
    body { margin: 0; min-height: 100vh; display: grid; place-items: center; }
    main { width: min(32rem, calc(100vw - 2rem)); }
  </style>
</head>
<body>
  <main>
    <h1>qpayd</h1>
    <p>Bitcoin and Lightning payment daemon.</p>
  </main>
</body>
</html>"#,
    )
}

async fn healthz() -> &'static str {
    "ok"
}

async fn create_invoice(
    State(state): State<AppState>,
    Path(store_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateInvoiceRequest>,
) -> Result<Json<InvoiceResponse>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize(store_cfg, &headers)?;

    let invoice = build_invoice(
        &state,
        store_cfg,
        store_id,
        request.amount,
        request.currency,
        request.metadata.unwrap_or_else(|| serde_json::json!({})),
    )
    .await?;

    Ok(Json(InvoiceResponse::new(
        invoice,
        store_cfg.confirmations(),
    )))
}

async fn create_payment_link_invoice(
    State(state): State<AppState>,
    Path((store_id, payment_link_id)): Path<(String, String)>,
) -> Result<Redirect, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    let payment_link = store_cfg
        .payment_link(&payment_link_id)
        .ok_or(ApiError::not_found("payment link not found"))?;

    let invoice = build_invoice(
        &state,
        store_cfg,
        store_id,
        payment_link.amount,
        payment_link.currency.clone(),
        payment_link.metadata.clone(),
    )
    .await?;

    Ok(Redirect::to(&invoice.checkout_url))
}

async fn payment_link(
    State(state): State<AppState>,
    Path((store_id, payment_link_id)): Path<(String, String)>,
) -> Result<Html<String>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    let payment_link = store_cfg
        .payment_link(&payment_link_id)
        .ok_or(ApiError::not_found("payment link not found"))?;
    let action = format!(
        "/p/{}/{}",
        escape_html(&store_id),
        escape_html(&payment_link_id)
    );

    Ok(Html(format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Pay {amount} {currency}</title>
  <style>
    :root {{ color-scheme: light dark; font-family: system-ui, sans-serif; }}
    body {{ margin: 0; min-height: 100vh; display: grid; place-items: center; }}
    main {{ width: min(32rem, calc(100vw - 2rem)); }}
    button {{ font: inherit; padding: 0.75rem 1rem; cursor: pointer; }}
  </style>
</head>
<body>
  <main>
    <h1>{amount} {currency}</h1>
    <form method="post" action="{action}">
      <button type="submit">Pay with Bitcoin</button>
    </form>
  </main>
</body>
</html>"#,
        amount = payment_link.amount,
        currency = escape_html(&payment_link.currency.to_uppercase()),
        action = action,
    )))
}

async fn build_invoice(
    state: &AppState,
    store_cfg: &crate::config::StoreConfig,
    store_id: String,
    amount: Decimal,
    currency: String,
    metadata: serde_json::Value,
) -> anyhow::Result<Invoice> {
    let currency = currency.to_uppercase();
    let rate = state.pricing.btc_rate(&currency).await?;
    let btc_amount_sats = sats_for(amount, rate.value)?;
    let (onchain_address, onchain_address_index, onchain_script_pubkey) = match &store_cfg.onchain {
        Some(onchain) => {
            let index = state.store.reserve_address_index(&store_id).await?;
            let descriptor = onchain
                .descriptor()?
                .parse::<Descriptor<DescriptorPublicKey>>()
                .context("invalid on-chain descriptor")?;
            let secp = Secp256k1::verification_only();
            let derived = descriptor
                .derived_descriptor(&secp, index)
                .context("failed to derive on-chain descriptor")?;
            let network = onchain
                .network
                .parse::<bitcoin::Network>()
                .context("invalid bitcoin network")?;
            let address = derived
                .address(network)
                .context("descriptor does not produce an address")?
                .to_string();
            let script_pubkey = hex::encode(derived.script_pubkey().as_bytes());
            (Some(address), Some(index), Some(script_pubkey))
        }
        None => (None, None, None),
    };
    let lightning_invoice = match &store_cfg.lightning {
        Some(lightning) => Some(
            crate::lightning::create_invoice(lightning, btc_amount_sats, "qpayd invoice").await?,
        ),
        None => None,
    };

    let now = Utc::now();
    let id = Uuid::new_v4();
    let checkout_url = format!(
        "{}/i/{}/{}",
        state.config.server.public_url.trim_end_matches('/'),
        store_id,
        id
    );
    let invoice = Invoice {
        id,
        store_id,
        status: InvoiceStatus::New,
        amount,
        currency,
        btc_amount_sats,
        onchain_address,
        onchain_address_index,
        onchain_script_pubkey,
        lightning_bolt11: lightning_invoice
            .as_ref()
            .map(|invoice| invoice.bolt11.clone()),
        lightning_payment_hash: lightning_invoice.and_then(|invoice| invoice.payment_hash),
        rate_source: rate.source,
        rate: rate.value,
        metadata,
        checkout_url,
        expires_at: now + Duration::minutes(store_cfg.expiry_minutes() as i64),
        created_at: now,
        updated_at: now,
    };

    let event = events::invoice_created_event(&invoice, now);
    state
        .store
        .insert_invoice(&invoice, &event, store_cfg.webhook_url.as_deref())
        .await?;

    Ok(invoice)
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn list_events(
    State(state): State<AppState>,
    Path(store_id): Path<String>,
    Query(query): Query<ListEventsQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<events::EventEnvelope>>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize(store_cfg, &headers)?;

    Ok(Json(
        state
            .store
            .events(&store_id, query.limit.unwrap_or(50))
            .await?,
    ))
}

async fn get_event(
    State(state): State<AppState>,
    Path((store_id, event_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<events::EventEnvelope>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize(store_cfg, &headers)?;

    let event = state
        .store
        .event(&store_id, &event_id)
        .await?
        .ok_or(ApiError::not_found("event not found"))?;
    Ok(Json(event))
}

async fn replay_event(
    State(state): State<AppState>,
    Path((store_id, event_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize(store_cfg, &headers)?;
    let url = store_cfg
        .webhook_url
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("store has no webhook_url"))?;
    state
        .store
        .event(&store_id, &event_id)
        .await?
        .ok_or(ApiError::not_found("event not found"))?;
    state
        .store
        .enqueue_webhook_delivery(&event_id, &store_id, url, Utc::now())
        .await?;
    Ok(StatusCode::ACCEPTED)
}

async fn get_invoice(
    State(state): State<AppState>,
    Path((store_id, invoice_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<InvoiceResponse>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize(store_cfg, &headers)?;

    let invoice = state
        .store
        .invoice(&store_id, invoice_id)
        .await?
        .ok_or(ApiError::not_found("invoice not found"))?;

    Ok(Json(InvoiceResponse::new(
        invoice,
        store_cfg.confirmations(),
    )))
}

async fn checkout(
    State(state): State<AppState>,
    Path((store_id, invoice_id)): Path<(String, Uuid)>,
) -> Result<Html<String>, ApiError> {
    let invoice = state
        .store
        .invoice(&store_id, invoice_id)
        .await?
        .ok_or(ApiError::not_found("invoice not found"))?;
    let sats = invoice.btc_amount_sats;
    let btc = Decimal::from(sats) / Decimal::from(100_000_000u64);
    let address = invoice
        .onchain_address
        .as_deref()
        .unwrap_or("no on-chain address configured");
    let lightning = invoice.lightning_bolt11.as_deref().unwrap_or("");

    Ok(Html(format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Pay invoice {id}</title>
  <style>
    :root {{ color-scheme: light dark; font-family: system-ui, sans-serif; }}
    body {{ margin: 0; min-height: 100vh; display: grid; place-items: center; }}
    main {{ width: min(38rem, calc(100vw - 2rem)); }}
    code {{ overflow-wrap: anywhere; }}
  </style>
</head>
<body>
  <main>
    <h1>{amount} {currency}</h1>
    <p>Status: <strong>{status}</strong></p>
    <p>BTC amount: <strong>{btc}</strong> BTC ({sats} sats)</p>
    <p>Address: <code>{address}</code></p>
    <p>Lightning: <code>{lightning}</code></p>
    <p>Invoice: <code>{id}</code></p>
    <p>Expires: {expires}</p>
  </main>
</body>
</html>"#,
        id = invoice.id,
        amount = invoice.amount,
        currency = invoice.currency,
        status = invoice.status.as_str(),
        btc = btc,
        sats = sats,
        address = address,
        lightning = lightning,
        expires = invoice.expires_at.to_rfc3339(),
    )))
}

fn authorize(store: &crate::config::StoreConfig, headers: &HeaderMap) -> Result<(), ApiError> {
    let expected = store.api_token()?;
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return Err(ApiError::unauthorized());
    };
    let Ok(value) = value.to_str() else {
        return Err(ApiError::unauthorized());
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err(ApiError::unauthorized());
    };

    if subtle::ConstantTimeEq::ct_eq(token.as_bytes(), expected.as_bytes()).into() {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}

fn sats_for(amount: Decimal, btc_quote_rate: Decimal) -> anyhow::Result<u64> {
    if amount <= Decimal::ZERO {
        anyhow::bail!("amount must be positive");
    }
    if btc_quote_rate <= Decimal::ZERO {
        anyhow::bail!("rate must be positive");
    }

    let sats = (amount / btc_quote_rate) * Decimal::from(100_000_000u64);
    sats.round_dp(0)
        .to_u64()
        .ok_or_else(|| anyhow::anyhow!("amount is outside supported range"))
}

#[derive(Debug, Deserialize)]
pub struct CreateInvoiceRequest {
    pub amount: Decimal,
    pub currency: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ListEventsQuery {
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct InvoiceResponse {
    pub id: Uuid,
    pub store_id: String,
    pub status: InvoiceStatus,
    pub amount: Decimal,
    pub currency: String,
    pub btc_amount_sats: u64,
    pub onchain_address: Option<String>,
    pub onchain_address_index: Option<u32>,
    pub onchain_script_pubkey: Option<String>,
    pub lightning_bolt11: Option<String>,
    pub lightning_payment_hash: Option<String>,
    pub min_confirmations: u32,
    pub rate_source: String,
    pub rate: Decimal,
    pub metadata: serde_json::Value,
    pub checkout_url: String,
    pub expires_at: chrono::DateTime<Utc>,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
}

impl InvoiceResponse {
    fn new(invoice: Invoice, min_confirmations: u32) -> Self {
        Self {
            id: invoice.id,
            store_id: invoice.store_id,
            status: invoice.status,
            amount: invoice.amount,
            currency: invoice.currency,
            btc_amount_sats: invoice.btc_amount_sats,
            onchain_address: invoice.onchain_address,
            onchain_address_index: invoice.onchain_address_index,
            onchain_script_pubkey: invoice.onchain_script_pubkey,
            lightning_bolt11: invoice.lightning_bolt11,
            lightning_payment_hash: invoice.lightning_payment_hash,
            min_confirmations,
            rate_source: invoice.rate_source,
            rate: invoice.rate,
            metadata: invoice.metadata,
            checkout_url: invoice.checkout_url,
            expires_at: invoice.expires_at,
            created_at: invoice.created_at,
            updated_at: invoice.updated_at,
        }
    }
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: "unauthorized".to_string(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(serde_json::json!({ "error": self.message }));
        (self.status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request, StatusCode},
    };
    use rust_decimal::Decimal;
    use tower::ServiceExt;

    use super::{AppState, router, sats_for};
    use crate::{
        config::Config,
        pricing::{Rate, RateSource},
        storage::{SqliteStore, Store},
    };

    #[test]
    fn converts_fiat_to_sats() {
        let sats = sats_for(Decimal::from(25), Decimal::from(100_000)).unwrap();
        assert_eq!(sats, 25_000);
    }

    #[tokio::test]
    async fn public_payment_link_redirects_to_checkout() {
        let app = test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/p/main/donate-10")
                    .method(Method::POST)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(location.starts_with("https://pay.example.com/i/main/"));
    }

    #[tokio::test]
    async fn public_payment_link_get_renders_payment_button_without_creating_invoice() {
        let app = test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/p/main/donate-10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        assert!(body.contains("Pay with Bitcoin"));
        assert!(body.contains("method=\"post\""));
        assert!(body.contains("action=\"/p/main/donate-10\""));
    }

    async fn test_app() -> axum::Router {
        let config = public_payment_link_config();
        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.migrate().await.unwrap();
        router(AppState {
            config: Arc::new(config),
            store: Arc::new(store),
            pricing: Arc::new(FixedRateSource),
        })
    }

    fn public_payment_link_config() -> Config {
        let config: Config = toml::from_str(
            r#"
            [server]
            public_url = "https://pay.example.com"

            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"

            [stores.onchain]
            network = "bitcoin"
            descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            electrum_servers = ["ssl://electrum.blockstream.info:50002"]

            [[stores.payment_links]]
            id = "donate-10"
            amount = "10.00"
            currency = "USD"
            metadata = { source = "github-pages" }
            "#,
        )
        .unwrap();
        config.validate().unwrap();
        config
    }

    struct FixedRateSource;

    #[async_trait]
    impl RateSource for FixedRateSource {
        async fn btc_rate(&self, quote: &str) -> anyhow::Result<Rate> {
            Ok(Rate {
                source: "test".to_string(),
                value: if quote == "USD" {
                    Decimal::from(100_000)
                } else {
                    Decimal::ONE
                },
            })
        }
    }
}
