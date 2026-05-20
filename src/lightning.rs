use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::{
    config::{LightningBackend, LightningConfig, LightningSweepConfig},
    invoice::{Invoice, InvoiceStatus},
};

#[derive(Debug, Clone)]
pub struct LightningInvoice {
    pub bolt11: String,
    pub payment_hash: Option<String>,
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

pub async fn create_invoice(
    config: &LightningConfig,
    amount_sats: u64,
    description: &str,
) -> anyhow::Result<LightningInvoice> {
    match config.backend {
        LightningBackend::Phoenixd => {
            create_phoenixd_invoice(config, amount_sats, description).await
        }
    }
}

pub async fn sweep_to_address(
    config: &LightningSweepConfig,
    address: String,
) -> anyhow::Result<Option<SweepResult>> {
    match config.backend {
        LightningBackend::Phoenixd => sweep_phoenixd_to_address(config, address).await,
    }
}

pub async fn observe(config: &LightningConfig, invoice: &Invoice) -> anyhow::Result<InvoiceStatus> {
    match config.backend {
        LightningBackend::Phoenixd => observe_phoenixd_invoice(config, invoice).await,
    }
}

async fn create_phoenixd_invoice(
    config: &LightningConfig,
    amount_sats: u64,
    description: &str,
) -> anyhow::Result<LightningInvoice> {
    let password = match &config.api_password_env {
        Some(env) => Some(std::env::var(env).with_context(|| format!("missing env var {env}"))?),
        None => None,
    };
    let url = format!("{}/createinvoice", config.url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let mut request = client.post(url).form(&[
        ("amountSat", amount_sats.to_string()),
        ("description", description.to_string()),
    ]);
    if let Some(password) = password {
        request = request.basic_auth("", Some(password));
    }

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
) -> anyhow::Result<InvoiceStatus> {
    let payment_hash = invoice
        .lightning_payment_hash
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("invoice has no lightning payment hash"))?;
    let payment = get_phoenixd_incoming_payment(config, payment_hash).await?;
    Ok(next_status(
        invoice.status,
        invoice.expires_at,
        Utc::now(),
        payment.is_paid && payment.received_sat >= invoice.btc_amount_sats,
    ))
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

async fn sweep_phoenixd_to_address(
    config: &LightningSweepConfig,
    address: String,
) -> anyhow::Result<Option<SweepResult>> {
    let password = std::env::var(&config.full_api_password_env)
        .with_context(|| format!("missing env var {}", config.full_api_password_env))?;
    let client = reqwest::Client::new();
    let balance = get_phoenixd_balance(config, &client, &password).await?;
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

async fn get_phoenixd_balance(
    config: &LightningSweepConfig,
    client: &reqwest::Client,
    password: &str,
) -> anyhow::Result<PhoenixdBalanceResponse> {
    client
        .get(format!("{}/getbalance", config.url.trim_end_matches('/')))
        .basic_auth("", Some(password))
        .send()
        .await
        .context("phoenixd getbalance request failed")?
        .error_for_status()
        .context("phoenixd getbalance returned an error")?
        .json()
        .await
        .context("failed to decode phoenixd getbalance response")
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

    use super::{PhoenixdBalanceResponse, next_status, sweep_decision};
    use crate::invoice::InvoiceStatus;

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
}
