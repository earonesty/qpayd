use bitcoin::ScriptBuf;
use chrono::{DateTime, Utc};
use electrum_client::{Client, ElectrumApi};
use serde::Serialize;

use crate::invoice::{Invoice, InvoiceStatus};

#[derive(Debug, Clone, Serialize)]
pub struct OnchainObservation {
    pub server: String,
    pub confirmed_sats: u64,
    pub unconfirmed_sats: u64,
    pub next_status: InvoiceStatus,
}

pub async fn observe(servers: Vec<String>, invoice: Invoice) -> anyhow::Result<OnchainObservation> {
    tokio::task::spawn_blocking(move || observe_blocking(&servers, &invoice))
        .await
        .map_err(anyhow::Error::from)?
}

fn observe_blocking(servers: &[String], invoice: &Invoice) -> anyhow::Result<OnchainObservation> {
    if servers.is_empty() {
        anyhow::bail!("invoice has no electrum servers configured");
    }
    let script_hex = invoice
        .onchain_script_pubkey
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("invoice has no on-chain script pubkey"))?;
    let script = ScriptBuf::from_bytes(hex::decode(script_hex)?);
    let start = invoice
        .onchain_address_index
        .map(|index| index as usize)
        .unwrap_or_default();
    let mut errors = Vec::new();
    for server in rotated_servers(servers, start) {
        match observe_script(server, invoice, script.as_script()) {
            Ok(observation) => return Ok(observation),
            Err(error) => errors.push(format!("{server}: {error:#}")),
        }
    }

    anyhow::bail!(
        "all electrum servers failed for invoice {}: {}",
        invoice.id,
        errors.join("; ")
    )
}

fn observe_script(
    server: &str,
    invoice: &Invoice,
    script: &bitcoin::Script,
) -> anyhow::Result<OnchainObservation> {
    let client = Client::new(server)?;
    let balance = client.script_get_balance(script)?;
    let confirmed_sats = balance.confirmed;
    let unconfirmed_sats = balance.unconfirmed.max(0) as u64;
    Ok(OnchainObservation {
        server: server.to_string(),
        confirmed_sats,
        unconfirmed_sats,
        next_status: next_status(invoice, Utc::now(), confirmed_sats, unconfirmed_sats),
    })
}

fn rotated_servers(servers: &[String], start: usize) -> Vec<&str> {
    let len = servers.len();
    (0..len)
        .map(|offset| servers[(start + offset) % len].as_str())
        .collect()
}

pub fn next_status(
    invoice: &Invoice,
    now: DateTime<Utc>,
    confirmed_sats: u64,
    unconfirmed_sats: u64,
) -> InvoiceStatus {
    let total = confirmed_sats + unconfirmed_sats;
    if invoice.status == InvoiceStatus::Expired {
        return if total >= invoice.btc_amount_sats {
            InvoiceStatus::PaidLate
        } else {
            InvoiceStatus::Expired
        };
    }
    if now > invoice.expires_at {
        return match invoice.status {
            InvoiceStatus::New if total >= invoice.btc_amount_sats => InvoiceStatus::PaidLate,
            InvoiceStatus::New => InvoiceStatus::Expired,
            InvoiceStatus::PartiallyPaid if total >= invoice.btc_amount_sats => {
                InvoiceStatus::PaidLate
            }
            InvoiceStatus::PartiallyPaid => InvoiceStatus::Expired,
            InvoiceStatus::PaymentDetected if confirmed_sats >= invoice.btc_amount_sats => {
                InvoiceStatus::Settled
            }
            InvoiceStatus::PaymentDetected => InvoiceStatus::Expired,
            _ => invoice.status,
        };
    }
    if confirmed_sats >= invoice.btc_amount_sats {
        InvoiceStatus::Settled
    } else if total >= invoice.btc_amount_sats {
        InvoiceStatus::PaymentDetected
    } else if total > 0 {
        InvoiceStatus::PartiallyPaid
    } else {
        invoice.status
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use rust_decimal::Decimal;
    use uuid::Uuid;

    use super::{next_status, rotated_servers};
    use crate::invoice::{Invoice, InvoiceStatus};

    #[test]
    fn expires_unpaid_invoice() {
        let now = Utc::now();
        let invoice = invoice(InvoiceStatus::New, now - Duration::minutes(1));
        assert_eq!(next_status(&invoice, now, 0, 0), InvoiceStatus::Expired);
    }

    #[test]
    fn does_not_settle_expired_invoice_as_on_time_payment() {
        let now = Utc::now();
        let invoice = invoice(InvoiceStatus::Expired, now - Duration::minutes(1));
        assert_eq!(
            next_status(&invoice, now, 10_000, 0),
            InvoiceStatus::PaidLate
        );
    }

    #[test]
    fn first_seen_full_payment_after_expiry_is_late() {
        let now = Utc::now();
        let invoice = invoice(InvoiceStatus::New, now - Duration::minutes(1));
        assert_eq!(
            next_status(&invoice, now, 10_000, 0),
            InvoiceStatus::PaidLate
        );
    }

    #[test]
    fn detected_payment_can_settle_after_expiry() {
        let now = Utc::now();
        let invoice = invoice(InvoiceStatus::PaymentDetected, now - Duration::minutes(1));
        assert_eq!(
            next_status(&invoice, now, 10_000, 0),
            InvoiceStatus::Settled
        );
    }

    #[test]
    fn waits_for_confirmation_before_settled() {
        let now = Utc::now();
        let invoice = invoice(InvoiceStatus::New, now + Duration::minutes(15));
        assert_eq!(
            next_status(&invoice, now, 0, 10_000),
            InvoiceStatus::PaymentDetected
        );
        assert_eq!(
            next_status(&invoice, now, 10_000, 0),
            InvoiceStatus::Settled
        );
    }

    #[test]
    fn reports_partial_and_overpaid_statuses() {
        let now = Utc::now();
        let invoice = invoice(InvoiceStatus::New, now + Duration::minutes(15));
        assert_eq!(
            next_status(&invoice, now, 0, 5_000),
            InvoiceStatus::PartiallyPaid
        );
        assert_eq!(
            next_status(&invoice, now, 12_000, 0),
            InvoiceStatus::Settled
        );
    }

    #[test]
    fn rotates_electrum_servers_by_address_index() {
        let servers = vec![
            "ssl://one.example:50002".to_string(),
            "ssl://two.example:50002".to_string(),
            "ssl://three.example:50002".to_string(),
        ];
        assert_eq!(
            rotated_servers(&servers, 0),
            vec![
                "ssl://one.example:50002",
                "ssl://two.example:50002",
                "ssl://three.example:50002"
            ]
        );
        assert_eq!(
            rotated_servers(&servers, 1),
            vec![
                "ssl://two.example:50002",
                "ssl://three.example:50002",
                "ssl://one.example:50002"
            ]
        );
        assert_eq!(
            rotated_servers(&servers, 5),
            vec![
                "ssl://three.example:50002",
                "ssl://one.example:50002",
                "ssl://two.example:50002"
            ]
        );
    }

    fn invoice(status: InvoiceStatus, expires_at: chrono::DateTime<Utc>) -> Invoice {
        Invoice {
            id: Uuid::new_v4(),
            store_id: "main".to_string(),
            status,
            amount: Decimal::from(1),
            currency: "USD".to_string(),
            btc_amount_sats: 10_000,
            paid_sats: 0,
            confirmed_sats: 0,
            unconfirmed_sats: 0,
            onchain_address: Some("bc1qexample".to_string()),
            onchain_address_index: Some(0),
            onchain_script_pubkey: Some("0014".to_string()),
            lightning_bolt11: None,
            lightning_payment_hash: None,
            idempotency_key: None,
            payment_link_id: None,
            rate_source: "test".to_string(),
            rate: Decimal::from(100_000),
            metadata: serde_json::json!({}),
            expires_at,
            created_at: expires_at,
            updated_at: expires_at,
        }
    }
}
