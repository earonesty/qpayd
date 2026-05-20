use std::sync::Arc;

use anyhow::Context;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
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
        .route("/healthz", get(healthz))
        .route("/v1/stores/{store_id}/invoices", post(create_invoice))
        .route(
            "/v1/stores/{store_id}/invoices/{invoice_id}",
            get(get_invoice),
        )
        .route("/i/{store_id}/{invoice_id}", get(checkout))
        .with_state(state)
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

    let currency = request.currency.to_uppercase();
    let rate = state.pricing.btc_rate(&currency).await?;
    let btc_amount_sats = sats_for(request.amount, rate.value)?;
    let (onchain_address, onchain_address_index) = match &store_cfg.onchain {
        Some(onchain) => {
            let index = state.store.reserve_address_index(&store_id).await?;
            let descriptor = onchain
                .descriptor
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
            (
                Some(
                    derived
                        .address(network)
                        .context("descriptor does not produce an address")?
                        .to_string(),
                ),
                Some(index),
            )
        }
        None => (None, None),
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
        amount: request.amount,
        currency,
        btc_amount_sats,
        onchain_address,
        onchain_address_index,
        lightning_bolt11: lightning_invoice
            .as_ref()
            .map(|invoice| invoice.bolt11.clone()),
        lightning_payment_hash: lightning_invoice.and_then(|invoice| invoice.payment_hash),
        rate_source: rate.source,
        rate: rate.value,
        metadata: request.metadata.unwrap_or_else(|| serde_json::json!({})),
        checkout_url,
        expires_at: now + Duration::minutes(store_cfg.expiry_minutes() as i64),
        created_at: now,
        updated_at: now,
    };

    state.store.insert_invoice(&invoice).await?;

    if let (Some(url), Some(secret_env)) = (&store_cfg.webhook_url, &store_cfg.webhook_secret_env) {
        let secret =
            std::env::var(secret_env).with_context(|| format!("missing env var {secret_env}"))?;
        let event = crate::webhook::Event {
            id: format!("evt_{}", Uuid::new_v4()),
            event_type: "invoice.created".to_string(),
            data: InvoiceResponse::new(invoice.clone(), store_cfg.confirmations()),
            created_at: Utc::now(),
        };
        crate::webhook::deliver(url, &secret, &event).await?;
    }

    Ok(Json(InvoiceResponse::new(
        invoice,
        store_cfg.confirmations(),
    )))
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
    use rust_decimal::Decimal;

    use super::sats_for;

    #[test]
    fn converts_fiat_to_sats() {
        let sats = sats_for(Decimal::from(25), Decimal::from(100_000)).unwrap();
        assert_eq!(sats, 25_000);
    }
}
