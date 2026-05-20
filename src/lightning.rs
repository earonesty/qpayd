use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::{
    config::{LightningBackend, LightningConfig},
    invoice::{Invoice, InvoiceStatus},
};

#[derive(Debug, Clone)]
pub struct LightningInvoice {
    pub bolt11: String,
    pub payment_hash: Option<String>,
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

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::next_status;
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
}
