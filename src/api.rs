use std::sync::Arc;

use anyhow::Context;
use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use bitcoin::secp256k1::Secp256k1;
use chrono::{Duration, Utc};
use miniscript::{Descriptor, DescriptorPublicKey};
use qrcode::{QrCode, render::svg};
use reqwest::Url;
use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    config::{Config, PaymentLinkConfig, StoreConfig},
    events,
    invoice::{
        Invoice, InvoiceStatus, LightningSweepRecord, Refund, RefundApprovalStatus,
        RefundDestinationType, RefundStatus, SweepStatus,
    },
    pricing::RateSource,
    storage::{InvoiceListFilter, Store},
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
    let admin_cors_state = state.clone();
    Router::new()
        .route("/", get(index))
        .route("/admin", get(admin_portal))
        .route("/healthz", get(healthz))
        .route(
            "/v1/admin/session",
            get(admin_session).options(admin_session_preflight),
        )
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
        .route(
            "/v1/stores/{store_id}/invoices",
            get(list_invoices).post(create_invoice),
        )
        .route(
            "/v1/stores/{store_id}/invoices/{invoice_id}",
            get(get_invoice),
        )
        .route(
            "/v1/stores/{store_id}/invoices/{invoice_id}/refund-summary",
            get(get_invoice_refund_summary),
        )
        .route(
            "/v1/stores/{store_id}/invoices/{invoice_id}/refunds",
            get(list_invoice_refunds).post(create_invoice_refund),
        )
        .route(
            "/v1/stores/{store_id}/refunds",
            get(list_refunds).post(create_refund),
        )
        .route("/v1/stores/{store_id}/refunds/{refund_id}", get(get_refund))
        .route(
            "/v1/stores/{store_id}/refunds/{refund_id}/finalize",
            post(finalize_refund),
        )
        .route(
            "/v1/stores/{store_id}/refunds/{refund_id}/approve",
            post(approve_refund),
        )
        .route(
            "/v1/stores/{store_id}/refunds/{refund_id}/cancel",
            post(cancel_refund),
        )
        .route(
            "/v1/stores/{store_id}/refunds/{refund_id}/fail",
            post(fail_refund),
        )
        .route(
            "/v1/stores/{store_id}/lightning/balance",
            get(get_lightning_balance),
        )
        .route(
            "/v1/stores/{store_id}/lightning/sweeps",
            get(list_lightning_sweeps).post(create_lightning_sweep),
        )
        .route("/v1/stores/{store_id}/events", get(list_events))
        .route("/v1/stores/{store_id}/events/{event_id}", get(get_event))
        .route(
            "/v1/stores/{store_id}/events/{event_id}/replay",
            post(replay_event),
        )
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            admin_cors_state,
            admin_cors_middleware,
        ))
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

async fn admin_portal(State(state): State<AppState>) -> Result<Response, ApiError> {
    let admin = &state.config.server.admin;
    if !admin.enabled {
        return Err(ApiError::not_found("admin portal is not enabled"));
    }
    let asset_source = admin.asset_source.as_deref().ok_or(ApiError::bad_request(
        "admin asset source is not configured",
    ))?;
    let store_id_attr = admin
        .store_id
        .as_deref()
        .or_else(|| (state.config.stores.len() == 1).then(|| state.config.stores[0].id.as_str()))
        .map(|store_id| format!(r#" data-store-id="{}""#, escape_attr(store_id)))
        .unwrap_or_default();
    let integrity = admin.asset_integrity.as_deref();
    let crossorigin = if asset_source.starts_with("https://") {
        r#" crossorigin="anonymous""#
    } else {
        ""
    };
    let integrity_attr = integrity
        .map(|value| format!(r#" integrity="{}""#, escape_attr(value)))
        .unwrap_or_default();
    let html = format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>qpayd admin</title>
</head>
<body>
  <main id="qpayd-admin"></main>
  <script type="module" src="{asset_source}" data-qpayd-admin data-target="#qpayd-admin"{store_id_attr}{integrity_attr}{crossorigin}></script>
</body>
</html>"##,
        asset_source = escape_attr(asset_source),
    );
    let mut response = Html(html).into_response();
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_str(&admin_csp(asset_source))
            .map_err(|_| ApiError::bad_request("invalid admin asset source"))?,
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(self)"),
    );
    Ok(response)
}

async fn admin_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AdminSessionResponse>, ApiError> {
    let token = bearer_token(&headers)?;
    let mut stores = Vec::new();
    for store in &state.config.stores {
        let admin = token_matches(token, &store.admin_token(&state.config.auth)?);
        let payout = token_matches(token, &store.payout_token(&state.config.auth)?)
            || (admin && store.admin_token_can_payout(&state.config.auth));
        if admin || payout {
            let mut scopes = Vec::new();
            if admin {
                scopes.push("admin".to_string());
            }
            if payout {
                scopes.push("payout".to_string());
            }
            stores.push(AdminSessionStore {
                id: store.id.clone(),
                name: store.name.clone(),
                scopes,
            });
        }
    }
    if stores.is_empty() {
        return Err(ApiError::unauthorized());
    }
    Ok(Json(AdminSessionResponse { stores }))
}

async fn admin_session_preflight(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    Ok(preflight_response(admin_cors_for_session(
        &state.config,
        &headers,
    )?))
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
    authorize_api(&state.config.auth, store_cfg, &headers)?;
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
        BuildInvoiceInput {
            store_id,
            amount: request.amount,
            currency: request.currency,
            metadata: request.metadata.unwrap_or_else(|| serde_json::json!({})),
            idempotency_key,
            payment_link_id: None,
        },
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
            .invoice_by_payment_link_idempotency_key(&store_id, &payment_link_id, key)
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
        BuildInvoiceInput {
            store_id,
            amount: payment_link.amount,
            currency: payment_link.currency.clone(),
            metadata: payment_link.metadata.clone(),
            idempotency_key,
            payment_link_id: Some(payment_link_id),
        },
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

struct BuildInvoiceInput {
    store_id: String,
    amount: Decimal,
    currency: String,
    metadata: serde_json::Value,
    idempotency_key: Option<String>,
    payment_link_id: Option<String>,
}

async fn build_invoice(
    state: &AppState,
    store_cfg: &crate::config::StoreConfig,
    input: BuildInvoiceInput,
) -> Result<Invoice, ApiError> {
    let currency = input.currency.to_uppercase();
    let rate = state.pricing.btc_rate(&currency).await?;
    let btc_amount_sats = sats_for(input.amount, rate.value)?;
    let (onchain_address, onchain_address_index, onchain_script_pubkey) = match &store_cfg.onchain {
        Some(onchain) => {
            let index = state.store.reserve_address_index(&input.store_id).await?;
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
            crate::lightning::create_invoice(lightning, btc_amount_sats, "qpayd invoice")
                .await
                .map_err(|error| {
                    ApiError::bad_gateway(format!(
                        "lightning backend failed to create invoice: {error}"
                    ))
                })?,
        ),
        None => None,
    };

    let now = Utc::now();
    let id = Uuid::new_v4();
    let invoice = Invoice {
        id,
        store_id: input.store_id,
        status: InvoiceStatus::New,
        amount: input.amount,
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
        idempotency_key: input.idempotency_key,
        payment_link_id: input.payment_link_id,
        rate_source: rate.source,
        rate: rate.value,
        metadata: input.metadata,
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
        if let Some(key) = &invoice.idempotency_key {
            let existing = match &invoice.payment_link_id {
                Some(payment_link_id) => {
                    state
                        .store
                        .invoice_by_payment_link_idempotency_key(
                            &invoice.store_id,
                            payment_link_id,
                            key,
                        )
                        .await?
                }
                None => {
                    state
                        .store
                        .invoice_by_idempotency_key(&invoice.store_id, key)
                        .await?
                }
            };
            if let Some(existing) = existing {
                return Ok(existing);
            }
        }
        return Err(error.into());
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
    authorize_admin(&state.config.auth, store_cfg, &headers)?;

    Ok(Json(
        state
            .store
            .events(&store_id, query.limit.unwrap_or(50))
            .await?,
    ))
}

async fn list_invoices(
    State(state): State<AppState>,
    Path(store_id): Path<String>,
    Query(query): Query<ListInvoicesQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<InvoiceResponse>>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize_admin(&state.config.auth, store_cfg, &headers)?;
    let status = query
        .status
        .as_deref()
        .map(InvoiceStatus::try_from)
        .transpose()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let invoices = state
        .store
        .invoices(
            &store_id,
            InvoiceListFilter {
                status,
                limit: query.limit.unwrap_or(50),
            },
        )
        .await?;
    Ok(Json(
        invoices
            .into_iter()
            .map(|invoice| InvoiceResponse::new(invoice, store_cfg.confirmations()))
            .collect(),
    ))
}

async fn create_refund(
    State(state): State<AppState>,
    Path(store_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateRefundRequest>,
) -> Result<Json<Refund>, ApiError> {
    let invoice_id = request
        .invoice_id
        .ok_or(ApiError::bad_request("invoice_id is required"))?;
    create_refund_for_invoice(state, store_id, invoice_id, headers, request).await
}

async fn create_invoice_refund(
    State(state): State<AppState>,
    Path((store_id, invoice_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
    Json(request): Json<CreateRefundRequest>,
) -> Result<Json<Refund>, ApiError> {
    if let Some(body_invoice_id) = request.invoice_id
        && body_invoice_id != invoice_id
    {
        return Err(ApiError::bad_request(
            "body invoice_id must match the invoice route",
        ));
    }
    create_refund_for_invoice(state, store_id, invoice_id, headers, request).await
}

async fn create_refund_for_invoice(
    state: AppState,
    store_id: String,
    invoice_id: Uuid,
    headers: HeaderMap,
    request: CreateRefundRequest,
) -> Result<Json<Refund>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize_payout(&state.config.auth, store_cfg, &headers)?;
    let invoice = state
        .store
        .invoice(&store_id, invoice_id)
        .await?
        .ok_or(ApiError::not_found("invoice not found"))?;
    let idempotency_key = idempotency_key_from_headers(&headers)?;
    if let Some(key) = &idempotency_key
        && let Some(refund) = state
            .store
            .refund_by_idempotency_key(&store_id, key)
            .await?
    {
        ensure_refund_matches_invoice(&refund, invoice.id)?;
        return Ok(Json(refund));
    }

    let refunds = state
        .store
        .refunds_for_invoice(&store_id, invoice.id)
        .await?;
    let summary = refund_summary(invoice.clone(), refunds, store_cfg.confirmations());
    let refundable_sats = summary.refundable_sats;
    let amount_sats = request.amount_sats.unwrap_or(refundable_sats);
    if amount_sats == 0 {
        return Err(ApiError::bad_request(
            "refund amount must be greater than zero",
        ));
    }
    if amount_sats > refundable_sats {
        return Err(ApiError::bad_request(
            "refund amount cannot exceed observed paid_sats",
        ));
    }

    let now = Utc::now();
    let destination_type = request.destination.as_deref().map(refund_destination_type);
    let approval_status =
        if refund_requires_manual_approval(store_cfg, destination_type, amount_sats) {
            RefundApprovalStatus::Pending
        } else {
            RefundApprovalStatus::NotRequired
        };
    let refund = Refund {
        id: Uuid::new_v4(),
        store_id: store_id.clone(),
        invoice_id: invoice.id,
        status: RefundStatus::Pending,
        approval_status,
        amount_sats,
        destination_type,
        destination: request.destination,
        reason: request.reason,
        tx_id: None,
        payment_proof: None,
        failure_reason: None,
        idempotency_key,
        metadata: request.metadata.unwrap_or_else(|| serde_json::json!({})),
        created_at: now,
        updated_at: now,
        finalized_at: None,
    };
    let event = events::refund_created_event(&refund, now);
    if let Err(error) = state
        .store
        .insert_refund(&refund, &event, store_cfg.webhook_url.as_deref())
        .await
    {
        if let Some(key) = &refund.idempotency_key
            && let Some(existing) = state
                .store
                .refund_by_idempotency_key(&store_id, key)
                .await?
        {
            ensure_refund_matches_invoice(&existing, invoice.id)?;
            return Ok(Json(existing));
        }
        return Err(error.into());
    }
    Ok(Json(refund))
}

async fn get_invoice_refund_summary(
    State(state): State<AppState>,
    Path((store_id, invoice_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<RefundSummaryResponse>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize_admin(&state.config.auth, store_cfg, &headers)?;
    let invoice = state
        .store
        .invoice(&store_id, invoice_id)
        .await?
        .ok_or(ApiError::not_found("invoice not found"))?;
    let refunds = state
        .store
        .refunds_for_invoice(&store_id, invoice.id)
        .await?;
    Ok(Json(refund_summary(
        invoice,
        refunds,
        store_cfg.confirmations(),
    )))
}

async fn list_invoice_refunds(
    State(state): State<AppState>,
    Path((store_id, invoice_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<Vec<Refund>>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize_admin(&state.config.auth, store_cfg, &headers)?;
    if state.store.invoice(&store_id, invoice_id).await?.is_none() {
        return Err(ApiError::not_found("invoice not found"));
    }
    Ok(Json(
        state
            .store
            .refunds_for_invoice(&store_id, invoice_id)
            .await?,
    ))
}

async fn list_refunds(
    State(state): State<AppState>,
    Path(store_id): Path<String>,
    Query(query): Query<ListEventsQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<Refund>>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize_admin_or_payout(&state.config.auth, store_cfg, &headers)?;
    Ok(Json(
        state
            .store
            .refunds(&store_id, query.limit.unwrap_or(50))
            .await?,
    ))
}

async fn get_refund(
    State(state): State<AppState>,
    Path((store_id, refund_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<Refund>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize_admin_or_payout(&state.config.auth, store_cfg, &headers)?;
    let refund = state
        .store
        .refund(&store_id, refund_id)
        .await?
        .ok_or(ApiError::not_found("refund not found"))?;
    Ok(Json(refund))
}

async fn approve_refund(
    State(state): State<AppState>,
    Path((store_id, refund_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<Refund>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize_admin(&state.config.auth, store_cfg, &headers)?;
    let mut refund = state
        .store
        .refund(&store_id, refund_id)
        .await?
        .ok_or(ApiError::not_found("refund not found"))?;
    if refund.status != RefundStatus::Pending {
        return Err(ApiError::bad_request(
            "only pending refunds can be approved",
        ));
    }
    if refund.approval_status != RefundApprovalStatus::Pending {
        return Err(ApiError::bad_request(
            "refund does not require manual approval",
        ));
    }
    let now = Utc::now();
    refund.approval_status = RefundApprovalStatus::Approved;
    refund.updated_at = now;
    let event = events::refund_approved_event(&refund, now);
    let approved = state
        .store
        .update_refund_approval(&refund, &event, store_cfg.webhook_url.as_deref())
        .await?;
    if !approved {
        return Err(ApiError::bad_request("refund approval state changed"));
    }
    Ok(Json(refund))
}

async fn finalize_refund(
    State(state): State<AppState>,
    Path((store_id, refund_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
    Json(request): Json<FinalizeRefundRequest>,
) -> Result<Json<Refund>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize_payout(&state.config.auth, store_cfg, &headers)?;
    let mut refund = state
        .store
        .refund(&store_id, refund_id)
        .await?
        .ok_or(ApiError::not_found("refund not found"))?;
    if refund.status != RefundStatus::Pending {
        return Err(ApiError::bad_request(
            "only pending refunds can be finalized",
        ));
    }
    if !refund.approval_status.allows_execution() {
        return Err(ApiError::bad_request("refund requires admin approval"));
    }
    let now = Utc::now();
    refund.status = RefundStatus::Succeeded;
    refund.tx_id = request.tx_id;
    refund.payment_proof = request.payment_proof;
    refund.failure_reason = None;
    refund.updated_at = now;
    refund.finalized_at = Some(now);
    let event = events::refund_finalized_event(&refund, now);
    state
        .store
        .update_refund_status(&refund, &event, store_cfg.webhook_url.as_deref())
        .await?;
    Ok(Json(refund))
}

async fn fail_refund(
    State(state): State<AppState>,
    Path((store_id, refund_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
    Json(request): Json<FailRefundRequest>,
) -> Result<Json<Refund>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize_payout(&state.config.auth, store_cfg, &headers)?;
    let mut refund = state
        .store
        .refund(&store_id, refund_id)
        .await?
        .ok_or(ApiError::not_found("refund not found"))?;
    if refund.status != RefundStatus::Pending {
        return Err(ApiError::bad_request("only pending refunds can be failed"));
    }
    let reason = request.failure_reason.trim();
    if reason.is_empty() {
        return Err(ApiError::bad_request("failure_reason is required"));
    }

    let now = Utc::now();
    refund.status = RefundStatus::Failed;
    refund.failure_reason = Some(reason.to_string());
    refund.updated_at = now;
    let event = events::refund_failed_event(&refund, now);
    state
        .store
        .update_refund_status(&refund, &event, store_cfg.webhook_url.as_deref())
        .await?;
    Ok(Json(refund))
}

async fn cancel_refund(
    State(state): State<AppState>,
    Path((store_id, refund_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<Refund>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize_payout(&state.config.auth, store_cfg, &headers)?;
    let mut refund = state
        .store
        .refund(&store_id, refund_id)
        .await?
        .ok_or(ApiError::not_found("refund not found"))?;
    if refund.status != RefundStatus::Pending {
        return Err(ApiError::bad_request(
            "only pending refunds can be canceled",
        ));
    }
    let now = Utc::now();
    refund.status = RefundStatus::Canceled;
    refund.updated_at = now;
    let event = events::refund_canceled_event(&refund, now);
    state
        .store
        .update_refund_status(&refund, &event, store_cfg.webhook_url.as_deref())
        .await?;
    Ok(Json(refund))
}

async fn get_lightning_balance(
    State(state): State<AppState>,
    Path(store_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<LightningBalanceResponse>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize_admin(&state.config.auth, store_cfg, &headers)?;
    let lightning = store_cfg
        .lightning
        .as_ref()
        .ok_or(ApiError::not_found("store has no lightning backend"))?;
    let balance = crate::lightning::hot_balance(lightning)
        .await
        .map_err(|error| {
            ApiError::bad_gateway(format!(
                "lightning backend failed to report balance: {error}"
            ))
        })?;
    Ok(Json(LightningBalanceResponse {
        backend: balance.backend,
        balance_sats: balance.balance_sats,
        spendable_sats: balance.spendable_sats,
    }))
}

async fn create_lightning_sweep(
    State(state): State<AppState>,
    Path(store_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<LightningSweepRecord>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize_admin(&state.config.auth, store_cfg, &headers)?;
    let payout_cfg = store_cfg
        .effective_lightning_payout()
        .ok_or(ApiError::not_found("store has no lightning_payout config"))?;
    let sweep_cfg = payout_cfg
        .sweep
        .as_ref()
        .filter(|sweep| sweep.enabled)
        .ok_or(ApiError::not_found(
            "store has no enabled lightning_payout.sweep config",
        ))?;
    let network = store_cfg
        .onchain
        .as_ref()
        .map(|onchain| onchain.network.as_str())
        .unwrap_or("bitcoin")
        .parse::<bitcoin::Network>()
        .context("invalid bitcoin network")?;
    let destination = derive_lightning_sweep_address(sweep_cfg, network)?;
    let now = Utc::now();
    let record =
        match crate::lightning::sweep_to_address(&payout_cfg, sweep_cfg, destination.clone()).await
        {
            Ok(Some(result)) => LightningSweepRecord {
                id: Uuid::new_v4(),
                store_id: store_id.clone(),
                backend: payout_cfg.backend.as_str().to_string(),
                status: SweepStatus::Succeeded,
                balance_sats: result.balance_sats,
                amount_sats: result.amount_sats,
                address: result.address,
                tx_id: result.tx_id,
                error: None,
                created_at: now,
                updated_at: now,
            },
            Ok(None) => LightningSweepRecord {
                id: Uuid::new_v4(),
                store_id: store_id.clone(),
                backend: payout_cfg.backend.as_str().to_string(),
                status: SweepStatus::Skipped,
                balance_sats: 0,
                amount_sats: 0,
                address: destination,
                tx_id: None,
                error: None,
                created_at: now,
                updated_at: now,
            },
            Err(error) => LightningSweepRecord {
                id: Uuid::new_v4(),
                store_id: store_id.clone(),
                backend: payout_cfg.backend.as_str().to_string(),
                status: SweepStatus::Failed,
                balance_sats: 0,
                amount_sats: 0,
                address: destination,
                tx_id: None,
                error: Some(error.to_string()),
                created_at: now,
                updated_at: now,
            },
        };
    state.store.insert_lightning_sweep(&record).await?;
    if record.status == SweepStatus::Failed {
        return Err(ApiError::bad_gateway(
            record
                .error
                .clone()
                .unwrap_or_else(|| "lightning sweep failed".to_string()),
        ));
    }
    Ok(Json(record))
}

async fn list_lightning_sweeps(
    State(state): State<AppState>,
    Path(store_id): Path<String>,
    Query(query): Query<ListEventsQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<LightningSweepRecord>>, ApiError> {
    let store_cfg = state
        .config
        .store(&store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    authorize_admin(&state.config.auth, store_cfg, &headers)?;
    Ok(Json(
        state
            .store
            .lightning_sweeps(&store_id, query.limit.unwrap_or(50))
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
    authorize_admin(&state.config.auth, store_cfg, &headers)?;

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
    authorize_admin(&state.config.auth, store_cfg, &headers)?;
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
    authorize_admin(&state.config.auth, store_cfg, &headers)?;

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

fn authorize_api(
    auth: &crate::config::AuthConfig,
    store: &crate::config::StoreConfig,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    authorize_with_token(&store.api_token(auth)?, headers)
}

fn authorize_admin(
    auth: &crate::config::AuthConfig,
    store: &crate::config::StoreConfig,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    authorize_with_token(&store.admin_token(auth)?, headers)
}

fn authorize_payout(
    auth: &crate::config::AuthConfig,
    store: &crate::config::StoreConfig,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    match authorize_with_token(&store.payout_token(auth)?, headers) {
        Ok(()) => Ok(()),
        Err(error) if store.admin_token_can_payout(auth) => {
            authorize_with_token(&store.admin_token(auth)?, headers).map_err(|_| error)
        }
        Err(error) => Err(error),
    }
}

fn authorize_admin_or_payout(
    auth: &crate::config::AuthConfig,
    store: &crate::config::StoreConfig,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    authorize_admin(auth, store, headers).or_else(|_| authorize_payout(auth, store, headers))
}

fn authorize_with_token(expected: &str, headers: &HeaderMap) -> Result<(), ApiError> {
    let token = bearer_token(headers)?;
    if token_matches(token, expected) {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return Err(ApiError::unauthorized());
    };
    let Ok(value) = value.to_str() else {
        return Err(ApiError::unauthorized());
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err(ApiError::unauthorized());
    };
    Ok(token)
}

fn token_matches(token: &str, expected: &str) -> bool {
    subtle::ConstantTimeEq::ct_eq(token.as_bytes(), expected.as_bytes()).into()
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

fn derive_lightning_sweep_address(
    config: &crate::config::LightningSweepConfig,
    network: bitcoin::Network,
) -> anyhow::Result<String> {
    let descriptor = config
        .destination_descriptor()?
        .parse::<Descriptor<DescriptorPublicKey>>()
        .context("invalid lightning sweep destination descriptor")?;
    let secp = Secp256k1::verification_only();
    let derived = descriptor
        .derived_descriptor(&secp, 0)
        .context("failed to derive lightning sweep destination descriptor")?;
    let address = derived
        .address(network)
        .context("lightning sweep destination descriptor does not produce an address")?;
    Ok(address.to_string())
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

async fn admin_cors_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let cors = if request.uri().path() == "/v1/admin/session" {
        match admin_cors_for_session(&state.config, request.headers()) {
            Ok(cors) => Some(cors),
            Err(error) => return error.into_response(),
        }
    } else {
        let Some(store_id) = admin_store_id_from_path(request.uri().path()) else {
            return next.run(request).await;
        };
        match admin_cors_for_store(&state.config, store_id, request.headers()) {
            Ok(cors) => Some(cors),
            Err(error) => return error.into_response(),
        }
    };
    let cors = cors.expect("admin CORS branch sets a policy");
    if request.method() == Method::OPTIONS {
        return preflight_response(cors);
    }
    let mut response = next.run(request).await;
    add_public_cors_headers(response.headers_mut(), cors);
    response
}

fn admin_store_id_from_path(path: &str) -> Option<&str> {
    let mut parts = path.trim_start_matches('/').split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("v1"), Some("stores"), Some(store_id)) if !store_id.is_empty() => Some(store_id),
        _ => None,
    }
}

fn admin_cors_for_store(
    config: &Config,
    store_id: &str,
    headers: &HeaderMap,
) -> Result<PublicCors, ApiError> {
    if headers.get(header::ORIGIN).is_none() {
        return Ok(PublicCors::NoBrowserOrigin);
    }
    let store = config
        .store(store_id)
        .ok_or(ApiError::not_found("store not found"))?;
    if store.admin_allowed_origins.is_empty() {
        return Err(ApiError::forbidden("admin origin is not allowed"));
    }
    public_cors_for_allowed_origins(&store.admin_allowed_origins, headers)
}

fn admin_cors_for_session(config: &Config, headers: &HeaderMap) -> Result<PublicCors, ApiError> {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(PublicCors::NoBrowserOrigin);
    };
    let origin_str = origin
        .to_str()
        .map_err(|_| ApiError::forbidden("origin not allowed"))?;
    if config.stores.iter().any(|store| {
        store
            .admin_allowed_origins
            .iter()
            .any(|allowed| same_origin(allowed, origin_str))
    }) {
        Ok(PublicCors::Origin(origin.clone()))
    } else {
        Err(ApiError::forbidden("admin origin is not allowed"))
    }
}

fn admin_csp(asset_source: &str) -> String {
    let script_source = if asset_source.starts_with("https://") {
        Url::parse(asset_source)
            .ok()
            .and_then(|url| {
                Some(format!(
                    "{}://{}",
                    url.scheme(),
                    url.host_str().map(str::to_string)?
                ))
            })
            .unwrap_or_else(|| "'self'".to_string())
    } else {
        "'self'".to_string()
    };
    format!(
        "default-src 'none'; script-src 'self' {script_source}; connect-src 'self'; style-src 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob:; base-uri 'none'; frame-ancestors 'none'; object-src 'none'"
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

fn escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
        HeaderValue::from_static("authorization,content-type,idempotency-key"),
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
struct ListInvoicesQuery {
    limit: Option<u32>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListEventsQuery {
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct CreateRefundRequest {
    pub invoice_id: Option<Uuid>,
    pub amount_sats: Option<u64>,
    pub destination: Option<String>,
    pub reason: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct FinalizeRefundRequest {
    pub tx_id: Option<String>,
    pub payment_proof: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FailRefundRequest {
    pub failure_reason: String,
}

#[derive(Debug, Serialize)]
pub struct AdminSessionResponse {
    pub stores: Vec<AdminSessionStore>,
}

#[derive(Debug, Serialize)]
pub struct AdminSessionStore {
    pub id: String,
    pub name: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RefundSummaryResponse {
    pub invoice: InvoiceResponse,
    pub refunds: Vec<Refund>,
    pub paid_sats: u64,
    pub already_refunded_sats: u64,
    pub refundable_sats: u64,
    pub overpaid_sats: u64,
    pub pending_refund_sats: u64,
    pub succeeded_refund_sats: u64,
}

#[derive(Debug, Serialize)]
pub struct LightningBalanceResponse {
    pub backend: String,
    pub balance_sats: u64,
    pub spendable_sats: u64,
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

fn refund_summary(
    invoice: Invoice,
    refunds: Vec<Refund>,
    min_confirmations: u32,
) -> RefundSummaryResponse {
    let pending_refund_sats = refunds
        .iter()
        .filter(|refund| {
            matches!(
                refund.status,
                RefundStatus::Pending | RefundStatus::Processing
            )
        })
        .map(|refund| refund.amount_sats)
        .sum();
    let succeeded_refund_sats = refunds
        .iter()
        .filter(|refund| refund.status == RefundStatus::Succeeded)
        .map(|refund| refund.amount_sats)
        .sum();
    let already_refunded_sats = pending_refund_sats + succeeded_refund_sats;
    let paid_sats = invoice.paid_sats;
    let overpaid_sats = paid_sats.saturating_sub(invoice.btc_amount_sats);
    RefundSummaryResponse {
        invoice: InvoiceResponse::new(invoice, min_confirmations),
        refunds,
        paid_sats,
        already_refunded_sats,
        refundable_sats: paid_sats.saturating_sub(already_refunded_sats),
        overpaid_sats,
        pending_refund_sats,
        succeeded_refund_sats,
    }
}

fn ensure_refund_matches_invoice(refund: &Refund, invoice_id: Uuid) -> Result<(), ApiError> {
    if refund.invoice_id == invoice_id {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "Idempotency-Key is already used for a different invoice",
        ))
    }
}

fn refund_requires_manual_approval(
    store: &crate::config::StoreConfig,
    destination_type: Option<RefundDestinationType>,
    amount_sats: u64,
) -> bool {
    refund_manual_approval_thresholds(store, destination_type)
        .into_iter()
        .any(|threshold| amount_sats >= threshold)
}

fn refund_manual_approval_thresholds(
    store: &crate::config::StoreConfig,
    destination_type: Option<RefundDestinationType>,
) -> Vec<u64> {
    let mut thresholds = Vec::new();
    if matches!(
        destination_type,
        Some(RefundDestinationType::LightningInvoice | RefundDestinationType::Lnurl)
            | Some(RefundDestinationType::Unknown)
            | None
    ) && let Some(refunds) = store
        .effective_lightning_payout()
        .and_then(|payout| payout.refunds)
        && refunds.enabled
        && let Some(threshold) = refunds.manual_approval_threshold_sats
    {
        thresholds.push(threshold);
    }
    if matches!(
        destination_type,
        Some(RefundDestinationType::BitcoinAddress | RefundDestinationType::BitcoinUri)
            | Some(RefundDestinationType::Unknown)
            | None
    ) && let Some(refunds) = store
        .bitcoin_payout
        .as_ref()
        .and_then(|payout| payout.refunds.as_ref())
        && refunds.enabled
        && let Some(threshold) = refunds.manual_approval_threshold_sats
    {
        thresholds.push(threshold);
    }
    thresholds
}

fn refund_destination_type(destination: &str) -> RefundDestinationType {
    let value = destination.trim();
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("bitcoin:") && lower.len() > "bitcoin:".len() {
        RefundDestinationType::BitcoinUri
    } else if (lower.starts_with("lightning:") && lower.len() > "lightning:".len())
        || looks_like_bolt11(&lower)
    {
        RefundDestinationType::LightningInvoice
    } else if looks_like_lnurl(&lower) {
        RefundDestinationType::Lnurl
    } else if looks_like_bitcoin_address(value, &lower) {
        RefundDestinationType::BitcoinAddress
    } else {
        RefundDestinationType::Unknown
    }
}

fn looks_like_bolt11(lower: &str) -> bool {
    (lower.starts_with("lnbc") && lower.len() > 8)
        || (lower.starts_with("lntb") && lower.len() > 8)
        || (lower.starts_with("lnbcrt") && lower.len() > 10)
}

fn looks_like_lnurl(lower: &str) -> bool {
    lower.starts_with("lnurl1") && lower.len() > 12
}

fn looks_like_bitcoin_address(value: &str, lower: &str) -> bool {
    let len = value.len();
    ((lower.starts_with("bc1") || lower.starts_with("tb1") || lower.starts_with("bcrt1"))
        && len >= 14)
        || ((value.starts_with('1') || value.starts_with('3')) && (26..=62).contains(&len))
        || ((value.starts_with('m') || value.starts_with('n') || value.starts_with('2'))
            && (26..=62).contains(&len))
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

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: message.into(),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        tracing::error!(error = ?error, "internal API error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal server error".to_string(),
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

    use super::{AppState, refund_destination_type, router, sats_for};
    use crate::{
        config::{BitcoinPayoutBackend, BitcoinPayoutConfig, Config, RefundExecutionConfig},
        events,
        invoice::{PaymentAmounts, RefundDestinationType, RefundStatus},
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
    async fn list_invoices_returns_recent_admin_invoices() {
        // SAFETY: this test uses a single fixed value and does not depend on
        // concurrent mutation of the same environment variable.
        unsafe {
            std::env::set_var("QPAYD_MAIN_API_TOKEN", "test-token");
        }
        let app = test_app().await;

        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stores/main/invoices")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"amount":"10.00","currency":"USD"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::OK);
        let created: serde_json::Value =
            serde_json::from_slice(&to_bytes(created.into_body(), usize::MAX).await.unwrap())
                .unwrap();

        let listed = app
            .oneshot(
                Request::builder()
                    .uri("/v1/stores/main/invoices?status=new")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let listed: serde_json::Value =
            serde_json::from_slice(&to_bytes(listed.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["id"], created["id"]);
    }

    #[tokio::test]
    async fn admin_token_env_scopes_admin_routes() {
        // SAFETY: this test uses fixed values and does not depend on concurrent
        // mutation of the same environment variables.
        unsafe {
            std::env::set_var("QPAYD_MAIN_API_TOKEN", "test-token");
            std::env::set_var("QPAYD_MAIN_ADMIN_TOKEN", "admin-token");
        }
        let mut config = test_config();
        config.stores[0].admin_token_env = Some("QPAYD_MAIN_ADMIN_TOKEN".to_string());
        let app = test_app_with_config(config).await;

        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/stores/main/invoices")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        let allowed = app
            .oneshot(
                Request::builder()
                    .uri("/v1/stores/main/invoices")
                    .header(header::AUTHORIZATION, "Bearer admin-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn admin_session_routes_token_to_authorized_store_scopes() {
        unsafe {
            std::env::set_var("QPAYD_MAIN_API_TOKEN", "test-token");
            std::env::set_var("QPAYD_MAIN_ADMIN_TOKEN", "admin-token");
            std::env::set_var("QPAYD_MAIN_PAYOUT_TOKEN", "payout-token");
        }
        let mut config = test_config();
        config.server.admin.enabled = true;
        config.server.admin.asset_source = Some("/admin.js".to_string());
        config.stores[0].admin_token_env = Some("QPAYD_MAIN_ADMIN_TOKEN".to_string());
        config.stores[0].payout_token_env = Some("QPAYD_MAIN_PAYOUT_TOKEN".to_string());
        config.stores[0].admin_allowed_origins = vec!["https://pay.example".to_string()];
        let app = test_app_with_config(config).await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/session")
                    .header(header::AUTHORIZATION, "Bearer payout-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["stores"][0]["id"], "main");
        assert_eq!(body["stores"][0]["scopes"], serde_json::json!(["payout"]));

        let denied = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/session")
                    .header(header::AUTHORIZATION, "Bearer nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_token_can_payout_when_configured() {
        unsafe {
            std::env::set_var("QPAYD_MAIN_API_TOKEN", "test-token");
            std::env::set_var("QPAYD_MAIN_ADMIN_TOKEN", "admin-token");
            std::env::set_var("QPAYD_MAIN_PAYOUT_TOKEN", "payout-token");
        }
        let mut config = test_config();
        config.stores[0].admin_token_env = Some("QPAYD_MAIN_ADMIN_TOKEN".to_string());
        config.stores[0].payout_token_env = Some("QPAYD_MAIN_PAYOUT_TOKEN".to_string());
        config.stores[0].admin_token_can_payout = Some(true);
        let (app, store) = test_app_and_store_with_config(config).await;

        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stores/main/invoices")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"amount":"10.00","currency":"USD"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let created: serde_json::Value =
            serde_json::from_slice(&to_bytes(created.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let invoice_id = uuid::Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
        store
            .update_invoice_payment_amounts(
                "main",
                invoice_id,
                PaymentAmounts {
                    paid_sats: 12_000,
                    confirmed_sats: 12_000,
                    unconfirmed_sats: 0,
                },
                chrono::Utc::now(),
            )
            .await
            .unwrap();

        let allowed = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/stores/main/invoices/{invoice_id}/refunds"))
                    .header(header::AUTHORIZATION, "Bearer admin-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"amount_sats":2000}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn payout_token_env_scopes_refund_mutations() {
        // SAFETY: this test uses fixed values and does not depend on concurrent
        // mutation of the same environment variables.
        unsafe {
            std::env::set_var("QPAYD_MAIN_API_TOKEN", "test-token");
            std::env::set_var("QPAYD_MAIN_ADMIN_TOKEN", "admin-token");
            std::env::set_var("QPAYD_MAIN_PAYOUT_TOKEN", "payout-token");
        }
        let mut config = test_config();
        config.stores[0].admin_token_env = Some("QPAYD_MAIN_ADMIN_TOKEN".to_string());
        config.stores[0].payout_token_env = Some("QPAYD_MAIN_PAYOUT_TOKEN".to_string());
        let (app, store) = test_app_and_store_with_config(config).await;

        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stores/main/invoices")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"amount":"10.00","currency":"USD"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::OK);
        let created: serde_json::Value =
            serde_json::from_slice(&to_bytes(created.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let invoice_id = uuid::Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
        store
            .update_invoice_payment_amounts(
                "main",
                invoice_id,
                PaymentAmounts {
                    paid_sats: 12_000,
                    confirmed_sats: 12_000,
                    unconfirmed_sats: 0,
                },
                chrono::Utc::now(),
            )
            .await
            .unwrap();

        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/stores/main/invoices/{invoice_id}/refunds"))
                    .header(header::AUTHORIZATION, "Bearer admin-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"amount_sats":2000}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        let allowed = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/stores/main/invoices/{invoice_id}/refunds"))
                    .header(header::AUTHORIZATION, "Bearer payout-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"amount_sats":2000}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn large_refunds_require_admin_approval_before_finalize() {
        unsafe {
            std::env::set_var("QPAYD_MAIN_API_TOKEN", "test-token");
            std::env::set_var("QPAYD_MAIN_ADMIN_TOKEN", "admin-token");
            std::env::set_var("QPAYD_MAIN_PAYOUT_TOKEN", "payout-token");
        }
        let mut config = test_config();
        config.stores[0].admin_token_env = Some("QPAYD_MAIN_ADMIN_TOKEN".to_string());
        config.stores[0].payout_token_env = Some("QPAYD_MAIN_PAYOUT_TOKEN".to_string());
        config.stores[0].bitcoin_payout = Some(BitcoinPayoutConfig {
            backend: BitcoinPayoutBackend::Bitcoind,
            url: "http://127.0.0.1:8332".to_string(),
            wallet: Some("refunds".to_string()),
            rpc_auth_env: "BITCOIND_REFUND_RPC_AUTH".to_string(),
            refunds: Some(RefundExecutionConfig {
                enabled: true,
                max_refund_sats: 10_000,
                daily_refund_limit_sats: 50_000,
                manual_approval_threshold_sats: Some(1_000),
                poll_seconds: 30,
            }),
        });
        let (app, store) = test_app_and_store_with_config(config).await;

        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stores/main/invoices")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"amount":"10.00","currency":"USD"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let created: serde_json::Value =
            serde_json::from_slice(&to_bytes(created.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let invoice_id = uuid::Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
        store
            .update_invoice_payment_amounts(
                "main",
                invoice_id,
                PaymentAmounts {
                    paid_sats: 12_000,
                    confirmed_sats: 12_000,
                    unconfirmed_sats: 0,
                },
                chrono::Utc::now(),
            )
            .await
            .unwrap();

        let refund = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/stores/main/invoices/{invoice_id}/refunds"))
                    .header(header::AUTHORIZATION, "Bearer payout-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"amount_sats":2000,"destination":"bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(refund.status(), StatusCode::OK);
        let refund: serde_json::Value =
            serde_json::from_slice(&to_bytes(refund.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(refund["approval_status"], "pending");
        let refund_id = refund["id"].as_str().unwrap();

        let rejected_finalize = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/stores/main/refunds/{refund_id}/finalize"))
                    .header(header::AUTHORIZATION, "Bearer payout-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"tx_id":"refund-tx"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected_finalize.status(), StatusCode::BAD_REQUEST);

        let rejected_approval = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/stores/main/refunds/{refund_id}/approve"))
                    .header(header::AUTHORIZATION, "Bearer payout-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected_approval.status(), StatusCode::UNAUTHORIZED);

        let approved = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/stores/main/refunds/{refund_id}/approve"))
                    .header(header::AUTHORIZATION, "Bearer admin-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(approved.status(), StatusCode::OK);
        let approved: serde_json::Value =
            serde_json::from_slice(&to_bytes(approved.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(approved["approval_status"], "approved");

        let finalized = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/stores/main/refunds/{refund_id}/finalize"))
                    .header(header::AUTHORIZATION, "Bearer payout-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"tx_id":"refund-tx"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(finalized.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn invoice_scoped_refunds_track_refundable_balance() {
        // SAFETY: this test uses a single fixed value and does not depend on
        // concurrent mutation of the same environment variable.
        unsafe {
            std::env::set_var("QPAYD_MAIN_API_TOKEN", "test-token");
        }
        let (app, store) = test_app_and_store().await;

        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stores/main/invoices")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"amount":"10.00","currency":"USD"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::OK);
        let created: serde_json::Value =
            serde_json::from_slice(&to_bytes(created.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let invoice_id = uuid::Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();

        store
            .update_invoice_payment_amounts(
                "main",
                invoice_id,
                PaymentAmounts {
                    paid_sats: 12_000,
                    confirmed_sats: 12_000,
                    unconfirmed_sats: 0,
                },
                chrono::Utc::now(),
            )
            .await
            .unwrap();

        let summary = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/stores/main/invoices/{invoice_id}/refund-summary"
                    ))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(summary.status(), StatusCode::OK);
        let summary: serde_json::Value =
            serde_json::from_slice(&to_bytes(summary.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(summary["paid_sats"], 12_000);
        assert_eq!(summary["overpaid_sats"], 2_000);
        assert_eq!(summary["refundable_sats"], 12_000);

        let refund = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/stores/main/invoices/{invoice_id}/refunds"))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("Idempotency-Key", "refund-order-1")
                    .body(Body::from(
                        r#"{"amount_sats":2000,"destination":"bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh","reason":"overpayment"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(refund.status(), StatusCode::OK);
        let refund: serde_json::Value =
            serde_json::from_slice(&to_bytes(refund.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(refund["destination_type"], "bitcoin_address");
        assert_eq!(refund["idempotency_key"], "refund-order-1");

        let replayed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/stores/main/invoices/{invoice_id}/refunds"))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("Idempotency-Key", "refund-order-1")
                    .body(Body::from(
                        r#"{"amount_sats":3000,"destination":"bitcoin:bc1qother","reason":"retry"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replayed.status(), StatusCode::OK);
        let replayed: serde_json::Value =
            serde_json::from_slice(&to_bytes(replayed.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(replayed["id"], refund["id"]);
        assert_eq!(replayed["amount_sats"], 2000);

        let too_much = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/stores/main/invoices/{invoice_id}/refunds"))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"amount_sats":11000}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(too_much.status(), StatusCode::BAD_REQUEST);

        let canceled = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/stores/main/refunds/{}/cancel",
                        refund["id"].as_str().unwrap()
                    ))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(canceled.status(), StatusCode::OK);
        let canceled: serde_json::Value =
            serde_json::from_slice(&to_bytes(canceled.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(canceled["status"], "canceled");

        let summary = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/stores/main/invoices/{invoice_id}/refund-summary"
                    ))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let summary: serde_json::Value =
            serde_json::from_slice(&to_bytes(summary.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(summary["already_refunded_sats"], 0);
        assert_eq!(summary["refundable_sats"], 12_000);
    }

    #[tokio::test]
    async fn processing_refunds_count_against_refundable_balance() {
        // SAFETY: this test uses a single fixed value and does not depend on
        // concurrent mutation of the same environment variable.
        unsafe {
            std::env::set_var("QPAYD_MAIN_API_TOKEN", "test-token");
        }
        let (app, store) = test_app_and_store().await;

        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stores/main/invoices")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"amount":"10.00","currency":"USD"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let created: serde_json::Value =
            serde_json::from_slice(&to_bytes(created.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let invoice_id = uuid::Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
        store
            .update_invoice_payment_amounts(
                "main",
                invoice_id,
                PaymentAmounts {
                    paid_sats: 12_000,
                    confirmed_sats: 12_000,
                    unconfirmed_sats: 0,
                },
                chrono::Utc::now(),
            )
            .await
            .unwrap();

        let refund = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/stores/main/invoices/{invoice_id}/refunds"))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"amount_sats":2000,"destination":"bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let refund: serde_json::Value =
            serde_json::from_slice(&to_bytes(refund.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let refund_id = uuid::Uuid::parse_str(refund["id"].as_str().unwrap()).unwrap();
        let mut refund = store.refund("main", refund_id).await.unwrap().unwrap();
        let now = chrono::Utc::now();
        refund.status = RefundStatus::Processing;
        refund.updated_at = now;
        let event = events::refund_processing_event(&refund, now);
        store
            .update_refund_status(&refund, &event, None)
            .await
            .unwrap();

        let summary = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/stores/main/invoices/{invoice_id}/refund-summary"
                    ))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(summary.status(), StatusCode::OK);
        let summary: serde_json::Value =
            serde_json::from_slice(&to_bytes(summary.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(summary["pending_refund_sats"], 2_000);
        assert_eq!(summary["already_refunded_sats"], 2_000);
        assert_eq!(summary["refundable_sats"], 10_000);
    }

    #[tokio::test]
    async fn refund_idempotency_key_cannot_replay_across_invoices() {
        // SAFETY: this test uses a single fixed value and does not depend on
        // concurrent mutation of the same environment variable.
        unsafe {
            std::env::set_var("QPAYD_MAIN_API_TOKEN", "test-token");
        }
        let (app, store) = test_app_and_store().await;

        let mut invoice_ids = Vec::new();
        for _ in 0..2 {
            let created = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/stores/main/invoices")
                        .header(header::AUTHORIZATION, "Bearer test-token")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"amount":"10.00","currency":"USD"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            let created: serde_json::Value =
                serde_json::from_slice(&to_bytes(created.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            let invoice_id = uuid::Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
            store
                .update_invoice_payment_amounts(
                    "main",
                    invoice_id,
                    PaymentAmounts {
                        paid_sats: 12_000,
                        confirmed_sats: 12_000,
                        unconfirmed_sats: 0,
                    },
                    chrono::Utc::now(),
                )
                .await
                .unwrap();
            invoice_ids.push(invoice_id);
        }

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/stores/main/invoices/{}/refunds",
                        invoice_ids[0]
                    ))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("Idempotency-Key", "shared-refund-key")
                    .body(Body::from(r#"{"amount_sats":2000}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let second = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/stores/main/invoices/{}/refunds",
                        invoice_ids[1]
                    ))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("Idempotency-Key", "shared-refund-key")
                    .body(Body::from(r#"{"amount_sats":2000}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn refunds_can_finalize_with_proof_and_fail() {
        // SAFETY: this test uses a single fixed value and does not depend on
        // concurrent mutation of the same environment variable.
        unsafe {
            std::env::set_var("QPAYD_MAIN_API_TOKEN", "test-token");
        }
        let (app, store) = test_app_and_store().await;

        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stores/main/invoices")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"amount":"10.00","currency":"USD"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let created: serde_json::Value =
            serde_json::from_slice(&to_bytes(created.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let invoice_id = uuid::Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
        store
            .update_invoice_payment_amounts(
                "main",
                invoice_id,
                PaymentAmounts {
                    paid_sats: 12_000,
                    confirmed_sats: 12_000,
                    unconfirmed_sats: 0,
                },
                chrono::Utc::now(),
            )
            .await
            .unwrap();

        let finalized_refund = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/stores/main/invoices/{invoice_id}/refunds"))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"amount_sats":2000,"destination":"bitcoin:bc1qrefund","reason":"operator"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let finalized_refund: serde_json::Value = serde_json::from_slice(
            &to_bytes(finalized_refund.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(finalized_refund["destination_type"], "bitcoin_uri");

        let finalized = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/stores/main/refunds/{}/finalize",
                        finalized_refund["id"].as_str().unwrap()
                    ))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"tx_id":"tx123","payment_proof":"proof123"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(finalized.status(), StatusCode::OK);
        let finalized: serde_json::Value =
            serde_json::from_slice(&to_bytes(finalized.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(finalized["status"], "succeeded");
        assert_eq!(finalized["tx_id"], "tx123");
        assert_eq!(finalized["payment_proof"], "proof123");

        let failed_refund = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/stores/main/invoices/{invoice_id}/refunds"))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"amount_sats":1000,"destination":"lnbc1refund","reason":"operator"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let failed_refund: serde_json::Value = serde_json::from_slice(
            &to_bytes(failed_refund.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(failed_refund["destination_type"], "lightning_invoice");

        let failed = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/stores/main/refunds/{}/fail",
                        failed_refund["id"].as_str().unwrap()
                    ))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"failure_reason":"expired invoice"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(failed.status(), StatusCode::OK);
        let failed: serde_json::Value =
            serde_json::from_slice(&to_bytes(failed.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(failed["status"], "failed");
        assert_eq!(failed["failure_reason"], "expired invoice");
    }

    #[test]
    fn refund_destination_type_requires_realistic_prefixes() {
        assert_eq!(
            refund_destination_type("bc1"),
            RefundDestinationType::Unknown
        );
        assert_eq!(
            refund_destination_type("lnbc"),
            RefundDestinationType::Unknown
        );
        assert_eq!(
            refund_destination_type("lnurl"),
            RefundDestinationType::Unknown
        );
        assert_eq!(
            refund_destination_type("bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh"),
            RefundDestinationType::BitcoinAddress
        );
        assert_eq!(
            refund_destination_type("bitcoin:bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh"),
            RefundDestinationType::BitcoinUri
        );
        assert_eq!(
            refund_destination_type("lnbc2500u1pwywxzwpp5jptserfk4zkc6hvfqqqsq9w"),
            RefundDestinationType::LightningInvoice
        );
        assert_eq!(
            refund_destination_type("lnurl1dp68gurn8ghj7mrww4exctnrdakj7"),
            RefundDestinationType::Lnurl
        );
    }

    #[tokio::test]
    async fn admin_cors_requires_configured_origin() {
        let mut config = test_config();
        config.stores[0].admin_allowed_origins = vec!["https://admin.example".to_string()];
        let app = test_app_with_config(config).await;

        let allowed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/v1/stores/main/invoices")
                    .header(header::ORIGIN, "https://admin.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            allowed.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://admin.example"
        );
        assert!(
            allowed.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS]
                .to_str()
                .unwrap()
                .contains("authorization")
        );

        let rejected = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/v1/stores/main/invoices")
                    .header(header::ORIGIN, "https://other.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn admin_portal_serves_pinned_asset_bootstrap() {
        let mut config = test_config();
        config.server.admin.enabled = true;
        config.server.admin.store_id = Some("main".to_string());
        config.server.admin.asset_source =
            Some("https://cdn.jsdelivr.net/npm/@qpayd/admin@0.4.0/src/index.js".to_string());
        config.server.admin.asset_integrity = Some("sha384-testdigest".to_string());
        let app = test_app_with_config(config).await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("https://cdn.jsdelivr.net")
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        assert!(body.contains("data-qpayd-admin"));
        assert!(body.contains("data-store-id=\"main\""));
        assert!(body.contains("integrity=\"sha384-testdigest\""));
        assert!(body.contains("crossorigin=\"anonymous\""));
    }

    #[tokio::test]
    async fn admin_portal_can_defer_store_selection_to_token_session() {
        let mut config = test_config();
        config.server.admin.enabled = true;
        config.server.admin.store_id = None;
        config.server.admin.asset_source = Some("/admin.js".to_string());
        let mut second = config.stores[0].clone();
        second.id = "second".to_string();
        second.name = "Second Store".to_string();
        second.api_token_env = Some("QPAYD_SECOND_API_TOKEN".to_string());
        config.stores.push(second);
        let app = test_app_with_config(config).await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        assert!(body.contains("data-qpayd-admin"));
        assert!(!body.contains("data-store-id="));
    }

    #[tokio::test]
    async fn create_invoice_surfaces_lightning_backend_failure() {
        // SAFETY: this test uses fixed values and does not depend on concurrent
        // mutation of the same environment variables.
        unsafe {
            std::env::set_var("QPAYD_MAIN_API_TOKEN", "test-token");
            std::env::set_var("QPAYD_TEST_LIGHTNING_PASSWORD", "test-password");
        }
        let mut config = test_config();
        config.stores[0].onchain = None;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((socket, _)) = listener.accept().await {
                drop(socket);
            }
        });
        config.stores[0].lightning = Some(crate::config::LightningConfig {
            backend: crate::config::LightningBackend::Phoenixd,
            url: format!("http://{addr}"),
            api_password_env: Some("QPAYD_TEST_LIGHTNING_PASSWORD".to_string()),
        });
        let app = test_app_with_config(config).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stores/main/invoices")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"amount":"10.00","currency":"USD"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .starts_with("lightning backend failed to create invoice")
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
    async fn public_payment_link_does_not_reuse_admin_idempotency_key() {
        // SAFETY: this test uses a single fixed value and does not depend on
        // concurrent mutation of the same environment variable.
        unsafe {
            std::env::set_var("QPAYD_MAIN_API_TOKEN", "test-token");
        }
        let app = test_app().await;

        let admin = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stores/main/invoices")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("Idempotency-Key", "shared-order-1")
                    .body(Body::from(r#"{"amount":"10.00","currency":"USD"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(admin.status(), StatusCode::OK);
        let admin: serde_json::Value =
            serde_json::from_slice(&to_bytes(admin.into_body(), usize::MAX).await.unwrap())
                .unwrap();

        let public = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/public/stores/main/payment-links/donate-10/invoices")
                    .header("Idempotency-Key", "shared-order-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(public.status(), StatusCode::OK);
        let public: serde_json::Value =
            serde_json::from_slice(&to_bytes(public.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_ne!(public["id"], admin["id"]);

        let replay = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/public/stores/main/payment-links/donate-10/invoices")
                    .header("Idempotency-Key", "shared-order-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        let replay: serde_json::Value =
            serde_json::from_slice(&to_bytes(replay.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(replay["id"], public["id"]);
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
            "authorization,content-type,idempotency-key"
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
        test_app_and_store_with_config(config).await.0
    }

    async fn test_app_and_store() -> (axum::Router, Arc<SqliteStore>) {
        test_app_and_store_with_config(test_config()).await
    }

    async fn test_app_and_store_with_config(config: Config) -> (axum::Router, Arc<SqliteStore>) {
        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.migrate().await.unwrap();
        let store = Arc::new(store);
        let app_store: Arc<dyn Store> = store.clone();
        let app = router(AppState {
            config: Arc::new(config),
            store: app_store,
            pricing: Arc::new(FixedRateSource),
        });
        (app, store)
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
