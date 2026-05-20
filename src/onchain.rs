use bitcoin::ScriptBuf;
use electrum_client::{Client, ElectrumApi};
use serde::Serialize;

use crate::invoice::{Invoice, InvoiceStatus};

#[derive(Debug, Clone, Serialize)]
pub struct OnchainObservation {
    pub confirmed_sats: u64,
    pub unconfirmed_sats: u64,
    pub next_status: InvoiceStatus,
}

pub async fn observe(server: String, invoice: Invoice) -> anyhow::Result<OnchainObservation> {
    tokio::task::spawn_blocking(move || observe_blocking(&server, &invoice))
        .await
        .map_err(anyhow::Error::from)?
}

fn observe_blocking(server: &str, invoice: &Invoice) -> anyhow::Result<OnchainObservation> {
    let script_hex = invoice
        .onchain_script_pubkey
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("invoice has no on-chain script pubkey"))?;
    let script = ScriptBuf::from_bytes(hex::decode(script_hex)?);
    let client = Client::new(server)?;
    let balance = client.script_get_balance(script.as_script())?;
    let confirmed_sats = balance.confirmed as u64;
    let unconfirmed_sats = balance.unconfirmed.max(0) as u64;
    let total = confirmed_sats + unconfirmed_sats;
    let next_status = if confirmed_sats >= invoice.btc_amount_sats {
        InvoiceStatus::Settled
    } else if total >= invoice.btc_amount_sats {
        InvoiceStatus::PaymentDetected
    } else if total > 0 {
        InvoiceStatus::PartiallyPaid
    } else {
        invoice.status
    };

    Ok(OnchainObservation {
        confirmed_sats,
        unconfirmed_sats,
        next_status,
    })
}
