use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::invoice::{Invoice, InvoiceStatus, Refund};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub store_id: String,
    pub invoice_id: Option<Uuid>,
    pub data: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct QueuedWebhookDelivery {
    pub id: i64,
    pub event: EventEnvelope,
    pub url: String,
    pub attempts: u32,
}

pub fn invoice_created_event(invoice: &Invoice, created_at: DateTime<Utc>) -> EventEnvelope {
    invoice_event(invoice, "invoice.created", created_at)
}

pub fn invoice_status_event(
    invoice: &Invoice,
    status: InvoiceStatus,
    created_at: DateTime<Utc>,
) -> EventEnvelope {
    let mut data = invoice.clone();
    data.status = status;
    data.updated_at = created_at;
    invoice_event(&data, &format!("invoice.{}", status.as_str()), created_at)
}

pub fn refund_created_event(refund: &Refund, created_at: DateTime<Utc>) -> EventEnvelope {
    refund_event(refund, "refund.created", created_at)
}

pub fn refund_approved_event(refund: &Refund, created_at: DateTime<Utc>) -> EventEnvelope {
    refund_event(refund, "refund.approved", created_at)
}

pub fn refund_processing_event(refund: &Refund, created_at: DateTime<Utc>) -> EventEnvelope {
    refund_event(refund, "refund.processing", created_at)
}

pub fn refund_retry_event(refund: &Refund, created_at: DateTime<Utc>) -> EventEnvelope {
    refund_event(refund, "refund.retry", created_at)
}

pub fn refund_finalized_event(refund: &Refund, created_at: DateTime<Utc>) -> EventEnvelope {
    refund_event(refund, "refund.finalized", created_at)
}

pub fn refund_canceled_event(refund: &Refund, created_at: DateTime<Utc>) -> EventEnvelope {
    refund_event(refund, "refund.canceled", created_at)
}

pub fn refund_failed_event(refund: &Refund, created_at: DateTime<Utc>) -> EventEnvelope {
    refund_event(refund, "refund.failed", created_at)
}

fn invoice_event(invoice: &Invoice, event_type: &str, created_at: DateTime<Utc>) -> EventEnvelope {
    let mut data = serde_json::to_value(invoice).expect("invoice serializes");
    if let Some(object) = data.as_object_mut() {
        object.insert(
            "remaining_sats".to_string(),
            serde_json::json!(invoice.btc_amount_sats.saturating_sub(invoice.paid_sats)),
        );
        object.insert(
            "overpaid_sats".to_string(),
            serde_json::json!(invoice.paid_sats.saturating_sub(invoice.btc_amount_sats)),
        );
    }
    EventEnvelope {
        id: invoice_event_id(invoice.id, event_type),
        event_type: event_type.to_string(),
        store_id: invoice.store_id.clone(),
        invoice_id: Some(invoice.id),
        data,
        created_at,
    }
}

fn invoice_event_id(invoice_id: Uuid, event_type: &str) -> String {
    format!("evt_{}_{}", invoice_id, event_type.replace('.', "_"))
}

fn refund_event(refund: &Refund, event_type: &str, created_at: DateTime<Utc>) -> EventEnvelope {
    EventEnvelope {
        id: format!("evt_{}_{}", refund.id, event_type.replace('.', "_")),
        event_type: event_type.to_string(),
        store_id: refund.store_id.clone(),
        invoice_id: Some(refund.invoice_id),
        data: serde_json::to_value(refund).expect("refund serializes"),
        created_at,
    }
}
