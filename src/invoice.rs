use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invoice {
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
    pub rate_source: String,
    pub rate: Decimal,
    pub metadata: serde_json::Value,
    pub checkout_url: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceStatus {
    New,
    PaymentDetected,
    PartiallyPaid,
    Settled,
    Expired,
    PaidLate,
    Invalid,
}

impl InvoiceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::PaymentDetected => "payment_detected",
            Self::PartiallyPaid => "partially_paid",
            Self::Settled => "settled",
            Self::Expired => "expired",
            Self::PaidLate => "paid_late",
            Self::Invalid => "invalid",
        }
    }
}

impl TryFrom<&str> for InvoiceStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "new" => Ok(Self::New),
            "payment_detected" => Ok(Self::PaymentDetected),
            "partially_paid" => Ok(Self::PartiallyPaid),
            "settled" => Ok(Self::Settled),
            "expired" => Ok(Self::Expired),
            "paid_late" => Ok(Self::PaidLate),
            "invalid" => Ok(Self::Invalid),
            other => anyhow::bail!("invalid invoice status {other}"),
        }
    }
}
