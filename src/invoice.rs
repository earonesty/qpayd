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
    pub paid_sats: u64,
    pub confirmed_sats: u64,
    pub unconfirmed_sats: u64,
    pub onchain_address: Option<String>,
    pub onchain_address_index: Option<u32>,
    pub onchain_script_pubkey: Option<String>,
    pub lightning_bolt11: Option<String>,
    pub lightning_payment_hash: Option<String>,
    pub idempotency_key: Option<String>,
    #[serde(skip)]
    pub payment_link_id: Option<String>,
    pub rate_source: String,
    pub rate: Decimal,
    pub metadata: serde_json::Value,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaymentAmounts {
    pub paid_sats: u64,
    pub confirmed_sats: u64,
    pub unconfirmed_sats: u64,
}

impl PaymentAmounts {
    pub fn from_invoice(invoice: &Invoice) -> Self {
        Self {
            paid_sats: invoice.paid_sats,
            confirmed_sats: invoice.confirmed_sats,
            unconfirmed_sats: invoice.unconfirmed_sats,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvoiceStatusUpdate {
    pub status: InvoiceStatus,
    pub payment: PaymentAmounts,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Refund {
    pub id: Uuid,
    pub store_id: String,
    pub invoice_id: Uuid,
    pub status: RefundStatus,
    pub approval_status: RefundApprovalStatus,
    pub amount_sats: u64,
    pub destination: Option<String>,
    pub destination_type: Option<RefundDestinationType>,
    pub reason: Option<String>,
    pub tx_id: Option<String>,
    pub payment_proof: Option<String>,
    pub failure_reason: Option<String>,
    pub idempotency_key: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finalized_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefundApprovalStatus {
    NotRequired,
    Pending,
    Approved,
}

impl RefundApprovalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::Pending => "pending",
            Self::Approved => "approved",
        }
    }

    pub fn allows_execution(self) -> bool {
        matches!(self, Self::NotRequired | Self::Approved)
    }
}

impl TryFrom<&str> for RefundApprovalStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "not_required" => Ok(Self::NotRequired),
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            other => anyhow::bail!("invalid refund approval status {other}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefundStatus {
    Pending,
    Processing,
    Succeeded,
    Canceled,
    Failed,
}

impl RefundStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Processing => "processing",
            Self::Succeeded => "succeeded",
            Self::Canceled => "canceled",
            Self::Failed => "failed",
        }
    }
}

impl TryFrom<&str> for RefundStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "pending" => Ok(Self::Pending),
            "processing" => Ok(Self::Processing),
            "succeeded" => Ok(Self::Succeeded),
            "canceled" => Ok(Self::Canceled),
            "failed" => Ok(Self::Failed),
            other => anyhow::bail!("invalid refund status {other}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefundDestinationType {
    BitcoinAddress,
    BitcoinUri,
    LightningInvoice,
    Lnurl,
    Unknown,
}

impl RefundDestinationType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BitcoinAddress => "bitcoin_address",
            Self::BitcoinUri => "bitcoin_uri",
            Self::LightningInvoice => "lightning_invoice",
            Self::Lnurl => "lnurl",
            Self::Unknown => "unknown",
        }
    }
}

impl TryFrom<&str> for RefundDestinationType {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "bitcoin_address" => Ok(Self::BitcoinAddress),
            "bitcoin_uri" => Ok(Self::BitcoinUri),
            "lightning_invoice" => Ok(Self::LightningInvoice),
            "lnurl" => Ok(Self::Lnurl),
            "unknown" => Ok(Self::Unknown),
            other => anyhow::bail!("invalid refund destination type {other}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightningSweepRecord {
    pub id: Uuid,
    pub store_id: String,
    pub backend: String,
    pub status: SweepStatus,
    pub balance_sats: u64,
    pub amount_sats: u64,
    pub address: String,
    pub tx_id: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepStatus {
    Succeeded,
    Skipped,
    Failed,
}

impl SweepStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

impl TryFrom<&str> for SweepStatus {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "succeeded" => Ok(Self::Succeeded),
            "skipped" => Ok(Self::Skipped),
            "failed" => Ok(Self::Failed),
            other => anyhow::bail!("invalid sweep status {other}"),
        }
    }
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
