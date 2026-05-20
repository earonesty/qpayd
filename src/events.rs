use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::invoice::{Invoice, InvoiceStatus};

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

fn invoice_event(invoice: &Invoice, event_type: &str, created_at: DateTime<Utc>) -> EventEnvelope {
    EventEnvelope {
        id: invoice_event_id(invoice.id, event_type),
        event_type: event_type.to_string(),
        store_id: invoice.store_id.clone(),
        invoice_id: Some(invoice.id),
        data: serde_json::to_value(invoice).expect("invoice serializes"),
        created_at,
    }
}

fn invoice_event_id(invoice_id: Uuid, event_type: &str) -> String {
    format!("evt_{}_{}", invoice_id, event_type.replace('.', "_"))
}
