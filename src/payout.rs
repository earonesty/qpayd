use anyhow::{Context, bail};
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use serde::{Deserialize, Serialize};

use crate::{
    config::{BitcoinPayoutConfig, LightningBackend, LightningPayoutConfig, StoreConfig},
    invoice::{Refund, RefundDestinationType},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayoutResult {
    pub tx_id: Option<String>,
    pub payment_proof: Option<String>,
}

pub async fn execute_refund(store: &StoreConfig, refund: &Refund) -> anyhow::Result<PayoutResult> {
    let destination = refund
        .destination
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("refund has no destination"))?;
    match refund.destination_type {
        Some(RefundDestinationType::LightningInvoice) => {
            let payout = store
                .effective_lightning_payout()
                .filter(lightning_refunds_enabled)
                .ok_or_else(|| anyhow::anyhow!("store has no enabled lightning payout refunds"))?;
            execute_lightning_refund(&payout, destination).await
        }
        Some(RefundDestinationType::BitcoinAddress | RefundDestinationType::BitcoinUri) => {
            let payout = store
                .bitcoin_payout
                .as_ref()
                .filter(|payout| bitcoin_refunds_enabled(payout))
                .ok_or_else(|| anyhow::anyhow!("store has no enabled bitcoin payout refunds"))?;
            execute_bitcoin_refund(payout, destination, refund.amount_sats).await
        }
        Some(RefundDestinationType::Lnurl | RefundDestinationType::Unknown) | None => {
            bail!("no payout driver accepted refund destination")
        }
    }
}

fn lightning_refunds_enabled(payout: &LightningPayoutConfig) -> bool {
    payout
        .refunds
        .as_ref()
        .is_some_and(|refunds| refunds.enabled)
}

fn bitcoin_refunds_enabled(payout: &BitcoinPayoutConfig) -> bool {
    payout
        .refunds
        .as_ref()
        .is_some_and(|refunds| refunds.enabled)
}

async fn execute_lightning_refund(
    payout: &LightningPayoutConfig,
    destination: &str,
) -> anyhow::Result<PayoutResult> {
    match payout.backend {
        LightningBackend::Phoenixd => pay_phoenixd_invoice(payout, destination).await,
        LightningBackend::Barkd => pay_barkd_invoice(payout, destination).await,
    }
}

async fn pay_phoenixd_invoice(
    payout: &LightningPayoutConfig,
    destination: &str,
) -> anyhow::Result<PayoutResult> {
    let password = std::env::var(&payout.full_api_password_env)
        .with_context(|| format!("missing env var {}", payout.full_api_password_env))?;
    let invoice = destination
        .strip_prefix("lightning:")
        .unwrap_or(destination)
        .to_string();
    let response: PhoenixdPayInvoiceResponse = crate::http::client()
        .post(format!("{}/payinvoice", payout.url.trim_end_matches('/')))
        .basic_auth("", Some(password))
        .form(&[("invoice", invoice)])
        .send()
        .await
        .context("phoenixd payinvoice request failed")?
        .error_for_status()
        .context("phoenixd payinvoice returned an error")?
        .json()
        .await
        .context("failed to decode phoenixd payinvoice response")?;
    Ok(PayoutResult {
        tx_id: response.payment_hash,
        payment_proof: response.preimage,
    })
}

async fn pay_barkd_invoice(
    payout: &LightningPayoutConfig,
    destination: &str,
) -> anyhow::Result<PayoutResult> {
    let token = std::env::var(&payout.full_api_password_env)
        .with_context(|| format!("missing env var {}", payout.full_api_password_env))?;
    let invoice = destination
        .strip_prefix("lightning:")
        .unwrap_or(destination)
        .to_string();
    let response: BarkdPayInvoiceResponse = crate::http::client()
        .post(format!(
            "{}/api/v1/lightning/sends/invoice",
            payout.url.trim_end_matches('/')
        ))
        .bearer_auth(token)
        .json(&BarkdPayInvoiceRequest { invoice })
        .send()
        .await
        .context("barkd pay invoice request failed")?
        .error_for_status()
        .context("barkd pay invoice returned an error")?
        .json()
        .await
        .context("failed to decode barkd pay invoice response")?;
    Ok(PayoutResult {
        tx_id: response.payment_hash.or(response.id),
        payment_proof: response.payment_preimage,
    })
}

async fn execute_bitcoin_refund(
    payout: &BitcoinPayoutConfig,
    destination: &str,
    amount_sats: u64,
) -> anyhow::Result<PayoutResult> {
    let address = bitcoin_refund_address(destination)?;
    let auth = std::env::var(&payout.rpc_auth_env)
        .with_context(|| format!("missing env var {}", payout.rpc_auth_env))?;
    let amount_btc = Decimal::from_u64(amount_sats)
        .ok_or_else(|| anyhow::anyhow!("invalid refund amount"))?
        / Decimal::from(100_000_000u64);
    let url = if let Some(wallet) = &payout.wallet {
        format!("{}/wallet/{}", payout.url.trim_end_matches('/'), wallet)
    } else {
        payout.url.clone()
    };
    let mut request = crate::http::client().post(url).json(&BitcoinRpcRequest {
        jsonrpc: "1.0",
        id: "qpayd-refund",
        method: "sendtoaddress",
        params: serde_json::json!([address, amount_btc.to_string()]),
    });
    if let Some((user, password)) = auth.split_once(':') {
        request = request.basic_auth(user, Some(password.to_string()));
    } else {
        request = request.bearer_auth(auth);
    }
    let response = request
        .send()
        .await
        .context("bitcoind sendtoaddress request failed")?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read bitcoind sendtoaddress response")?;
    let response: BitcoinRpcResponse<String> = serde_json::from_str(&body)
        .with_context(|| format!("failed to decode bitcoind sendtoaddress response: {body}"))?;
    if let Some(error) = response.error {
        bail!("bitcoind sendtoaddress failed: {error}");
    }
    if !status.is_success() {
        bail!("bitcoind sendtoaddress returned HTTP {status} without an RPC error body: {body}");
    }
    let tx_id = response
        .result
        .ok_or_else(|| anyhow::anyhow!("bitcoind sendtoaddress returned no txid"))?;
    Ok(PayoutResult {
        tx_id: Some(tx_id),
        payment_proof: None,
    })
}

fn bitcoin_refund_address(destination: &str) -> anyhow::Result<String> {
    if let Some(uri) = destination.strip_prefix("bitcoin:") {
        let address = uri
            .split_once('?')
            .map(|(address, _)| address)
            .unwrap_or(uri);
        if address.is_empty() {
            bail!("bitcoin URI has no address");
        }
        Ok(address.to_string())
    } else {
        Ok(destination.to_string())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhoenixdPayInvoiceResponse {
    payment_hash: Option<String>,
    preimage: Option<String>,
}

#[derive(Debug, Serialize)]
struct BarkdPayInvoiceRequest {
    invoice: String,
}

#[derive(Debug, Deserialize)]
struct BarkdPayInvoiceResponse {
    id: Option<String>,
    payment_hash: Option<String>,
    payment_preimage: Option<String>,
}

#[derive(Debug, Serialize)]
struct BitcoinRpcRequest {
    jsonrpc: &'static str,
    id: &'static str,
    method: &'static str,
    params: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct BitcoinRpcResponse<T> {
    result: Option<T>,
    error: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::post,
    };
    use chrono::Utc;
    use uuid::Uuid;

    use super::execute_refund;
    use crate::{
        config::{
            BitcoinPayoutBackend, BitcoinPayoutConfig, LightningBackend, LightningPayoutConfig,
            RefundExecutionConfig, StoreConfig,
        },
        invoice::{Refund, RefundApprovalStatus, RefundDestinationType, RefundStatus},
    };

    #[tokio::test]
    async fn executes_bitcoin_refund_with_bitcoind() {
        let server = test_bitcoind_server().await;
        let env = format!("QPAYD_TEST_BITCOIND_AUTH_{}", Uuid::new_v4().simple());
        unsafe {
            std::env::set_var(&env, "user:pass");
        }
        let store = test_store_with_bitcoin(server, env);
        let result = execute_refund(&store, &test_refund(RefundDestinationType::BitcoinUri))
            .await
            .unwrap();

        assert_eq!(result.tx_id.as_deref(), Some("bitcoin-refund-txid"));
        assert_eq!(result.payment_proof, None);
    }

    #[tokio::test]
    async fn preserves_bitcoind_json_rpc_error_on_http_failure() {
        let server = test_bitcoind_rpc_error_server().await;
        let env = format!("QPAYD_TEST_BITCOIND_AUTH_{}", Uuid::new_v4().simple());
        unsafe {
            std::env::set_var(&env, "user:pass");
        }
        let store = test_store_with_bitcoin(server, env);
        let err = execute_refund(&store, &test_refund(RefundDestinationType::BitcoinUri))
            .await
            .unwrap_err();

        assert!(err.to_string().contains("Insufficient funds"));
    }

    #[tokio::test]
    async fn executes_lightning_refund_with_barkd() {
        let server = test_barkd_server().await;
        let env = format!("QPAYD_TEST_BARKD_PAY_TOKEN_{}", Uuid::new_v4().simple());
        unsafe {
            std::env::set_var(&env, "test-token");
        }
        let store = test_store_with_lightning(server, env);
        let result = execute_refund(
            &store,
            &Refund {
                destination: Some("lnbc2500u1pwywxzwpp5jptserfk4zkc6hvfqqqsq9w".to_string()),
                destination_type: Some(RefundDestinationType::LightningInvoice),
                ..test_refund(RefundDestinationType::LightningInvoice)
            },
        )
        .await
        .unwrap();

        assert_eq!(result.tx_id.as_deref(), Some("lightning-payment-hash"));
        assert_eq!(result.payment_proof.as_deref(), Some("lightning-preimage"));
    }

    #[tokio::test]
    async fn executes_lightning_refund_with_phoenixd() {
        let server = test_phoenixd_server().await;
        let env = format!("QPAYD_TEST_PHOENIXD_PAY_TOKEN_{}", Uuid::new_v4().simple());
        unsafe {
            std::env::set_var(&env, "test-password");
        }
        let store = test_store_with_phoenixd(server, env);
        let result = execute_refund(
            &store,
            &Refund {
                destination: Some("lnbc2500u1pwywxzwpp5jptserfk4zkc6hvfqqqsq9w".to_string()),
                destination_type: Some(RefundDestinationType::LightningInvoice),
                ..test_refund(RefundDestinationType::LightningInvoice)
            },
        )
        .await
        .unwrap();

        assert_eq!(result.tx_id.as_deref(), Some("phoenix-payment-hash"));
        assert_eq!(result.payment_proof.as_deref(), Some("phoenix-preimage"));
    }

    async fn test_bitcoind_server() -> String {
        async fn send_to_address(
            headers: HeaderMap,
            Json(body): Json<serde_json::Value>,
        ) -> Result<Json<serde_json::Value>, StatusCode> {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Basic dXNlcjpwYXNz")
            );
            assert_eq!(body["method"], "sendtoaddress");
            assert_eq!(body["params"][0], "bc1qrefund");
            assert_eq!(body["params"][1], "0.00002");
            Ok(Json(serde_json::json!({
                "result": "bitcoin-refund-txid",
                "error": null,
                "id": "qpayd-refund"
            })))
        }

        let app = Router::new().route("/wallet/refunds", post(send_to_address));
        test_server(app).await
    }

    async fn test_bitcoind_rpc_error_server() -> String {
        async fn send_to_address(
            headers: HeaderMap,
            Json(body): Json<serde_json::Value>,
        ) -> impl IntoResponse {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Basic dXNlcjpwYXNz")
            );
            assert_eq!(body["method"], "sendtoaddress");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "result": null,
                    "error": {
                        "code": -6,
                        "message": "Insufficient funds"
                    },
                    "id": "qpayd-refund"
                })),
            )
        }

        let app = Router::new().route("/wallet/refunds", post(send_to_address));
        test_server(app).await
    }

    async fn test_barkd_server() -> String {
        async fn pay_invoice(
            headers: HeaderMap,
            Json(body): Json<serde_json::Value>,
        ) -> Result<Json<serde_json::Value>, StatusCode> {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer test-token")
            );
            assert_eq!(
                body["invoice"],
                "lnbc2500u1pwywxzwpp5jptserfk4zkc6hvfqqqsq9w"
            );
            Ok(Json(serde_json::json!({
                "payment_hash": "lightning-payment-hash",
                "payment_preimage": "lightning-preimage"
            })))
        }

        let app = Router::new().route("/api/v1/lightning/sends/invoice", post(pay_invoice));
        test_server(app).await
    }

    async fn test_phoenixd_server() -> String {
        async fn pay_invoice(
            headers: HeaderMap,
            body: String,
        ) -> Result<Json<serde_json::Value>, StatusCode> {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Basic OnRlc3QtcGFzc3dvcmQ=")
            );
            assert_eq!(body, "invoice=lnbc2500u1pwywxzwpp5jptserfk4zkc6hvfqqqsq9w");
            Ok(Json(serde_json::json!({
                "paymentHash": "phoenix-payment-hash",
                "preimage": "phoenix-preimage"
            })))
        }

        let app = Router::new().route("/payinvoice", post(pay_invoice));
        test_server(app).await
    }

    async fn test_server(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    fn test_store_with_bitcoin(url: String, rpc_auth_env: String) -> StoreConfig {
        let mut store = test_store();
        store.bitcoin_payout = Some(BitcoinPayoutConfig {
            backend: BitcoinPayoutBackend::Bitcoind,
            url,
            wallet: Some("refunds".to_string()),
            rpc_auth_env,
            refunds: Some(refund_execution_config()),
        });
        store
    }

    fn test_store_with_lightning(url: String, full_api_password_env: String) -> StoreConfig {
        let mut store = test_store();
        store.lightning_payout = Some(LightningPayoutConfig {
            backend: LightningBackend::Barkd,
            url,
            full_api_password_env,
            sweep: None,
            refunds: Some(refund_execution_config()),
        });
        store
    }

    fn test_store_with_phoenixd(url: String, full_api_password_env: String) -> StoreConfig {
        let mut store = test_store();
        store.lightning_payout = Some(LightningPayoutConfig {
            backend: LightningBackend::Phoenixd,
            url,
            full_api_password_env,
            sweep: None,
            refunds: Some(refund_execution_config()),
        });
        store
    }

    fn test_store() -> StoreConfig {
        StoreConfig {
            id: "main".to_string(),
            name: "Main".to_string(),
            api_token_env: Some("QPAYD_API_TOKEN".to_string()),
            admin_token_env: None,
            payout_token_env: None,
            public_allowed_origins: Vec::new(),
            admin_allowed_origins: Vec::new(),
            webhook_url: None,
            webhook_secret_env: None,
            invoice_expiry_minutes: 15,
            min_confirmations: 1,
            onchain: None,
            lightning: None,
            lightning_payout: None,
            lightning_sweep: None,
            bitcoin_payout: None,
            payment_links: Vec::new(),
        }
    }

    fn refund_execution_config() -> RefundExecutionConfig {
        RefundExecutionConfig {
            enabled: true,
            max_refund_sats: 100_000,
            daily_refund_limit_sats: 500_000,
            manual_approval_threshold_sats: None,
            poll_seconds: 30,
        }
    }

    fn test_refund(destination_type: RefundDestinationType) -> Refund {
        let now = Utc::now();
        Refund {
            id: Uuid::new_v4(),
            store_id: "main".to_string(),
            invoice_id: Uuid::new_v4(),
            status: RefundStatus::Pending,
            approval_status: RefundApprovalStatus::NotRequired,
            amount_sats: 2_000,
            destination: Some("bitcoin:bc1qrefund?amount=0.00002".to_string()),
            destination_type: Some(destination_type),
            reason: None,
            tx_id: None,
            payment_proof: None,
            failure_reason: None,
            idempotency_key: None,
            metadata: serde_json::json!({}),
            created_at: now,
            updated_at: now,
            finalized_at: None,
        }
    }
}
