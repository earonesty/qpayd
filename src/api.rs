use std::sync::Arc;

use anyhow::Context;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use bitcoin::secp256k1::Secp256k1;
use chrono::{Duration, Utc};
use miniscript::{Descriptor, DescriptorPublicKey};
use qrcode::{QrCode, render::svg};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    config::{Config, PaymentLinkConfig, StoreConfig},
    events,
    invoice::{Invoice, InvoiceStatus},
    pricing::RateSource,
    storage::Store,
};

const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";
const MAX_IDEMPOTENCY_KEY_LEN: usize = 255;

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
            "/v1/public/stores/{store_id}/payment-links/{payment_link_id}/invoices",
            post(create_public_payment_link_invoice).options(public_payment_link_invoice_preflight),
        )
        .route(
            "/v1/public/stores/{store_id}/invoices/{invoice_id}",
            get(get_public_invoice).options(public_invoice_preflight),
        )
        .route(
            "/v1/public/stores/{store_id}/invoices/{invoice_id}/qr/bitcoin.svg",
            get(get_public_invoice_bitcoin_qr).options(public_invoice_preflight),
        )
        .route(
            "/v1/public/stores/{store_id}/invoices/{invoice_id}/qr/lightning.svg",
            get(get_public_invoice_lightning_qr).options(public_invoice_preflight),
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
    let idempotency_key = idempotency_key_from_headers(&headers)?;
    if let Some(key) = &idempotency_key
        && let Some(invoice) = state
            .store
            .invoice_by_idempotency_key(&store_id, key)
            .await?
    {
        return Ok(Json(InvoiceResponse::new(
            invoice,
            store_cfg.confirmations(),
        )));
    }

    let invoice = build_invoice(
        &state,
        store_cfg,
        store_id,
        request.amount,
        request.currency,
        request.metadata.unwrap_or_else(|| serde_json::json!({})),
        idempotency_key,
    )
    .await?;

    Ok(Json(InvoiceResponse::new(
        invoice,
        store_cfg.confirmations(),
    )))
}

async fn get_public_invoice(
    State(state): State<AppState>,
    Path((store_id, invoice_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    let cors = public_cors_for_store(&state.config, store_cfg, &headers)?;
    let invoice = state
        .store
        .invoice(&store_id, invoice_id)
        .await?
        .ok_or(ApiError::not_found("invoice not found"))?;

    json_public_response(
        PublicInvoiceResponse::new(invoice, store_cfg.confirmations()),
        cors,
    )
}

async fn get_public_invoice_bitcoin_qr(
    State(state): State<AppState>,
    Path((store_id, invoice_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    let cors = public_cors_for_store(&state.config, store_cfg, &headers)?;
    let invoice = state
        .store
        .invoice(&store_id, invoice_id)
        .await?
        .ok_or(ApiError::not_found("invoice not found"))?;
    let uri = bitcoin_uri(&invoice).ok_or(ApiError::not_found("invoice has no bitcoin address"))?;
    svg_qr_response(&uri, cors)
}

async fn get_public_invoice_lightning_qr(
    State(state): State<AppState>,
    Path((store_id, invoice_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    let cors = public_cors_for_store(&state.config, store_cfg, &headers)?;
    let invoice = state
        .store
        .invoice(&store_id, invoice_id)
        .await?
        .ok_or(ApiError::not_found("invoice not found"))?;
    let uri =
        lightning_uri(&invoice).ok_or(ApiError::not_found("invoice has no lightning invoice"))?;
    svg_qr_response(&uri, cors)
}

async fn create_public_payment_link_invoice(
    State(state): State<AppState>,
    Path((store_id, payment_link_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    let payment_link = store_cfg
        .payment_link(&payment_link_id)
        .ok_or(ApiError::not_found("payment link not found"))?;
    let cors = public_cors_for_payment_link(&state.config, store_cfg, payment_link, &headers)?;
    let idempotency_key = idempotency_key_from_headers(&headers)?;
    if let Some(key) = &idempotency_key
        && let Some(invoice) = state
            .store
            .invoice_by_idempotency_key(&store_id, key)
            .await?
    {
        return json_public_response(
            PublicInvoiceResponse::new(invoice, store_cfg.confirmations()),
            cors,
        );
    }

    let invoice = build_invoice(
        &state,
        store_cfg,
        store_id,
        payment_link.amount,
        payment_link.currency.clone(),
        payment_link.metadata.clone(),
        idempotency_key,
    )
    .await?;

    json_public_response(
        PublicInvoiceResponse::new(invoice, store_cfg.confirmations()),
        cors,
    )
}

async fn public_payment_link_invoice_preflight(
    State(state): State<AppState>,
    Path((store_id, payment_link_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    let payment_link = store_cfg
        .payment_link(&payment_link_id)
        .ok_or(ApiError::not_found("payment link not found"))?;
    let cors = public_cors_for_payment_link(&state.config, store_cfg, payment_link, &headers)?;
    Ok(preflight_response(cors))
}

async fn public_invoice_preflight(
    State(state): State<AppState>,
    Path((store_id, _invoice_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    let cors = public_cors_for_store(&state.config, store_cfg, &headers)?;
    Ok(preflight_response(cors))
}

async fn build_invoice(
    state: &AppState,
    store_cfg: &crate::config::StoreConfig,
    store_id: String,
    amount: Decimal,
    currency: String,
    metadata: serde_json::Value,
    idempotency_key: Option<String>,
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
    let invoice = Invoice {
        id,
        store_id,
        status: InvoiceStatus::New,
        amount,
        currency,
        btc_amount_sats,
        paid_sats: 0,
        confirmed_sats: 0,
        unconfirmed_sats: 0,
        onchain_address,
        onchain_address_index,
        onchain_script_pubkey,
        lightning_bolt11: lightning_invoice
            .as_ref()
            .map(|invoice| invoice.bolt11.clone()),
        lightning_payment_hash: lightning_invoice.and_then(|invoice| invoice.payment_hash),
        idempotency_key,
        rate_source: rate.source,
        rate: rate.value,
        metadata,
        expires_at: now + Duration::minutes(store_cfg.expiry_minutes() as i64),
        created_at: now,
        updated_at: now,
    };

    let event = events::invoice_created_event(&invoice, now);
    if let Err(error) = state
        .store
        .insert_invoice(&invoice, &event, store_cfg.webhook_url.as_deref())
        .await
    {
        if let Some(key) = &invoice.idempotency_key
            && let Some(existing) = state
                .store
                .invoice_by_idempotency_key(&invoice.store_id, key)
                .await?
        {
            return Ok(existing);
        }
        return Err(error);
    }

    Ok(invoice)
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

fn bitcoin_uri(invoice: &Invoice) -> Option<String> {
    let address = invoice.onchain_address.as_ref()?;
    let btc = Decimal::from(invoice.btc_amount_sats) / Decimal::from(100_000_000u64);
    Some(format!("bitcoin:{address}?amount={btc}"))
}

fn lightning_uri(invoice: &Invoice) -> Option<String> {
    invoice
        .lightning_bolt11
        .as_ref()
        .map(|bolt11| format!("lightning:{bolt11}"))
}

#[derive(Debug, Clone)]
enum PublicCors {
    AnyOrigin,
    Origin(HeaderValue),
    NoBrowserOrigin,
}

fn public_cors_for_payment_link(
    config: &Config,
    store: &StoreConfig,
    payment_link: &PaymentLinkConfig,
    headers: &HeaderMap,
) -> Result<PublicCors, ApiError> {
    public_cors_for_allowed_origins(
        most_specific_allowed_origins(
            &config.server.public_allowed_origins,
            &store.public_allowed_origins,
            &payment_link.public_allowed_origins,
        ),
        headers,
    )
}

fn public_cors_for_store(
    config: &Config,
    store: &StoreConfig,
    headers: &HeaderMap,
) -> Result<PublicCors, ApiError> {
    public_cors_for_allowed_origins(
        most_specific_allowed_origins(
            &config.server.public_allowed_origins,
            &store.public_allowed_origins,
            &[],
        ),
        headers,
    )
}

fn most_specific_allowed_origins<'a>(
    server: &'a [String],
    store: &'a [String],
    payment_link: &'a [String],
) -> &'a [String] {
    if !payment_link.is_empty() {
        payment_link
    } else if !store.is_empty() {
        store
    } else {
        server
    }
}

fn public_cors_for_allowed_origins(
    allowed_origins: &[String],
    headers: &HeaderMap,
) -> Result<PublicCors, ApiError> {
    if allowed_origins.is_empty() {
        return Ok(PublicCors::AnyOrigin);
    }

    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(PublicCors::NoBrowserOrigin);
    };
    let origin_str = origin
        .to_str()
        .map_err(|_| ApiError::forbidden("origin not allowed"))?;

    if allowed_origins
        .iter()
        .any(|allowed| same_origin(allowed, origin_str))
    {
        Ok(PublicCors::Origin(origin.clone()))
    } else {
        Err(ApiError::forbidden("origin not allowed"))
    }
}

fn same_origin(configured: &str, request_origin: &str) -> bool {
    configured.trim_end_matches('/') == request_origin.trim_end_matches('/')
}

fn json_public_response<T: Serialize>(value: T, cors: PublicCors) -> Result<Response, ApiError> {
    let mut response = Json(value).into_response();
    add_public_cors_headers(response.headers_mut(), cors);
    Ok(response)
}

fn preflight_response(cors: PublicCors) -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    add_public_cors_headers(response.headers_mut(), cors);
    response
}

fn add_public_cors_headers(headers: &mut HeaderMap, cors: PublicCors) {
    match cors {
        PublicCors::AnyOrigin => {
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            );
        }
        PublicCors::Origin(origin) => {
            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
            headers.insert(header::VARY, HeaderValue::from_static("origin"));
        }
        PublicCors::NoBrowserOrigin => {}
    }
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET,POST,OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type,idempotency-key"),
    );
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("content-type"),
    );
}

fn idempotency_key_from_headers(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    let Some(value) = headers.get(IDEMPOTENCY_KEY_HEADER) else {
        return Ok(None);
    };
    let key = value
        .to_str()
        .map_err(|_| ApiError::bad_request("Idempotency-Key must be valid ASCII"))?;
    if key.is_empty() {
        return Err(ApiError::bad_request("Idempotency-Key cannot be empty"));
    }
    if key.trim() != key {
        return Err(ApiError::bad_request(
            "Idempotency-Key cannot contain surrounding whitespace",
        ));
    }
    if key.len() > MAX_IDEMPOTENCY_KEY_LEN {
        return Err(ApiError::bad_request("Idempotency-Key is too long"));
    }
    Ok(Some(key.to_string()))
}

fn svg_qr_response(value: &str, cors: PublicCors) -> Result<Response, ApiError> {
    let code = QrCode::new(value.as_bytes()).map_err(|error| ApiError {
        status: StatusCode::BAD_REQUEST,
        message: error.to_string(),
    })?;
    let body = code
        .render::<svg::Color<'_>>()
        .min_dimensions(256, 256)
        .dark_color(svg::Color("#08100c"))
        .light_color(svg::Color("#ffffff"))
        .build();
    let mut response = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/svg+xml; charset=utf-8")],
        body,
    )
        .into_response();
    add_public_cors_headers(response.headers_mut(), cors);
    Ok(response)
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
    pub paid_sats: u64,
    pub confirmed_sats: u64,
    pub unconfirmed_sats: u64,
    pub remaining_sats: u64,
    pub overpaid_sats: u64,
    pub onchain_address: Option<String>,
    pub onchain_address_index: Option<u32>,
    pub onchain_script_pubkey: Option<String>,
    pub lightning_bolt11: Option<String>,
    pub lightning_payment_hash: Option<String>,
    pub bitcoin: Option<BitcoinPaymentResponse>,
    pub lightning: Option<LightningPaymentResponse>,
    pub min_confirmations: u32,
    pub rate_source: String,
    pub rate: Decimal,
    pub metadata: serde_json::Value,
    pub expires_at: chrono::DateTime<Utc>,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
}

impl InvoiceResponse {
    fn new(invoice: Invoice, min_confirmations: u32) -> Self {
        let id = invoice.id;
        let store_id = invoice.store_id.clone();
        let bitcoin = invoice
            .onchain_address
            .as_ref()
            .map(|address| BitcoinPaymentResponse {
                address: address.clone(),
                uri: bitcoin_uri(&invoice).expect("address exists"),
                qr_svg_url: format!("/v1/public/stores/{store_id}/invoices/{id}/qr/bitcoin.svg"),
            });
        let lightning = invoice
            .lightning_bolt11
            .as_ref()
            .map(|bolt11| LightningPaymentResponse {
                bolt11: bolt11.clone(),
                uri: lightning_uri(&invoice).expect("bolt11 exists"),
                qr_svg_url: format!("/v1/public/stores/{store_id}/invoices/{id}/qr/lightning.svg"),
            });
        Self {
            id,
            store_id: invoice.store_id,
            status: invoice.status,
            amount: invoice.amount,
            currency: invoice.currency,
            btc_amount_sats: invoice.btc_amount_sats,
            paid_sats: invoice.paid_sats,
            confirmed_sats: invoice.confirmed_sats,
            unconfirmed_sats: invoice.unconfirmed_sats,
            remaining_sats: invoice.btc_amount_sats.saturating_sub(invoice.paid_sats),
            overpaid_sats: invoice.paid_sats.saturating_sub(invoice.btc_amount_sats),
            onchain_address: invoice.onchain_address,
            onchain_address_index: invoice.onchain_address_index,
            onchain_script_pubkey: invoice.onchain_script_pubkey,
            lightning_bolt11: invoice.lightning_bolt11,
            lightning_payment_hash: invoice.lightning_payment_hash,
            bitcoin,
            lightning,
            min_confirmations,
            rate_source: invoice.rate_source,
            rate: invoice.rate,
            metadata: invoice.metadata,
            expires_at: invoice.expires_at,
            created_at: invoice.created_at,
            updated_at: invoice.updated_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PublicInvoiceResponse {
    pub id: Uuid,
    pub store_id: String,
    pub status: InvoiceStatus,
    pub amount: Decimal,
    pub currency: String,
    pub btc_amount_sats: u64,
    pub paid_sats: u64,
    pub confirmed_sats: u64,
    pub unconfirmed_sats: u64,
    pub remaining_sats: u64,
    pub overpaid_sats: u64,
    pub bitcoin: Option<BitcoinPaymentResponse>,
    pub lightning: Option<LightningPaymentResponse>,
    pub min_confirmations: u32,
    pub rate_source: String,
    pub rate: Decimal,
    pub metadata: serde_json::Value,
    pub expires_at: chrono::DateTime<Utc>,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct BitcoinPaymentResponse {
    pub address: String,
    pub uri: String,
    pub qr_svg_url: String,
}

#[derive(Debug, Serialize)]
pub struct LightningPaymentResponse {
    pub bolt11: String,
    pub uri: String,
    pub qr_svg_url: String,
}

impl PublicInvoiceResponse {
    fn new(invoice: Invoice, min_confirmations: u32) -> Self {
        let id = invoice.id;
        let store_id = invoice.store_id.clone();
        let bitcoin = invoice
            .onchain_address
            .as_ref()
            .map(|address| BitcoinPaymentResponse {
                address: address.clone(),
                uri: bitcoin_uri(&invoice).expect("address exists"),
                qr_svg_url: format!("/v1/public/stores/{store_id}/invoices/{id}/qr/bitcoin.svg"),
            });
        let lightning = invoice
            .lightning_bolt11
            .as_ref()
            .map(|bolt11| LightningPaymentResponse {
                bolt11: bolt11.clone(),
                uri: lightning_uri(&invoice).expect("bolt11 exists"),
                qr_svg_url: format!("/v1/public/stores/{store_id}/invoices/{id}/qr/lightning.svg"),
            });
        Self {
            id,
            store_id: invoice.store_id,
            status: invoice.status,
            amount: invoice.amount,
            currency: invoice.currency,
            btc_amount_sats: invoice.btc_amount_sats,
            paid_sats: invoice.paid_sats,
            confirmed_sats: invoice.confirmed_sats,
            unconfirmed_sats: invoice.unconfirmed_sats,
            remaining_sats: invoice.btc_amount_sats.saturating_sub(invoice.paid_sats),
            overpaid_sats: invoice.paid_sats.saturating_sub(invoice.btc_amount_sats),
            bitcoin,
            lightning,
            min_confirmations,
            rate_source: invoice.rate_source,
            rate: invoice.rate,
            metadata: invoice.metadata,
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

    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
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
        http::{Request, StatusCode, header},
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
    async fn create_invoice_returns_api_json_without_checkout_url() {
        // SAFETY: this test uses a single fixed value and does not depend on
        // concurrent mutation of the same environment variable.
        unsafe {
            std::env::set_var("QPAYD_MAIN_API_TOKEN", "test-token");
        }
        let app = test_app().await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stores/main/invoices")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"amount":"10.00","currency":"USD","metadata":{"source":"test"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["status"], "new");
        assert_eq!(body["onchain_address_index"], 0);
        assert_eq!(body["paid_sats"], 0);
        assert_eq!(body["remaining_sats"], body["btc_amount_sats"]);
        assert_eq!(body["overpaid_sats"], 0);
        assert!(body.get("checkout_url").is_none());
    }

    #[tokio::test]
    async fn create_invoice_reuses_idempotency_key() {
        // SAFETY: this test uses a single fixed value and does not depend on
        // concurrent mutation of the same environment variable.
        unsafe {
            std::env::set_var("QPAYD_MAIN_API_TOKEN", "test-token");
        }
        let app = test_app().await;

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stores/main/invoices")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("Idempotency-Key", "order-123")
                    .body(Body::from(r#"{"amount":"10.00","currency":"USD"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first: serde_json::Value =
            serde_json::from_slice(&to_bytes(first.into_body(), usize::MAX).await.unwrap())
                .unwrap();

        let second = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stores/main/invoices")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("Idempotency-Key", "order-123")
                    .body(Body::from(r#"{"amount":"10.00","currency":"USD"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let second: serde_json::Value =
            serde_json::from_slice(&to_bytes(second.into_body(), usize::MAX).await.unwrap())
                .unwrap();

        assert_eq!(second["id"], first["id"]);
        assert_eq!(
            second["onchain_address_index"],
            first["onchain_address_index"]
        );
    }

    #[tokio::test]
    async fn public_payment_link_creates_invoice_json() {
        let app = test_app().await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/public/stores/main/payment-links/donate-10/invoices")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["amount"], "10.00");
        assert_eq!(body["currency"], "USD");
        assert_eq!(body["metadata"]["kind"], "donation");
        assert!(
            body["bitcoin"]["address"]
                .as_str()
                .unwrap()
                .starts_with("bc1")
        );
        assert!(
            body["bitcoin"]["uri"]
                .as_str()
                .unwrap()
                .starts_with("bitcoin:bc1")
        );
        assert_eq!(
            body["bitcoin"]["qr_svg_url"],
            format!(
                "/v1/public/stores/main/invoices/{}/qr/bitcoin.svg",
                body["id"].as_str().unwrap()
            )
        );
        assert!(body.get("checkout_url").is_none());

        let qr_response = app
            .oneshot(
                Request::builder()
                    .uri(body["bitcoin"]["qr_svg_url"].as_str().unwrap())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(qr_response.status(), StatusCode::OK);
        assert_eq!(
            qr_response.headers()[header::CONTENT_TYPE],
            "image/svg+xml; charset=utf-8"
        );
        let qr_body = to_bytes(qr_response.into_body(), usize::MAX).await.unwrap();
        assert!(std::str::from_utf8(&qr_body).unwrap().contains("<svg"));
    }

    #[tokio::test]
    async fn public_payment_link_reuses_idempotency_key() {
        let app = test_app().await;

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/public/stores/main/payment-links/donate-10/invoices")
                    .header("Idempotency-Key", "browser-click-1")
                    .header(header::ORIGIN, "https://shop.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        let first: serde_json::Value =
            serde_json::from_slice(&to_bytes(first.into_body(), usize::MAX).await.unwrap())
                .unwrap();

        let second = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/public/stores/main/payment-links/donate-10/invoices")
                    .header("Idempotency-Key", "browser-click-1")
                    .header(header::ORIGIN, "https://shop.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(second.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        let second: serde_json::Value =
            serde_json::from_slice(&to_bytes(second.into_body(), usize::MAX).await.unwrap())
                .unwrap();

        assert_eq!(second["id"], first["id"]);
        assert_eq!(second["bitcoin"]["address"], first["bitcoin"]["address"]);
    }

    #[tokio::test]
    async fn public_payment_link_is_cors_permissive_by_default() {
        let app = test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/public/stores/main/payment-links/donate-10/invoices")
                    .header(header::ORIGIN, "https://any.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    }

    #[tokio::test]
    async fn public_payment_link_allows_configured_origin() {
        let mut config = test_config();
        config.stores[0].payment_links[0].public_allowed_origins =
            vec!["https://shop.example".to_string()];
        let app = test_app_with_config(config).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/public/stores/main/payment-links/donate-10/invoices")
                    .header(header::ORIGIN, "https://shop.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://shop.example"
        );
        assert_eq!(response.headers()[header::VARY], "origin");
    }

    #[tokio::test]
    async fn public_payment_link_rejects_disallowed_origin() {
        let mut config = test_config();
        config.stores[0].payment_links[0].public_allowed_origins =
            vec!["https://shop.example".to_string()];
        let app = test_app_with_config(config).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/public/stores/main/payment-links/donate-10/invoices")
                    .header(header::ORIGIN, "https://other.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }

    #[tokio::test]
    async fn public_payment_link_allows_server_side_requests_without_origin() {
        let mut config = test_config();
        config.stores[0].payment_links[0].public_allowed_origins =
            vec!["https://shop.example".to_string()];
        let app = test_app_with_config(config).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/public/stores/main/payment-links/donate-10/invoices")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }

    #[tokio::test]
    async fn public_preflight_uses_configured_origin_policy() {
        let mut config = test_config();
        config.stores[0].payment_links[0].public_allowed_origins =
            vec!["https://shop.example".to_string()];
        let app = test_app_with_config(config).await;

        let allowed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/v1/public/stores/main/payment-links/donate-10/invoices")
                    .header(header::ORIGIN, "https://shop.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            allowed.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://shop.example"
        );
        assert_eq!(
            allowed.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS],
            "content-type,idempotency-key"
        );

        let rejected = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/v1/public/stores/main/payment-links/donate-10/invoices")
                    .header(header::ORIGIN, "https://other.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    }

    async fn test_app() -> axum::Router {
        test_app_with_config(test_config()).await
    }

    async fn test_app_with_config(config: Config) -> axum::Router {
        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.migrate().await.unwrap();
        router(AppState {
            config: Arc::new(config),
            store: Arc::new(store),
            pricing: Arc::new(FixedRateSource),
        })
    }

    fn test_config() -> Config {
        let config: Config = toml::from_str(
            r#"
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
            metadata = { kind = "donation" }
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
