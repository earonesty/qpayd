use anyhow::Context;
use serde::Deserialize;

use crate::config::{LightningBackend, LightningConfig};

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhoenixdInvoiceResponse {
    serialized: String,
    payment_hash: Option<String>,
}
