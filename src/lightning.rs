use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    config::{LightningBackend, LightningConfig, LightningSweepConfig},
    invoice::{Invoice, InvoiceStatus},
};

const PHOENIXD_STARTUP_RETRY_ATTEMPTS: usize = 5;
const PHOENIXD_STARTUP_RETRY_DELAY_MS: u64 = 500;

#[derive(Debug, Clone)]
pub struct LightningInvoice {
    pub bolt11: String,
    pub payment_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightningObservation {
    pub received_sats: u64,
    pub next_status: InvoiceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepDecision {
    pub balance_sats: u64,
    pub amount_sats: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct SweepResult {
    pub balance_sats: u64,
    pub amount_sats: u64,
    pub address: String,
    pub tx_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HotBalance {
    pub backend: String,
    pub balance_sats: u64,
    pub spendable_sats: u64,
}

pub async fn create_invoice(
    config: &LightningConfig,
    amount_sats: u64,
    description: &str,
) -> anyhow::Result<LightningInvoice> {
    match config.backend {
        LightningBackend::Phoenixd => {
            create_phoenixd_invoice(config, amount_sats, description).await
        }
        LightningBackend::Barkd => create_barkd_invoice(config, amount_sats, description).await,
    }
}

pub async fn sweep_to_address(
    config: &LightningSweepConfig,
    address: String,
) -> anyhow::Result<Option<SweepResult>> {
    match config.backend {
        LightningBackend::Phoenixd => sweep_phoenixd_to_address(config, address).await,
        LightningBackend::Barkd => sweep_barkd_to_address(config, address).await,
    }
}

pub async fn hot_balance(config: &LightningConfig) -> anyhow::Result<HotBalance> {
    let client = reqwest::Client::new();
    match config.backend {
        LightningBackend::Phoenixd => {
            let password = lightning_api_secret(config)?;
            let balance = get_phoenixd_balance(config.url.as_str(), &client, &password).await?;
            Ok(HotBalance {
                backend: "phoenixd".to_string(),
                balance_sats: balance.balance_sats,
                spendable_sats: balance.balance_sats,
            })
        }
        LightningBackend::Barkd => {
            let token = lightning_api_secret(config)?;
            let balance = get_barkd_balance(config.url.as_str(), &client, &token).await?;
            Ok(HotBalance {
                backend: "barkd".to_string(),
                balance_sats: balance.spendable_sats,
                spendable_sats: balance.spendable_sats,
            })
        }
    }
}

pub async fn observe(
    config: &LightningConfig,
    invoice: &Invoice,
) -> anyhow::Result<LightningObservation> {
    match config.backend {
        LightningBackend::Phoenixd => observe_phoenixd_invoice(config, invoice).await,
        LightningBackend::Barkd => observe_barkd_invoice(config, invoice).await,
    }
}

async fn create_barkd_invoice(
    config: &LightningConfig,
    amount_sats: u64,
    description: &str,
) -> anyhow::Result<LightningInvoice> {
    let token = lightning_api_secret(config)?;
    let url = format!(
        "{}/api/v1/lightning/receives/invoice",
        config.url.trim_end_matches('/')
    );
    let response: BarkdInvoiceResponse = reqwest::Client::new()
        .post(url)
        .bearer_auth(token)
        .json(&BarkdInvoiceRequest {
            amount_sat: amount_sats,
            description: Some(description),
        })
        .send()
        .await
        .context("barkd invoice request failed")?
        .error_for_status()
        .context("barkd invoice returned an error")?
        .json()
        .await
        .context("failed to decode barkd invoice response")?;

    Ok(LightningInvoice {
        payment_hash: Some(response.invoice.clone()),
        bolt11: response.invoice,
    })
}

async fn create_phoenixd_invoice(
    config: &LightningConfig,
    amount_sats: u64,
    description: &str,
) -> anyhow::Result<LightningInvoice> {
    let password = lightning_api_secret(config)?;
    let url = format!("{}/createinvoice", config.url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let mut request = client.post(url).form(&[
        ("amountSat", amount_sats.to_string()),
        ("description", description.to_string()),
    ]);
    request = request.basic_auth("", Some(password));

    let response: PhoenixdInvoiceResponse = request
        .send()
        .await
        .context("phoenixd createinvoice request failed")?
        .error_for_status()
        .context("phoenixd createinvoice returned an error")?
        .json()
        .await
        .context("failed to decode phoenixd createinvoice response")?;

    Ok(LightningInvoice {
        bolt11: response.serialized,
        payment_hash: response.payment_hash,
    })
}

async fn observe_phoenixd_invoice(
    config: &LightningConfig,
    invoice: &Invoice,
) -> anyhow::Result<LightningObservation> {
    let payment_hash = invoice
        .lightning_payment_hash
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("invoice has no lightning payment hash"))?;
    let payment = get_phoenixd_incoming_payment(config, payment_hash).await?;
    let paid = payment.is_paid && payment.received_sat >= invoice.btc_amount_sats;
    Ok(LightningObservation {
        received_sats: if payment.is_paid {
            payment.received_sat
        } else {
            invoice.paid_sats
        },
        next_status: next_status(invoice.status, invoice.expires_at, Utc::now(), paid),
    })
}

async fn observe_barkd_invoice(
    config: &LightningConfig,
    invoice: &Invoice,
) -> anyhow::Result<LightningObservation> {
    let identifier = invoice
        .lightning_payment_hash
        .as_deref()
        .or(invoice.lightning_bolt11.as_deref())
        .ok_or_else(|| anyhow::anyhow!("invoice has no barkd lightning receive identifier"))?;
    let receive = get_barkd_receive(config, identifier).await?;
    let paid = receive.finished_at.is_some()
        && receive.preimage_revealed_at.is_some()
        && receive.amount_sat >= invoice.btc_amount_sats;
    Ok(LightningObservation {
        received_sats: if paid {
            receive.amount_sat
        } else {
            invoice.paid_sats
        },
        next_status: next_status(invoice.status, invoice.expires_at, Utc::now(), paid),
    })
}

fn next_status(
    current: InvoiceStatus,
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
    paid: bool,
) -> InvoiceStatus {
    if paid {
        if current == InvoiceStatus::Expired || now > expires_at {
            InvoiceStatus::PaidLate
        } else {
            InvoiceStatus::Settled
        }
    } else if now > expires_at {
        InvoiceStatus::Expired
    } else {
        current
    }
}

async fn get_phoenixd_incoming_payment(
    config: &LightningConfig,
    payment_hash: &str,
) -> anyhow::Result<PhoenixdIncomingPaymentResponse> {
    let password = match &config.api_password_env {
        Some(env) => Some(std::env::var(env).with_context(|| format!("missing env var {env}"))?),
        None => None,
    };
    let url = format!(
        "{}/payments/incoming/{}",
        config.url.trim_end_matches('/'),
        payment_hash
    );
    let client = reqwest::Client::new();
    let mut request = client.get(url);
    if let Some(password) = password {
        request = request.basic_auth("", Some(password));
    }

    request
        .send()
        .await
        .context("phoenixd incoming payment request failed")?
        .error_for_status()
        .context("phoenixd incoming payment returned an error")?
        .json()
        .await
        .context("failed to decode phoenixd incoming payment response")
}

async fn get_barkd_receive(
    config: &LightningConfig,
    identifier: &str,
) -> anyhow::Result<BarkdReceiveResponse> {
    let token = lightning_api_secret(config)?;
    let mut url = reqwest::Url::parse(&format!(
        "{}/api/v1/lightning/receives",
        config.url.trim_end_matches('/')
    ))
    .context("invalid barkd url")?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("invalid barkd url"))?
        .push(identifier);

    reqwest::Client::new()
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .context("barkd receive status request failed")?
        .error_for_status()
        .context("barkd receive status returned an error")?
        .json()
        .await
        .context("failed to decode barkd receive status response")
}

fn lightning_api_secret(config: &LightningConfig) -> anyhow::Result<String> {
    let env = config
        .api_password_env
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("lightning backend requires api_password_env"))?;
    std::env::var(env).with_context(|| format!("missing env var {env}"))
}

async fn sweep_phoenixd_to_address(
    config: &LightningSweepConfig,
    address: String,
) -> anyhow::Result<Option<SweepResult>> {
    let password = std::env::var(&config.full_api_password_env)
        .with_context(|| format!("missing env var {}", config.full_api_password_env))?;
    let client = reqwest::Client::new();
    let balance = get_phoenixd_balance(config.url.as_str(), &client, &password).await?;
    let decision = sweep_decision(
        balance.balance_sats,
        config.min_balance_sats,
        config.target_balance_sats,
    );
    let Some(amount_sats) = decision.amount_sats else {
        return Ok(None);
    };

    let mut form = vec![
        ("address", address.clone()),
        ("amountSat", amount_sats.to_string()),
    ];
    let feerate;
    if let Some(value) = config.feerate_sat_byte {
        feerate = value.to_string();
        form.push(("feerateSatByte", feerate));
    }
    let response: PhoenixdSendToAddressResponse = client
        .post(format!(
            "{}/sendtoaddress",
            config.url.trim_end_matches('/')
        ))
        .basic_auth("", Some(password))
        .form(&form)
        .send()
        .await
        .context("phoenixd sendtoaddress request failed")?
        .error_for_status()
        .context("phoenixd sendtoaddress returned an error")?
        .json()
        .await
        .context("failed to decode phoenixd sendtoaddress response")?;

    Ok(Some(SweepResult {
        balance_sats: balance.balance_sats,
        amount_sats,
        address,
        tx_id: response.tx_id.or(response.txid),
    }))
}

async fn sweep_barkd_to_address(
    config: &LightningSweepConfig,
    address: String,
) -> anyhow::Result<Option<SweepResult>> {
    let token = std::env::var(&config.full_api_password_env)
        .with_context(|| format!("missing env var {}", config.full_api_password_env))?;
    let client = reqwest::Client::new();
    let balance = get_barkd_balance(config.url.as_str(), &client, &token).await?;
    let decision = sweep_decision(
        balance.spendable_sats,
        config.min_balance_sats,
        config.target_balance_sats,
    );
    let Some(amount_sats) = decision.amount_sats else {
        return Ok(None);
    };

    let response: BarkdSendOnchainResponse = client
        .post(format!(
            "{}/api/v1/wallet/send-onchain",
            config.url.trim_end_matches('/')
        ))
        .bearer_auth(token)
        .json(&BarkdSendOnchainRequest {
            destination: address.clone(),
            amount_sat: amount_sats,
        })
        .send()
        .await
        .context("barkd send-onchain request failed")?
        .error_for_status()
        .context("barkd send-onchain returned an error")?
        .json()
        .await
        .context("failed to decode barkd send-onchain response")?;

    Ok(Some(SweepResult {
        balance_sats: balance.spendable_sats,
        amount_sats,
        address,
        tx_id: Some(response.offboard_txid),
    }))
}

async fn get_barkd_balance(
    url: &str,
    client: &reqwest::Client,
    token: &str,
) -> anyhow::Result<BarkdBalanceResponse> {
    client
        .get(format!(
            "{}/api/v1/wallet/balance",
            url.trim_end_matches('/')
        ))
        .bearer_auth(token)
        .send()
        .await
        .context("barkd balance request failed")?
        .error_for_status()
        .context("barkd balance returned an error")?
        .json()
        .await
        .context("failed to decode barkd balance response")
}

async fn get_phoenixd_balance(
    url: &str,
    client: &reqwest::Client,
    password: &str,
) -> anyhow::Result<PhoenixdBalanceResponse> {
    let url = format!("{}/getbalance", url.trim_end_matches('/'));
    let mut last_error = None;

    for attempt in 1..=PHOENIXD_STARTUP_RETRY_ATTEMPTS {
        match client.get(&url).basic_auth("", Some(password)).send().await {
            Ok(response) => {
                return response
                    .error_for_status()
                    .context("phoenixd getbalance returned an error")?
                    .json()
                    .await
                    .context("failed to decode phoenixd getbalance response");
            }
            Err(error) => {
                last_error = Some(error);
                if attempt < PHOENIXD_STARTUP_RETRY_ATTEMPTS {
                    tokio::time::sleep(std::time::Duration::from_millis(
                        PHOENIXD_STARTUP_RETRY_DELAY_MS,
                    ))
                    .await;
                }
            }
        }
    }

    Err(last_error.expect("phoenixd getbalance retry loop ran at least once"))
        .context("phoenixd getbalance request failed")
}

fn sweep_decision(
    balance_sats: u64,
    min_balance_sats: u64,
    target_balance_sats: u64,
) -> SweepDecision {
    let amount_sats = (balance_sats > min_balance_sats)
        .then_some(balance_sats.saturating_sub(target_balance_sats))
        .filter(|amount| *amount > 0);
    SweepDecision {
        balance_sats,
        amount_sats,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhoenixdInvoiceResponse {
    serialized: String,
    payment_hash: Option<String>,
}

#[derive(Debug, Serialize)]
struct BarkdInvoiceRequest<'a> {
    amount_sat: u64,
    description: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct BarkdInvoiceResponse {
    invoice: String,
}

#[derive(Debug, Deserialize)]
struct BarkdReceiveResponse {
    amount_sat: u64,
    finished_at: Option<DateTime<Utc>>,
    preimage_revealed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct BarkdBalanceResponse {
    #[serde(rename = "spendable_sat")]
    spendable_sats: u64,
}

#[derive(Debug, Serialize)]
struct BarkdSendOnchainRequest {
    destination: String,
    amount_sat: u64,
}

#[derive(Debug, Deserialize)]
struct BarkdSendOnchainResponse {
    offboard_txid: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhoenixdIncomingPaymentResponse {
    is_paid: bool,
    received_sat: u64,
}

#[derive(Debug, Deserialize)]
struct PhoenixdBalanceResponse {
    #[serde(rename = "balanceSat")]
    balance_sats: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhoenixdSendToAddressResponse {
    tx_id: Option<String>,
    txid: Option<String>,
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use rust_decimal::Decimal;
    use uuid::Uuid;

    use super::{
        BarkdBalanceResponse, BarkdReceiveResponse, PhoenixdBalanceResponse, create_invoice,
        next_status, observe, sweep_decision, sweep_to_address,
    };
    use crate::{
        config::{LightningBackend, LightningConfig, LightningSweepConfig},
        invoice::{Invoice, InvoiceStatus},
    };

    #[test]
    fn paid_lightning_invoice_settles_before_expiry() {
        let now = Utc::now();
        assert_eq!(
            next_status(InvoiceStatus::New, now + Duration::minutes(1), now, true),
            InvoiceStatus::Settled
        );
    }

    #[test]
    fn paid_lightning_invoice_after_expiry_is_late() {
        let now = Utc::now();
        assert_eq!(
            next_status(
                InvoiceStatus::Expired,
                now - Duration::minutes(1),
                now,
                true
            ),
            InvoiceStatus::PaidLate
        );
    }

    #[test]
    fn sweep_decision_keeps_target_balance() {
        let decision = sweep_decision(150_000, 100_000, 25_000);
        assert_eq!(decision.amount_sats, Some(125_000));
    }

    #[test]
    fn sweep_decision_skips_balance_at_threshold() {
        let decision = sweep_decision(100_000, 100_000, 25_000);
        assert_eq!(decision.amount_sats, None);
    }

    #[test]
    fn decodes_phoenixd_balance_response() {
        let balance: PhoenixdBalanceResponse =
            serde_json::from_str(r#"{"balanceSat":1234,"feeCreditSat":0}"#).unwrap();
        assert_eq!(balance.balance_sats, 1234);
    }

    #[test]
    fn decodes_barkd_receive_response() {
        let receive: BarkdReceiveResponse = serde_json::from_str(
            r#"{
                "amount_sat": 1234,
                "payment_hash": "hash",
                "payment_preimage": "preimage",
                "invoice": "lnbc123",
                "htlc_vtxos": [],
                "finished_at": "2026-05-21T00:00:00Z",
                "preimage_revealed_at": "2026-05-21T00:00:00Z"
            }"#,
        )
        .unwrap();
        assert_eq!(receive.amount_sat, 1234);
        assert!(receive.finished_at.is_some());
        assert!(receive.preimage_revealed_at.is_some());
    }

    #[test]
    fn decodes_barkd_balance_response() {
        let balance: BarkdBalanceResponse = serde_json::from_str(
            r#"{
                "spendable_sat": 150000,
                "pending_lightning_send_sat": 0,
                "claimable_lightning_receive_sat": 0,
                "pending_in_round_sat": 0,
                "pending_board_sat": 0,
                "pending_exit_sat": null
            }"#,
        )
        .unwrap();
        assert_eq!(balance.spendable_sats, 150_000);
    }

    #[tokio::test]
    async fn barkd_create_invoice_uses_json_api_and_bearer_auth() {
        let server = test_barkd_server().await;
        let env = format!("QPAYD_TEST_BARKD_TOKEN_{}", Uuid::new_v4().simple());
        unsafe {
            std::env::set_var(&env, "test-token");
        }
        let invoice = create_invoice(
            &LightningConfig {
                backend: LightningBackend::Barkd,
                url: server,
                api_password_env: Some(env),
            },
            1234,
            "qpayd invoice",
        )
        .await
        .unwrap();
        assert_eq!(invoice.bolt11, "lnbc1234test");
        assert_eq!(invoice.payment_hash.as_deref(), Some("lnbc1234test"));
    }

    #[tokio::test]
    async fn barkd_observe_settles_finished_revealed_receive() {
        let server = test_barkd_server().await;
        let env = format!("QPAYD_TEST_BARKD_TOKEN_{}", Uuid::new_v4().simple());
        unsafe {
            std::env::set_var(&env, "test-token");
        }
        let now = Utc::now();
        let observation = observe(
            &LightningConfig {
                backend: LightningBackend::Barkd,
                url: server,
                api_password_env: Some(env),
            },
            &Invoice {
                id: Uuid::new_v4(),
                store_id: "main".to_string(),
                status: InvoiceStatus::New,
                amount: Decimal::ONE,
                currency: "USD".to_string(),
                btc_amount_sats: 1234,
                paid_sats: 0,
                confirmed_sats: 0,
                unconfirmed_sats: 0,
                onchain_address: None,
                onchain_address_index: None,
                onchain_script_pubkey: None,
                lightning_bolt11: Some("lnbc1234test".to_string()),
                lightning_payment_hash: Some("lnbc1234test".to_string()),
                idempotency_key: None,
                rate_source: "test".to_string(),
                rate: Decimal::ONE,
                metadata: serde_json::json!({}),
                expires_at: now + Duration::minutes(1),
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
        assert_eq!(observation.next_status, InvoiceStatus::Settled);
        assert_eq!(observation.received_sats, 1234);
    }

    #[tokio::test]
    async fn barkd_sweep_sends_excess_spendable_balance() {
        let server = test_barkd_server().await;
        let env = format!("QPAYD_TEST_BARKD_TOKEN_{}", Uuid::new_v4().simple());
        unsafe {
            std::env::set_var(&env, "test-token");
        }
        let result = sweep_to_address(
            &LightningSweepConfig {
                backend: LightningBackend::Barkd,
                url: server,
                full_api_password_env: env,
                destination_descriptor: None,
                destination_descriptor_env: None,
                min_balance_sats: 100_000,
                target_balance_sats: 25_000,
                interval_seconds: 3600,
                feerate_sat_byte: None,
            },
            "bc1qexample".to_string(),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(result.balance_sats, 150_000);
        assert_eq!(result.amount_sats, 125_000);
        assert_eq!(result.address, "bc1qexample");
        assert_eq!(result.tx_id.as_deref(), Some("barkd-offboard-txid"));
    }

    async fn test_barkd_server() -> String {
        use axum::{
            Json, Router,
            extract::Path,
            http::{HeaderMap, StatusCode},
            routing::{get, post},
        };

        async fn create_invoice(
            headers: HeaderMap,
            Json(body): Json<serde_json::Value>,
        ) -> Result<Json<serde_json::Value>, StatusCode> {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer test-token")
            );
            assert_eq!(body["amount_sat"], 1234);
            assert_eq!(body["description"], "qpayd invoice");
            Ok(Json(serde_json::json!({ "invoice": "lnbc1234test" })))
        }

        async fn receive_status(
            headers: HeaderMap,
            Path(identifier): Path<String>,
        ) -> Result<Json<serde_json::Value>, StatusCode> {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer test-token")
            );
            assert_eq!(identifier, "lnbc1234test");
            Ok(Json(serde_json::json!({
                "amount_sat": 1234,
                "payment_hash": "hash",
                "payment_preimage": "preimage",
                "invoice": "lnbc1234test",
                "htlc_vtxos": [],
                "finished_at": Utc::now(),
                "preimage_revealed_at": Utc::now()
            })))
        }

        async fn balance(headers: HeaderMap) -> Result<Json<serde_json::Value>, StatusCode> {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer test-token")
            );
            Ok(Json(serde_json::json!({
                "spendable_sat": 150000,
                "pending_lightning_send_sat": 0,
                "claimable_lightning_receive_sat": 0,
                "pending_in_round_sat": 0,
                "pending_board_sat": 0,
                "pending_exit_sat": null
            })))
        }

        async fn send_onchain(
            headers: HeaderMap,
            Json(body): Json<serde_json::Value>,
        ) -> Result<Json<serde_json::Value>, StatusCode> {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer test-token")
            );
            assert_eq!(body["destination"], "bc1qexample");
            assert_eq!(body["amount_sat"], 125_000);
            Ok(Json(serde_json::json!({
                "offboard_txid": "barkd-offboard-txid"
            })))
        }

        let app = Router::new()
            .route("/api/v1/lightning/receives/invoice", post(create_invoice))
            .route(
                "/api/v1/lightning/receives/{identifier}",
                get(receive_status),
            )
            .route("/api/v1/wallet/balance", get(balance))
            .route("/api/v1/wallet/send-onchain", post(send_onchain));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }
}
