use std::{str::FromStr, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{
    PgPool, Row, SqlitePool,
    postgres::PgPoolOptions,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use uuid::Uuid;

use crate::{
    events::{EventEnvelope, QueuedWebhookDelivery},
    invoice::{
        Invoice, InvoiceStatus, InvoiceStatusUpdate, LightningSweepRecord, PaymentAmounts, Refund,
        RefundApprovalStatus, RefundDestinationType, RefundStatus, SweepStatus,
    },
};

#[derive(Debug, Clone, Default)]
pub struct InvoiceListFilter {
    pub status: Option<InvoiceStatus>,
    pub limit: u32,
}

#[derive(Debug, Clone)]
pub struct RefundExecutionCandidate {
    pub refund: Refund,
    pub invoice: Invoice,
}

#[async_trait]
pub trait Store: Send + Sync {
    async fn migrate(&self) -> anyhow::Result<()>;
    async fn reserve_address_index(&self, store_id: &str) -> anyhow::Result<u32>;
    async fn insert_invoice(
        &self,
        invoice: &Invoice,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<()>;
    async fn invoice(&self, store_id: &str, id: Uuid) -> anyhow::Result<Option<Invoice>>;
    async fn invoices(
        &self,
        store_id: &str,
        filter: InvoiceListFilter,
    ) -> anyhow::Result<Vec<Invoice>>;
    async fn invoice_by_idempotency_key(
        &self,
        store_id: &str,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<Invoice>>;
    async fn invoice_by_payment_link_idempotency_key(
        &self,
        store_id: &str,
        payment_link_id: &str,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<Invoice>>;
    async fn active_onchain_invoices(&self, store_id: &str) -> anyhow::Result<Vec<Invoice>>;
    async fn active_lightning_invoices(&self, store_id: &str) -> anyhow::Result<Vec<Invoice>>;
    async fn expirable_invoices(
        &self,
        store_id: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Vec<Invoice>>;
    async fn update_invoice_status(
        &self,
        store_id: &str,
        id: Uuid,
        update: InvoiceStatusUpdate,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<()>;
    async fn update_invoice_payment_amounts(
        &self,
        store_id: &str,
        id: Uuid,
        payment: PaymentAmounts,
        updated_at: DateTime<Utc>,
    ) -> anyhow::Result<()>;
    async fn events(&self, store_id: &str, limit: u32) -> anyhow::Result<Vec<EventEnvelope>>;
    async fn event(&self, store_id: &str, event_id: &str) -> anyhow::Result<Option<EventEnvelope>>;
    async fn enqueue_webhook_delivery(
        &self,
        event_id: &str,
        store_id: &str,
        url: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()>;
    async fn due_webhook_deliveries(
        &self,
        limit: u32,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Vec<QueuedWebhookDelivery>>;
    async fn mark_webhook_delivered(
        &self,
        id: i64,
        delivered_at: DateTime<Utc>,
    ) -> anyhow::Result<()>;
    async fn mark_webhook_failed(
        &self,
        id: i64,
        attempts: u32,
        next_attempt_at: DateTime<Utc>,
        error: &str,
        updated_at: DateTime<Utc>,
    ) -> anyhow::Result<()>;
    async fn insert_refund(
        &self,
        refund: &Refund,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<()>;
    async fn refund(&self, store_id: &str, id: Uuid) -> anyhow::Result<Option<Refund>>;
    async fn refund_by_idempotency_key(
        &self,
        store_id: &str,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<Refund>>;
    async fn refunds(&self, store_id: &str, limit: u32) -> anyhow::Result<Vec<Refund>>;
    async fn refunds_for_invoice(
        &self,
        store_id: &str,
        invoice_id: Uuid,
    ) -> anyhow::Result<Vec<Refund>>;
    async fn pending_refund_executions(
        &self,
        store_id: &str,
        limit: u32,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Vec<RefundExecutionCandidate>>;
    async fn claim_refund_execution(
        &self,
        store_id: &str,
        refund_id: Uuid,
        worker_id: &str,
        lease_until: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Option<RefundExecutionCandidate>>;
    async fn try_start_refund_execution(
        &self,
        refund: &Refund,
        day_start: DateTime<Utc>,
        daily_limit_sats: u64,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<bool>;
    async fn update_refund_approval(
        &self,
        refund: &Refund,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<bool>;
    async fn update_refund_status(
        &self,
        refund: &Refund,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<()>;
    async fn insert_lightning_sweep(&self, sweep: &LightningSweepRecord) -> anyhow::Result<()>;
    async fn lightning_sweeps(
        &self,
        store_id: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<LightningSweepRecord>>;
}

#[derive(Debug)]
pub struct SqliteStore {
    pool: SqlitePool,
}

#[derive(Debug)]
pub struct PostgresStore {
    pool: PgPool,
}

pub async fn connect_store(url: &str) -> anyhow::Result<Arc<dyn Store>> {
    if url.starts_with("sqlite:") {
        Ok(Arc::new(SqliteStore::connect(url).await?))
    } else if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        Ok(Arc::new(PostgresStore::connect(url).await?))
    } else {
        anyhow::bail!("database url must start with sqlite:, postgres://, or postgresql://")
    }
}

impl SqliteStore {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let options = SqliteConnectOptions::from_str(url)?.create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }
}

impl PostgresStore {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::new().max_connections(5).connect(url).await?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn migrate(&self) -> anyhow::Result<()> {
        migrate_sqlite(&self.pool).await
    }

    async fn reserve_address_index(&self, store_id: &str) -> anyhow::Result<u32> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO store_counters (store_id, next_onchain_index)
            VALUES (?, 0)
            ON CONFLICT(store_id) DO NOTHING
            "#,
        )
        .bind(store_id)
        .execute(&mut *tx)
        .await?;

        let row = sqlx::query(
            r#"
            SELECT next_onchain_index
            FROM store_counters
            WHERE store_id = ?
            "#,
        )
        .bind(store_id)
        .fetch_one(&mut *tx)
        .await?;
        let index = row.get::<i64, _>("next_onchain_index");

        sqlx::query(
            r#"
            UPDATE store_counters
            SET next_onchain_index = next_onchain_index + 1
            WHERE store_id = ?
            "#,
        )
        .bind(store_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(index as u32)
    }

    async fn insert_invoice(
        &self,
        invoice: &Invoice,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO invoices (
                id, store_id, status, amount, currency, btc_amount_sats,
                paid_sats, confirmed_sats, unconfirmed_sats,
                onchain_address, onchain_address_index, onchain_script_pubkey,
                lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, rate_source, rate,
                metadata, expires_at, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(invoice.id.to_string())
        .bind(&invoice.store_id)
        .bind(invoice.status.as_str())
        .bind(invoice.amount.to_string())
        .bind(&invoice.currency)
        .bind(invoice.btc_amount_sats as i64)
        .bind(invoice.paid_sats as i64)
        .bind(invoice.confirmed_sats as i64)
        .bind(invoice.unconfirmed_sats as i64)
        .bind(&invoice.onchain_address)
        .bind(invoice.onchain_address_index.map(|index| index as i64))
        .bind(&invoice.onchain_script_pubkey)
        .bind(&invoice.lightning_bolt11)
        .bind(&invoice.lightning_payment_hash)
        .bind(&invoice.idempotency_key)
        .bind(&invoice.payment_link_id)
        .bind(&invoice.rate_source)
        .bind(invoice.rate.to_string())
        .bind(invoice.metadata.to_string())
        .bind(invoice.expires_at.to_rfc3339())
        .bind(invoice.created_at.to_rfc3339())
        .bind(invoice.updated_at.to_rfc3339())
        .execute(&mut *tx)
        .await?;

        insert_event_query(event).execute(&mut *tx).await?;
        if let Some(url) = webhook_url {
            enqueue_webhook_query(&event.id, &event.store_id, url, event.created_at)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;

        Ok(())
    }

    async fn invoice(&self, store_id: &str, id: Uuid) -> anyhow::Result<Option<Invoice>> {
        let Some(row) = sqlx::query(
            r#"
            SELECT id, store_id, status, amount, currency, btc_amount_sats,
                   paid_sats, confirmed_sats, unconfirmed_sats,
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                   expires_at, created_at, updated_at
            FROM invoices
            WHERE store_id = ? AND id = ?
            "#,
        )
        .bind(store_id)
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        Ok(Some(invoice_from_row(row)?))
    }

    async fn invoices(
        &self,
        store_id: &str,
        filter: InvoiceListFilter,
    ) -> anyhow::Result<Vec<Invoice>> {
        let limit = i64::from(filter.limit.clamp(1, 200));
        let rows = if let Some(status) = filter.status {
            sqlx::query(
                r#"
                SELECT id, store_id, status, amount, currency, btc_amount_sats,
                       paid_sats, confirmed_sats, unconfirmed_sats,
                       onchain_address, onchain_address_index, onchain_script_pubkey,
                       rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                       expires_at, created_at, updated_at
                FROM invoices
                WHERE store_id = ? AND status = ?
                ORDER BY created_at DESC
                LIMIT ?
                "#,
            )
            .bind(store_id)
            .bind(status.as_str())
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT id, store_id, status, amount, currency, btc_amount_sats,
                       paid_sats, confirmed_sats, unconfirmed_sats,
                       onchain_address, onchain_address_index, onchain_script_pubkey,
                       rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                       expires_at, created_at, updated_at
                FROM invoices
                WHERE store_id = ?
                ORDER BY created_at DESC
                LIMIT ?
                "#,
            )
            .bind(store_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };

        rows.into_iter().map(invoice_from_row).collect()
    }

    async fn invoice_by_idempotency_key(
        &self,
        store_id: &str,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<Invoice>> {
        let Some(row) = sqlx::query(
            r#"
            SELECT id, store_id, status, amount, currency, btc_amount_sats,
                   paid_sats, confirmed_sats, unconfirmed_sats,
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                   expires_at, created_at, updated_at
            FROM invoices
            WHERE store_id = ? AND idempotency_key = ? AND payment_link_id IS NULL
            "#,
        )
        .bind(store_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        Ok(Some(invoice_from_row(row)?))
    }

    async fn invoice_by_payment_link_idempotency_key(
        &self,
        store_id: &str,
        payment_link_id: &str,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<Invoice>> {
        let Some(row) = sqlx::query(
            r#"
            SELECT id, store_id, status, amount, currency, btc_amount_sats,
                   paid_sats, confirmed_sats, unconfirmed_sats,
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                   expires_at, created_at, updated_at
            FROM invoices
            WHERE store_id = ? AND payment_link_id = ? AND idempotency_key = ?
            "#,
        )
        .bind(store_id)
        .bind(payment_link_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        Ok(Some(invoice_from_row(row)?))
    }

    async fn active_onchain_invoices(&self, store_id: &str) -> anyhow::Result<Vec<Invoice>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, status, amount, currency, btc_amount_sats,
                   paid_sats, confirmed_sats, unconfirmed_sats,
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                   expires_at, created_at, updated_at
            FROM invoices
            WHERE store_id = ?
              AND onchain_script_pubkey IS NOT NULL
              AND status IN ('new', 'payment_detected', 'partially_paid', 'expired')
            "#,
        )
        .bind(store_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(invoice_from_row).collect()
    }

    async fn active_lightning_invoices(&self, store_id: &str) -> anyhow::Result<Vec<Invoice>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, status, amount, currency, btc_amount_sats,
                   paid_sats, confirmed_sats, unconfirmed_sats,
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                   expires_at, created_at, updated_at
            FROM invoices
            WHERE store_id = ?
              AND lightning_payment_hash IS NOT NULL
              AND status IN ('new', 'payment_detected', 'partially_paid', 'expired')
            "#,
        )
        .bind(store_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(invoice_from_row).collect()
    }

    async fn expirable_invoices(
        &self,
        store_id: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Vec<Invoice>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, status, amount, currency, btc_amount_sats,
                   paid_sats, confirmed_sats, unconfirmed_sats,
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                   expires_at, created_at, updated_at
            FROM invoices
            WHERE store_id = ?
              AND status IN ('new', 'partially_paid')
              AND expires_at <= ?
            "#,
        )
        .bind(store_id)
        .bind(now.to_rfc3339())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(invoice_from_row).collect()
    }

    async fn update_invoice_status(
        &self,
        store_id: &str,
        id: Uuid,
        update: InvoiceStatusUpdate,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            UPDATE invoices
            SET status = ?,
                paid_sats = ?,
                confirmed_sats = ?,
                unconfirmed_sats = ?,
                updated_at = ?
            WHERE store_id = ? AND id = ?
            "#,
        )
        .bind(update.status.as_str())
        .bind(update.payment.paid_sats as i64)
        .bind(update.payment.confirmed_sats as i64)
        .bind(update.payment.unconfirmed_sats as i64)
        .bind(update.updated_at.to_rfc3339())
        .bind(store_id)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;

        let inserted = insert_event_query(event)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            > 0;
        if inserted && let Some(url) = webhook_url {
            enqueue_webhook_query(&event.id, &event.store_id, url, event.created_at)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;

        Ok(())
    }

    async fn update_invoice_payment_amounts(
        &self,
        store_id: &str,
        id: Uuid,
        payment: PaymentAmounts,
        updated_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            UPDATE invoices
            SET paid_sats = ?,
                confirmed_sats = ?,
                unconfirmed_sats = ?,
                updated_at = ?
            WHERE store_id = ? AND id = ?
            "#,
        )
        .bind(payment.paid_sats as i64)
        .bind(payment.confirmed_sats as i64)
        .bind(payment.unconfirmed_sats as i64)
        .bind(updated_at.to_rfc3339())
        .bind(store_id)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn events(&self, store_id: &str, limit: u32) -> anyhow::Result<Vec<EventEnvelope>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, invoice_id, type, payload_json, created_at
            FROM events
            WHERE store_id = ?
            ORDER BY created_at DESC
            LIMIT ?
            "#,
        )
        .bind(store_id)
        .bind(i64::from(limit.min(200)))
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(event_from_row).collect()
    }

    async fn event(&self, store_id: &str, event_id: &str) -> anyhow::Result<Option<EventEnvelope>> {
        let Some(row) = sqlx::query(
            r#"
            SELECT id, store_id, invoice_id, type, payload_json, created_at
            FROM events
            WHERE store_id = ? AND id = ?
            "#,
        )
        .bind(store_id)
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        Ok(Some(event_from_row(row)?))
    }

    async fn enqueue_webhook_delivery(
        &self,
        event_id: &str,
        store_id: &str,
        url: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        enqueue_webhook_query(event_id, store_id, url, now)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn due_webhook_deliveries(
        &self,
        limit: u32,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Vec<QueuedWebhookDelivery>> {
        let rows = sqlx::query(
            r#"
            SELECT
                wd.id AS delivery_id,
                wd.url AS delivery_url,
                wd.attempts AS delivery_attempts,
                e.id, e.store_id, e.invoice_id, e.type, e.payload_json, e.created_at
            FROM webhook_deliveries wd
            JOIN events e ON e.id = wd.event_id
            WHERE wd.status = 'pending' AND wd.next_attempt_at <= ?
            ORDER BY wd.next_attempt_at ASC, wd.id ASC
            LIMIT ?
            "#,
        )
        .bind(now.to_rfc3339())
        .bind(i64::from(limit.min(100)))
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(webhook_delivery_from_row).collect()
    }

    async fn mark_webhook_delivered(
        &self,
        id: i64,
        delivered_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            UPDATE webhook_deliveries
            SET status = 'delivered',
                delivered_at = ?,
                updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(delivered_at.to_rfc3339())
        .bind(delivered_at.to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mark_webhook_failed(
        &self,
        id: i64,
        attempts: u32,
        next_attempt_at: DateTime<Utc>,
        error: &str,
        updated_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            UPDATE webhook_deliveries
            SET attempts = ?,
                next_attempt_at = ?,
                last_error = ?,
                updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(i64::from(attempts))
        .bind(next_attempt_at.to_rfc3339())
        .bind(error)
        .bind(updated_at.to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn insert_refund(
        &self,
        refund: &Refund,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO refunds (
                id, store_id, invoice_id, status, approval_status, amount_sats, destination, destination_type,
                reason, tx_id, payment_proof, failure_reason, idempotency_key, metadata, created_at, updated_at, finalized_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(refund.id.to_string())
        .bind(&refund.store_id)
        .bind(refund.invoice_id.to_string())
        .bind(refund.status.as_str())
        .bind(refund.approval_status.as_str())
        .bind(refund.amount_sats as i64)
        .bind(&refund.destination)
        .bind(refund.destination_type.map(|value| value.as_str()))
        .bind(&refund.reason)
        .bind(&refund.tx_id)
        .bind(&refund.payment_proof)
        .bind(&refund.failure_reason)
        .bind(&refund.idempotency_key)
        .bind(refund.metadata.to_string())
        .bind(refund.created_at.to_rfc3339())
        .bind(refund.updated_at.to_rfc3339())
        .bind(refund.finalized_at.map(|time| time.to_rfc3339()))
        .execute(&mut *tx)
        .await?;
        insert_event_query(event).execute(&mut *tx).await?;
        if let Some(url) = webhook_url {
            enqueue_webhook_query(&event.id, &event.store_id, url, event.created_at)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn refund(&self, store_id: &str, id: Uuid) -> anyhow::Result<Option<Refund>> {
        let Some(row) = sqlx::query(
            r#"
            SELECT id, store_id, invoice_id, status, approval_status, amount_sats, destination, destination_type,
                   reason, tx_id, payment_proof, failure_reason, idempotency_key, metadata, created_at, updated_at, finalized_at
            FROM refunds
            WHERE store_id = ? AND id = ?
            "#,
        )
        .bind(store_id)
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        Ok(Some(refund_from_row(row)?))
    }

    async fn refund_by_idempotency_key(
        &self,
        store_id: &str,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<Refund>> {
        let Some(row) = sqlx::query(
            r#"
            SELECT id, store_id, invoice_id, status, approval_status, amount_sats, destination, destination_type,
                   reason, tx_id, payment_proof, failure_reason, idempotency_key, metadata, created_at, updated_at, finalized_at
            FROM refunds
            WHERE store_id = ? AND idempotency_key = ?
            "#,
        )
        .bind(store_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        Ok(Some(refund_from_row(row)?))
    }

    async fn refunds(&self, store_id: &str, limit: u32) -> anyhow::Result<Vec<Refund>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, invoice_id, status, approval_status, amount_sats, destination, destination_type,
                   reason, tx_id, payment_proof, failure_reason, idempotency_key, metadata, created_at, updated_at, finalized_at
            FROM refunds
            WHERE store_id = ?
            ORDER BY created_at DESC
            LIMIT ?
            "#,
        )
        .bind(store_id)
        .bind(i64::from(limit.clamp(1, 200)))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(refund_from_row).collect()
    }

    async fn refunds_for_invoice(
        &self,
        store_id: &str,
        invoice_id: Uuid,
    ) -> anyhow::Result<Vec<Refund>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, invoice_id, status, approval_status, amount_sats, destination, destination_type,
                   reason, tx_id, payment_proof, failure_reason, idempotency_key, metadata, created_at, updated_at, finalized_at
            FROM refunds
            WHERE store_id = ? AND invoice_id = ?
            ORDER BY created_at DESC
            "#,
        )
        .bind(store_id)
        .bind(invoice_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(refund_from_row).collect()
    }

    async fn pending_refund_executions(
        &self,
        store_id: &str,
        limit: u32,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Vec<RefundExecutionCandidate>> {
        let rows = sqlx::query(
            r#"
            SELECT
                r.id AS refund_id, r.store_id AS refund_store_id, r.invoice_id AS refund_invoice_id,
                r.status AS refund_status, r.approval_status AS refund_approval_status, r.amount_sats AS refund_amount_sats,
                r.destination AS refund_destination, r.destination_type AS refund_destination_type,
                r.reason AS refund_reason, r.tx_id AS refund_tx_id, r.payment_proof AS refund_payment_proof,
                r.failure_reason AS refund_failure_reason, r.idempotency_key AS refund_idempotency_key,
                r.metadata AS refund_metadata, r.created_at AS refund_created_at,
                r.updated_at AS refund_updated_at, r.finalized_at AS refund_finalized_at,
                i.id AS invoice_id, i.store_id AS invoice_store_id, i.status AS invoice_status,
                i.amount AS invoice_amount, i.currency AS invoice_currency,
                i.btc_amount_sats AS invoice_btc_amount_sats, i.paid_sats AS invoice_paid_sats,
                i.confirmed_sats AS invoice_confirmed_sats, i.unconfirmed_sats AS invoice_unconfirmed_sats,
                i.onchain_address AS invoice_onchain_address, i.onchain_address_index AS invoice_onchain_address_index,
                i.onchain_script_pubkey AS invoice_onchain_script_pubkey,
                i.lightning_bolt11 AS invoice_lightning_bolt11,
                i.lightning_payment_hash AS invoice_lightning_payment_hash,
                i.idempotency_key AS invoice_idempotency_key, i.payment_link_id AS invoice_payment_link_id,
                i.rate_source AS invoice_rate_source, i.rate AS invoice_rate, i.metadata AS invoice_metadata,
                i.expires_at AS invoice_expires_at, i.created_at AS invoice_created_at,
                i.updated_at AS invoice_updated_at
            FROM refunds r
            JOIN invoices i ON i.store_id = r.store_id AND i.id = r.invoice_id
            WHERE r.store_id = ?
              AND r.status = 'pending'
              AND r.approval_status IN ('not_required', 'approved')
              AND r.destination IS NOT NULL
              AND (r.execution_lease_until IS NULL OR r.execution_lease_until <= ?)
            ORDER BY r.created_at ASC
            LIMIT ?
            "#,
        )
        .bind(store_id)
        .bind(now.to_rfc3339())
        .bind(i64::from(limit.clamp(1, 200)))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(refund_execution_candidate_from_row)
            .collect()
    }

    async fn claim_refund_execution(
        &self,
        store_id: &str,
        refund_id: Uuid,
        worker_id: &str,
        lease_until: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Option<RefundExecutionCandidate>> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"
            UPDATE refunds
            SET execution_claimed_by = ?,
                execution_lease_until = ?,
                updated_at = ?
            WHERE store_id = ?
              AND id = ?
              AND status = 'pending'
              AND approval_status IN ('not_required', 'approved')
              AND destination IS NOT NULL
              AND (execution_lease_until IS NULL OR execution_lease_until <= ?)
            "#,
        )
        .bind(worker_id)
        .bind(lease_until.to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(store_id)
        .bind(refund_id.to_string())
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(None);
        }
        let row = sqlx::query(
            r#"
            SELECT
                r.id AS refund_id, r.store_id AS refund_store_id, r.invoice_id AS refund_invoice_id,
                r.status AS refund_status, r.approval_status AS refund_approval_status, r.amount_sats AS refund_amount_sats,
                r.destination AS refund_destination, r.destination_type AS refund_destination_type,
                r.reason AS refund_reason, r.tx_id AS refund_tx_id, r.payment_proof AS refund_payment_proof,
                r.failure_reason AS refund_failure_reason, r.idempotency_key AS refund_idempotency_key,
                r.metadata AS refund_metadata, r.created_at AS refund_created_at,
                r.updated_at AS refund_updated_at, r.finalized_at AS refund_finalized_at,
                i.id AS invoice_id, i.store_id AS invoice_store_id, i.status AS invoice_status,
                i.amount AS invoice_amount, i.currency AS invoice_currency,
                i.btc_amount_sats AS invoice_btc_amount_sats, i.paid_sats AS invoice_paid_sats,
                i.confirmed_sats AS invoice_confirmed_sats, i.unconfirmed_sats AS invoice_unconfirmed_sats,
                i.onchain_address AS invoice_onchain_address, i.onchain_address_index AS invoice_onchain_address_index,
                i.onchain_script_pubkey AS invoice_onchain_script_pubkey,
                i.lightning_bolt11 AS invoice_lightning_bolt11,
                i.lightning_payment_hash AS invoice_lightning_payment_hash,
                i.idempotency_key AS invoice_idempotency_key, i.payment_link_id AS invoice_payment_link_id,
                i.rate_source AS invoice_rate_source, i.rate AS invoice_rate, i.metadata AS invoice_metadata,
                i.expires_at AS invoice_expires_at, i.created_at AS invoice_created_at,
                i.updated_at AS invoice_updated_at
            FROM refunds r
            JOIN invoices i ON i.store_id = r.store_id AND i.id = r.invoice_id
            WHERE r.store_id = ? AND r.id = ?
            "#,
        )
        .bind(store_id)
        .bind(refund_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(refund_execution_candidate_from_row(row)?))
    }

    async fn update_refund_status(
        &self,
        refund: &Refund,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            UPDATE refunds
            SET status = ?, tx_id = ?, payment_proof = ?, failure_reason = ?, updated_at = ?, finalized_at = ?
            WHERE store_id = ? AND id = ?
            "#,
        )
        .bind(refund.status.as_str())
        .bind(&refund.tx_id)
        .bind(&refund.payment_proof)
        .bind(&refund.failure_reason)
        .bind(refund.updated_at.to_rfc3339())
        .bind(refund.finalized_at.map(|time| time.to_rfc3339()))
        .bind(&refund.store_id)
        .bind(refund.id.to_string())
        .execute(&mut *tx)
        .await?;
        insert_event_query(event).execute(&mut *tx).await?;
        if let Some(url) = webhook_url {
            enqueue_webhook_query(&event.id, &event.store_id, url, event.created_at)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn try_start_refund_execution(
        &self,
        refund: &Refund,
        day_start: DateTime<Utc>,
        daily_limit_sats: u64,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            r#"
            UPDATE refunds
            SET status = 'processing', failure_reason = NULL, updated_at = ?, finalized_at = NULL
            WHERE store_id = ? AND id = ? AND status = 'pending'
            "#,
        )
        .bind(refund.updated_at.to_rfc3339())
        .bind(&refund.store_id)
        .bind(refund.id.to_string())
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(false);
        }
        let row = sqlx::query(
            r#"
            SELECT COALESCE(SUM(amount_sats), 0) AS total_sats
            FROM refunds
            WHERE store_id = ?
              AND (
                (status = 'succeeded' AND finalized_at IS NOT NULL AND finalized_at >= ?)
                OR (status = 'processing' AND updated_at >= ?)
              )
            "#,
        )
        .bind(&refund.store_id)
        .bind(day_start.to_rfc3339())
        .bind(day_start.to_rfc3339())
        .fetch_one(&mut *tx)
        .await?;
        let reserved = row.get::<i64, _>("total_sats") as u64;
        if reserved > daily_limit_sats {
            tx.rollback().await?;
            return Ok(false);
        }
        insert_event_query(event).execute(&mut *tx).await?;
        if let Some(url) = webhook_url {
            enqueue_webhook_query(&event.id, &event.store_id, url, event.created_at)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    async fn update_refund_approval(
        &self,
        refund: &Refund,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            r#"
            UPDATE refunds
            SET approval_status = ?, updated_at = ?
            WHERE store_id = ? AND id = ? AND status = 'pending' AND approval_status = 'pending'
            "#,
        )
        .bind(refund.approval_status.as_str())
        .bind(refund.updated_at.to_rfc3339())
        .bind(&refund.store_id)
        .bind(refund.id.to_string())
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(false);
        }
        let inserted = insert_event_query(event)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            > 0;
        if inserted && let Some(url) = webhook_url {
            enqueue_webhook_query(&event.id, &event.store_id, url, event.created_at)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    async fn insert_lightning_sweep(&self, sweep: &LightningSweepRecord) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO lightning_sweeps (
                id, store_id, backend, status, balance_sats, amount_sats,
                address, tx_id, error, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(sweep.id.to_string())
        .bind(&sweep.store_id)
        .bind(&sweep.backend)
        .bind(sweep.status.as_str())
        .bind(sweep.balance_sats as i64)
        .bind(sweep.amount_sats as i64)
        .bind(&sweep.address)
        .bind(&sweep.tx_id)
        .bind(&sweep.error)
        .bind(sweep.created_at.to_rfc3339())
        .bind(sweep.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn lightning_sweeps(
        &self,
        store_id: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<LightningSweepRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, backend, status, balance_sats, amount_sats,
                   address, tx_id, error, created_at, updated_at
            FROM lightning_sweeps
            WHERE store_id = ?
            ORDER BY created_at DESC
            LIMIT ?
            "#,
        )
        .bind(store_id)
        .bind(i64::from(limit.clamp(1, 200)))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(lightning_sweep_from_row).collect()
    }
}

#[async_trait]
impl Store for PostgresStore {
    async fn migrate(&self) -> anyhow::Result<()> {
        migrate_postgres(&self.pool).await
    }

    async fn reserve_address_index(&self, store_id: &str) -> anyhow::Result<u32> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO qpayd_store_counters (store_id, next_onchain_index)
            VALUES ($1, 0)
            ON CONFLICT(store_id) DO NOTHING
            "#,
        )
        .bind(store_id)
        .execute(&mut *tx)
        .await?;

        let row = sqlx::query(
            r#"
            SELECT next_onchain_index
            FROM qpayd_store_counters
            WHERE store_id = $1
            "#,
        )
        .bind(store_id)
        .fetch_one(&mut *tx)
        .await?;
        let index = row.get::<i64, _>("next_onchain_index");

        sqlx::query(
            r#"
            UPDATE qpayd_store_counters
            SET next_onchain_index = next_onchain_index + 1
            WHERE store_id = $1
            "#,
        )
        .bind(store_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(index as u32)
    }

    async fn insert_invoice(
        &self,
        invoice: &Invoice,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO qpayd_invoices (
                id, store_id, status, amount, currency, btc_amount_sats,
                paid_sats, confirmed_sats, unconfirmed_sats,
                onchain_address, onchain_address_index, onchain_script_pubkey,
                lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, rate_source, rate,
                metadata, expires_at, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22)
            "#,
        )
        .bind(invoice.id.to_string())
        .bind(&invoice.store_id)
        .bind(invoice.status.as_str())
        .bind(invoice.amount.to_string())
        .bind(&invoice.currency)
        .bind(invoice.btc_amount_sats as i64)
        .bind(invoice.paid_sats as i64)
        .bind(invoice.confirmed_sats as i64)
        .bind(invoice.unconfirmed_sats as i64)
        .bind(&invoice.onchain_address)
        .bind(invoice.onchain_address_index.map(|index| index as i64))
        .bind(&invoice.onchain_script_pubkey)
        .bind(&invoice.lightning_bolt11)
        .bind(&invoice.lightning_payment_hash)
        .bind(&invoice.idempotency_key)
        .bind(&invoice.payment_link_id)
        .bind(&invoice.rate_source)
        .bind(invoice.rate.to_string())
        .bind(invoice.metadata.to_string())
        .bind(invoice.expires_at.to_rfc3339())
        .bind(invoice.created_at.to_rfc3339())
        .bind(invoice.updated_at.to_rfc3339())
        .execute(&mut *tx)
        .await?;

        insert_pg_event_query(event).execute(&mut *tx).await?;
        if let Some(url) = webhook_url {
            enqueue_pg_webhook_query(&event.id, &event.store_id, url, event.created_at)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;

        Ok(())
    }

    async fn invoice(&self, store_id: &str, id: Uuid) -> anyhow::Result<Option<Invoice>> {
        let Some(row) = sqlx::query(
            r#"
            SELECT id, store_id, status, amount, currency, btc_amount_sats,
                   paid_sats, confirmed_sats, unconfirmed_sats,
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                   expires_at, created_at, updated_at
            FROM qpayd_invoices
            WHERE store_id = $1 AND id = $2
            "#,
        )
        .bind(store_id)
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        Ok(Some(invoice_from_pg_row(row)?))
    }

    async fn invoices(
        &self,
        store_id: &str,
        filter: InvoiceListFilter,
    ) -> anyhow::Result<Vec<Invoice>> {
        let limit = i64::from(filter.limit.clamp(1, 200));
        let rows = if let Some(status) = filter.status {
            sqlx::query(
                r#"
                SELECT id, store_id, status, amount, currency, btc_amount_sats,
                       paid_sats, confirmed_sats, unconfirmed_sats,
                       onchain_address, onchain_address_index, onchain_script_pubkey,
                       rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                       expires_at, created_at, updated_at
                FROM qpayd_invoices
                WHERE store_id = $1 AND status = $2
                ORDER BY created_at DESC
                LIMIT $3
                "#,
            )
            .bind(store_id)
            .bind(status.as_str())
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT id, store_id, status, amount, currency, btc_amount_sats,
                       paid_sats, confirmed_sats, unconfirmed_sats,
                       onchain_address, onchain_address_index, onchain_script_pubkey,
                       rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                       expires_at, created_at, updated_at
                FROM qpayd_invoices
                WHERE store_id = $1
                ORDER BY created_at DESC
                LIMIT $2
                "#,
            )
            .bind(store_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };

        rows.into_iter().map(invoice_from_pg_row).collect()
    }

    async fn invoice_by_idempotency_key(
        &self,
        store_id: &str,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<Invoice>> {
        let Some(row) = sqlx::query(
            r#"
            SELECT id, store_id, status, amount, currency, btc_amount_sats,
                   paid_sats, confirmed_sats, unconfirmed_sats,
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                   expires_at, created_at, updated_at
            FROM qpayd_invoices
            WHERE store_id = $1 AND idempotency_key = $2 AND payment_link_id IS NULL
            "#,
        )
        .bind(store_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        Ok(Some(invoice_from_pg_row(row)?))
    }

    async fn invoice_by_payment_link_idempotency_key(
        &self,
        store_id: &str,
        payment_link_id: &str,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<Invoice>> {
        let Some(row) = sqlx::query(
            r#"
            SELECT id, store_id, status, amount, currency, btc_amount_sats,
                   paid_sats, confirmed_sats, unconfirmed_sats,
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                   expires_at, created_at, updated_at
            FROM qpayd_invoices
            WHERE store_id = $1 AND payment_link_id = $2 AND idempotency_key = $3
            "#,
        )
        .bind(store_id)
        .bind(payment_link_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        Ok(Some(invoice_from_pg_row(row)?))
    }

    async fn active_onchain_invoices(&self, store_id: &str) -> anyhow::Result<Vec<Invoice>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, status, amount, currency, btc_amount_sats,
                   paid_sats, confirmed_sats, unconfirmed_sats,
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                   expires_at, created_at, updated_at
            FROM qpayd_invoices
            WHERE store_id = $1
              AND onchain_script_pubkey IS NOT NULL
              AND status IN ('new', 'payment_detected', 'partially_paid', 'expired')
            "#,
        )
        .bind(store_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(invoice_from_pg_row).collect()
    }

    async fn active_lightning_invoices(&self, store_id: &str) -> anyhow::Result<Vec<Invoice>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, status, amount, currency, btc_amount_sats,
                   paid_sats, confirmed_sats, unconfirmed_sats,
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                   expires_at, created_at, updated_at
            FROM qpayd_invoices
            WHERE store_id = $1
              AND lightning_payment_hash IS NOT NULL
              AND status IN ('new', 'payment_detected', 'partially_paid', 'expired')
            "#,
        )
        .bind(store_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(invoice_from_pg_row).collect()
    }

    async fn expirable_invoices(
        &self,
        store_id: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Vec<Invoice>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, status, amount, currency, btc_amount_sats,
                   paid_sats, confirmed_sats, unconfirmed_sats,
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, idempotency_key, payment_link_id, metadata,
                   expires_at, created_at, updated_at
            FROM qpayd_invoices
            WHERE store_id = $1
              AND status IN ('new', 'partially_paid')
              AND expires_at <= $2
            "#,
        )
        .bind(store_id)
        .bind(now.to_rfc3339())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(invoice_from_pg_row).collect()
    }

    async fn update_invoice_status(
        &self,
        store_id: &str,
        id: Uuid,
        update: InvoiceStatusUpdate,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            UPDATE qpayd_invoices
            SET status = $1,
                paid_sats = $2,
                confirmed_sats = $3,
                unconfirmed_sats = $4,
                updated_at = $5
            WHERE store_id = $6 AND id = $7
            "#,
        )
        .bind(update.status.as_str())
        .bind(update.payment.paid_sats as i64)
        .bind(update.payment.confirmed_sats as i64)
        .bind(update.payment.unconfirmed_sats as i64)
        .bind(update.updated_at.to_rfc3339())
        .bind(store_id)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;

        let inserted = insert_pg_event_query(event)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            > 0;
        if inserted && let Some(url) = webhook_url {
            enqueue_pg_webhook_query(&event.id, &event.store_id, url, event.created_at)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;

        Ok(())
    }

    async fn update_invoice_payment_amounts(
        &self,
        store_id: &str,
        id: Uuid,
        payment: PaymentAmounts,
        updated_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            UPDATE qpayd_invoices
            SET paid_sats = $1,
                confirmed_sats = $2,
                unconfirmed_sats = $3,
                updated_at = $4
            WHERE store_id = $5 AND id = $6
            "#,
        )
        .bind(payment.paid_sats as i64)
        .bind(payment.confirmed_sats as i64)
        .bind(payment.unconfirmed_sats as i64)
        .bind(updated_at.to_rfc3339())
        .bind(store_id)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn events(&self, store_id: &str, limit: u32) -> anyhow::Result<Vec<EventEnvelope>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, invoice_id, type, payload_json, created_at
            FROM qpayd_events
            WHERE store_id = $1
            ORDER BY created_at DESC
            LIMIT $2
            "#,
        )
        .bind(store_id)
        .bind(i64::from(limit.min(200)))
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(event_from_pg_row).collect()
    }

    async fn event(&self, store_id: &str, event_id: &str) -> anyhow::Result<Option<EventEnvelope>> {
        let Some(row) = sqlx::query(
            r#"
            SELECT id, store_id, invoice_id, type, payload_json, created_at
            FROM qpayd_events
            WHERE store_id = $1 AND id = $2
            "#,
        )
        .bind(store_id)
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        Ok(Some(event_from_pg_row(row)?))
    }

    async fn enqueue_webhook_delivery(
        &self,
        event_id: &str,
        store_id: &str,
        url: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        enqueue_pg_webhook_query(event_id, store_id, url, now)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn due_webhook_deliveries(
        &self,
        limit: u32,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Vec<QueuedWebhookDelivery>> {
        let rows = sqlx::query(
            r#"
            SELECT
                wd.id AS delivery_id,
                wd.url AS delivery_url,
                wd.attempts AS delivery_attempts,
                e.id, e.store_id, e.invoice_id, e.type, e.payload_json, e.created_at
            FROM qpayd_webhook_deliveries wd
            JOIN qpayd_events e ON e.id = wd.event_id
            WHERE wd.status = 'pending' AND wd.next_attempt_at <= $1
            ORDER BY wd.next_attempt_at ASC, wd.id ASC
            LIMIT $2
            "#,
        )
        .bind(now.to_rfc3339())
        .bind(i64::from(limit.min(100)))
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(pg_webhook_delivery_from_row).collect()
    }

    async fn mark_webhook_delivered(
        &self,
        id: i64,
        delivered_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            UPDATE qpayd_webhook_deliveries
            SET status = 'delivered',
                delivered_at = $1,
                updated_at = $2
            WHERE id = $3
            "#,
        )
        .bind(delivered_at.to_rfc3339())
        .bind(delivered_at.to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mark_webhook_failed(
        &self,
        id: i64,
        attempts: u32,
        next_attempt_at: DateTime<Utc>,
        error: &str,
        updated_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            UPDATE qpayd_webhook_deliveries
            SET attempts = $1,
                next_attempt_at = $2,
                last_error = $3,
                updated_at = $4
            WHERE id = $5
            "#,
        )
        .bind(i64::from(attempts))
        .bind(next_attempt_at.to_rfc3339())
        .bind(error)
        .bind(updated_at.to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn insert_refund(
        &self,
        refund: &Refund,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO qpayd_refunds (
                id, store_id, invoice_id, status, approval_status, amount_sats, destination, destination_type,
                reason, tx_id, payment_proof, failure_reason, idempotency_key, metadata, created_at, updated_at, finalized_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
            "#,
        )
        .bind(refund.id.to_string())
        .bind(&refund.store_id)
        .bind(refund.invoice_id.to_string())
        .bind(refund.status.as_str())
        .bind(refund.approval_status.as_str())
        .bind(refund.amount_sats as i64)
        .bind(&refund.destination)
        .bind(refund.destination_type.map(|value| value.as_str()))
        .bind(&refund.reason)
        .bind(&refund.tx_id)
        .bind(&refund.payment_proof)
        .bind(&refund.failure_reason)
        .bind(&refund.idempotency_key)
        .bind(refund.metadata.to_string())
        .bind(refund.created_at.to_rfc3339())
        .bind(refund.updated_at.to_rfc3339())
        .bind(refund.finalized_at.map(|time| time.to_rfc3339()))
        .execute(&mut *tx)
        .await?;
        insert_pg_event_query(event).execute(&mut *tx).await?;
        if let Some(url) = webhook_url {
            enqueue_pg_webhook_query(&event.id, &event.store_id, url, event.created_at)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn refund(&self, store_id: &str, id: Uuid) -> anyhow::Result<Option<Refund>> {
        let Some(row) = sqlx::query(
            r#"
            SELECT id, store_id, invoice_id, status, approval_status, amount_sats, destination, destination_type,
                   reason, tx_id, payment_proof, failure_reason, idempotency_key, metadata, created_at, updated_at, finalized_at
            FROM qpayd_refunds
            WHERE store_id = $1 AND id = $2
            "#,
        )
        .bind(store_id)
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        Ok(Some(refund_from_pg_row(row)?))
    }

    async fn refund_by_idempotency_key(
        &self,
        store_id: &str,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<Refund>> {
        let Some(row) = sqlx::query(
            r#"
            SELECT id, store_id, invoice_id, status, approval_status, amount_sats, destination, destination_type,
                   reason, tx_id, payment_proof, failure_reason, idempotency_key, metadata, created_at, updated_at, finalized_at
            FROM qpayd_refunds
            WHERE store_id = $1 AND idempotency_key = $2
            "#,
        )
        .bind(store_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        Ok(Some(refund_from_pg_row(row)?))
    }

    async fn refunds(&self, store_id: &str, limit: u32) -> anyhow::Result<Vec<Refund>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, invoice_id, status, approval_status, amount_sats, destination, destination_type,
                   reason, tx_id, payment_proof, failure_reason, idempotency_key, metadata, created_at, updated_at, finalized_at
            FROM qpayd_refunds
            WHERE store_id = $1
            ORDER BY created_at DESC
            LIMIT $2
            "#,
        )
        .bind(store_id)
        .bind(i64::from(limit.clamp(1, 200)))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(refund_from_pg_row).collect()
    }

    async fn refunds_for_invoice(
        &self,
        store_id: &str,
        invoice_id: Uuid,
    ) -> anyhow::Result<Vec<Refund>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, invoice_id, status, approval_status, amount_sats, destination, destination_type,
                   reason, tx_id, payment_proof, failure_reason, idempotency_key, metadata, created_at, updated_at, finalized_at
            FROM qpayd_refunds
            WHERE store_id = $1 AND invoice_id = $2
            ORDER BY created_at DESC
            "#,
        )
        .bind(store_id)
        .bind(invoice_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(refund_from_pg_row).collect()
    }

    async fn pending_refund_executions(
        &self,
        store_id: &str,
        limit: u32,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Vec<RefundExecutionCandidate>> {
        let rows = sqlx::query(
            r#"
            SELECT
                r.id AS refund_id, r.store_id AS refund_store_id, r.invoice_id AS refund_invoice_id,
                r.status AS refund_status, r.approval_status AS refund_approval_status, r.amount_sats AS refund_amount_sats,
                r.destination AS refund_destination, r.destination_type AS refund_destination_type,
                r.reason AS refund_reason, r.tx_id AS refund_tx_id, r.payment_proof AS refund_payment_proof,
                r.failure_reason AS refund_failure_reason, r.idempotency_key AS refund_idempotency_key,
                r.metadata AS refund_metadata, r.created_at AS refund_created_at,
                r.updated_at AS refund_updated_at, r.finalized_at AS refund_finalized_at,
                i.id AS invoice_id, i.store_id AS invoice_store_id, i.status AS invoice_status,
                i.amount AS invoice_amount, i.currency AS invoice_currency,
                i.btc_amount_sats AS invoice_btc_amount_sats, i.paid_sats AS invoice_paid_sats,
                i.confirmed_sats AS invoice_confirmed_sats, i.unconfirmed_sats AS invoice_unconfirmed_sats,
                i.onchain_address AS invoice_onchain_address, i.onchain_address_index AS invoice_onchain_address_index,
                i.onchain_script_pubkey AS invoice_onchain_script_pubkey,
                i.lightning_bolt11 AS invoice_lightning_bolt11,
                i.lightning_payment_hash AS invoice_lightning_payment_hash,
                i.idempotency_key AS invoice_idempotency_key, i.payment_link_id AS invoice_payment_link_id,
                i.rate_source AS invoice_rate_source, i.rate AS invoice_rate, i.metadata AS invoice_metadata,
                i.expires_at AS invoice_expires_at, i.created_at AS invoice_created_at,
                i.updated_at AS invoice_updated_at
            FROM qpayd_refunds r
            JOIN qpayd_invoices i ON i.store_id = r.store_id AND i.id = r.invoice_id
            WHERE r.store_id = $1
              AND r.status = 'pending'
              AND r.approval_status IN ('not_required', 'approved')
              AND r.destination IS NOT NULL
              AND (r.execution_lease_until IS NULL OR r.execution_lease_until <= $2)
            ORDER BY r.created_at ASC
            LIMIT $3
            "#,
        )
        .bind(store_id)
        .bind(now.to_rfc3339())
        .bind(i64::from(limit.clamp(1, 200)))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(refund_execution_candidate_from_pg_row)
            .collect()
    }

    async fn claim_refund_execution(
        &self,
        store_id: &str,
        refund_id: Uuid,
        worker_id: &str,
        lease_until: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Option<RefundExecutionCandidate>> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"
            UPDATE qpayd_refunds
            SET execution_claimed_by = $1,
                execution_lease_until = $2,
                updated_at = $3
            WHERE store_id = $4
              AND id = $5
              AND status = 'pending'
              AND approval_status IN ('not_required', 'approved')
              AND destination IS NOT NULL
              AND (execution_lease_until IS NULL OR execution_lease_until <= $6)
            "#,
        )
        .bind(worker_id)
        .bind(lease_until.to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(store_id)
        .bind(refund_id.to_string())
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(None);
        }
        let row = sqlx::query(
            r#"
            SELECT
                r.id AS refund_id, r.store_id AS refund_store_id, r.invoice_id AS refund_invoice_id,
                r.status AS refund_status, r.approval_status AS refund_approval_status, r.amount_sats AS refund_amount_sats,
                r.destination AS refund_destination, r.destination_type AS refund_destination_type,
                r.reason AS refund_reason, r.tx_id AS refund_tx_id, r.payment_proof AS refund_payment_proof,
                r.failure_reason AS refund_failure_reason, r.idempotency_key AS refund_idempotency_key,
                r.metadata AS refund_metadata, r.created_at AS refund_created_at,
                r.updated_at AS refund_updated_at, r.finalized_at AS refund_finalized_at,
                i.id AS invoice_id, i.store_id AS invoice_store_id, i.status AS invoice_status,
                i.amount AS invoice_amount, i.currency AS invoice_currency,
                i.btc_amount_sats AS invoice_btc_amount_sats, i.paid_sats AS invoice_paid_sats,
                i.confirmed_sats AS invoice_confirmed_sats, i.unconfirmed_sats AS invoice_unconfirmed_sats,
                i.onchain_address AS invoice_onchain_address, i.onchain_address_index AS invoice_onchain_address_index,
                i.onchain_script_pubkey AS invoice_onchain_script_pubkey,
                i.lightning_bolt11 AS invoice_lightning_bolt11,
                i.lightning_payment_hash AS invoice_lightning_payment_hash,
                i.idempotency_key AS invoice_idempotency_key, i.payment_link_id AS invoice_payment_link_id,
                i.rate_source AS invoice_rate_source, i.rate AS invoice_rate, i.metadata AS invoice_metadata,
                i.expires_at AS invoice_expires_at, i.created_at AS invoice_created_at,
                i.updated_at AS invoice_updated_at
            FROM qpayd_refunds r
            JOIN qpayd_invoices i ON i.store_id = r.store_id AND i.id = r.invoice_id
            WHERE r.store_id = $1 AND r.id = $2
            "#,
        )
        .bind(store_id)
        .bind(refund_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(refund_execution_candidate_from_pg_row(row)?))
    }

    async fn update_refund_status(
        &self,
        refund: &Refund,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            UPDATE qpayd_refunds
            SET status = $1, tx_id = $2, payment_proof = $3, failure_reason = $4, updated_at = $5, finalized_at = $6
            WHERE store_id = $7 AND id = $8
            "#,
        )
        .bind(refund.status.as_str())
        .bind(&refund.tx_id)
        .bind(&refund.payment_proof)
        .bind(&refund.failure_reason)
        .bind(refund.updated_at.to_rfc3339())
        .bind(refund.finalized_at.map(|time| time.to_rfc3339()))
        .bind(&refund.store_id)
        .bind(refund.id.to_string())
        .execute(&mut *tx)
        .await?;
        insert_pg_event_query(event).execute(&mut *tx).await?;
        if let Some(url) = webhook_url {
            enqueue_pg_webhook_query(&event.id, &event.store_id, url, event.created_at)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn try_start_refund_execution(
        &self,
        refund: &Refund,
        day_start: DateTime<Utc>,
        daily_limit_sats: u64,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(&refund.store_id)
            .execute(&mut *tx)
            .await?;

        let updated = sqlx::query(
            r#"
            UPDATE qpayd_refunds
            SET status = 'processing', failure_reason = NULL, updated_at = $1, finalized_at = NULL
            WHERE store_id = $2 AND id = $3 AND status = 'pending'
            "#,
        )
        .bind(refund.updated_at.to_rfc3339())
        .bind(&refund.store_id)
        .bind(refund.id.to_string())
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(false);
        }
        let row = sqlx::query(
            r#"
            SELECT COALESCE(SUM(amount_sats), 0)::BIGINT AS total_sats
            FROM qpayd_refunds
            WHERE store_id = $1
              AND (
                (status = 'succeeded' AND finalized_at IS NOT NULL AND finalized_at >= $2)
                OR (status = 'processing' AND updated_at >= $3)
              )
            "#,
        )
        .bind(&refund.store_id)
        .bind(day_start.to_rfc3339())
        .bind(day_start.to_rfc3339())
        .fetch_one(&mut *tx)
        .await?;
        let reserved = row.get::<i64, _>("total_sats") as u64;
        if reserved > daily_limit_sats {
            tx.rollback().await?;
            return Ok(false);
        }
        insert_pg_event_query(event).execute(&mut *tx).await?;
        if let Some(url) = webhook_url {
            enqueue_pg_webhook_query(&event.id, &event.store_id, url, event.created_at)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    async fn update_refund_approval(
        &self,
        refund: &Refund,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            r#"
            UPDATE qpayd_refunds
            SET approval_status = $1, updated_at = $2
            WHERE store_id = $3 AND id = $4 AND status = 'pending' AND approval_status = 'pending'
            "#,
        )
        .bind(refund.approval_status.as_str())
        .bind(refund.updated_at.to_rfc3339())
        .bind(&refund.store_id)
        .bind(refund.id.to_string())
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(false);
        }
        let inserted = insert_pg_event_query(event)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            > 0;
        if inserted && let Some(url) = webhook_url {
            enqueue_pg_webhook_query(&event.id, &event.store_id, url, event.created_at)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    async fn insert_lightning_sweep(&self, sweep: &LightningSweepRecord) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO qpayd_lightning_sweeps (
                id, store_id, backend, status, balance_sats, amount_sats,
                address, tx_id, error, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            "#,
        )
        .bind(sweep.id.to_string())
        .bind(&sweep.store_id)
        .bind(&sweep.backend)
        .bind(sweep.status.as_str())
        .bind(sweep.balance_sats as i64)
        .bind(sweep.amount_sats as i64)
        .bind(&sweep.address)
        .bind(&sweep.tx_id)
        .bind(&sweep.error)
        .bind(sweep.created_at.to_rfc3339())
        .bind(sweep.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn lightning_sweeps(
        &self,
        store_id: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<LightningSweepRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, backend, status, balance_sats, amount_sats,
                   address, tx_id, error, created_at, updated_at
            FROM qpayd_lightning_sweeps
            WHERE store_id = $1
            ORDER BY created_at DESC
            LIMIT $2
            "#,
        )
        .bind(store_id)
        .bind(i64::from(limit.clamp(1, 200)))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(lightning_sweep_from_pg_row).collect()
    }
}

struct Migration {
    version: i64,
    name: &'static str,
    statements: &'static [&'static str],
}

const SQLITE_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial_schema",
        statements: &[
            r#"
        CREATE TABLE IF NOT EXISTS store_counters (
            store_id TEXT PRIMARY KEY NOT NULL,
            next_onchain_index INTEGER NOT NULL
        )
        "#,
            r#"
        CREATE TABLE IF NOT EXISTS invoices (
            id TEXT PRIMARY KEY NOT NULL,
            store_id TEXT NOT NULL,
            status TEXT NOT NULL,
            amount TEXT NOT NULL,
            currency TEXT NOT NULL,
            btc_amount_sats INTEGER NOT NULL,
            onchain_address TEXT,
            onchain_address_index INTEGER,
            onchain_script_pubkey TEXT,
            lightning_bolt11 TEXT,
            lightning_payment_hash TEXT,
            rate_source TEXT NOT NULL,
            rate TEXT NOT NULL,
            metadata TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS invoices_store_created_idx
        ON invoices (store_id, created_at)
        "#,
            r#"
        CREATE TABLE IF NOT EXISTS events (
            id TEXT PRIMARY KEY NOT NULL,
            store_id TEXT NOT NULL,
            invoice_id TEXT,
            type TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        )
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS events_store_created_idx
        ON events (store_id, created_at)
        "#,
            r#"
        CREATE TABLE IF NOT EXISTS webhook_deliveries (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id TEXT NOT NULL,
            store_id TEXT NOT NULL,
            url TEXT NOT NULL,
            status TEXT NOT NULL,
            attempts INTEGER NOT NULL,
            next_attempt_at TEXT NOT NULL,
            last_error TEXT,
            delivered_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY(event_id) REFERENCES events(id)
        )
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS webhook_deliveries_due_idx
        ON webhook_deliveries (status, next_attempt_at)
        "#,
        ],
    },
    Migration {
        version: 2,
        name: "invoice_idempotency_keys",
        statements: &[
            r#"
        ALTER TABLE invoices ADD COLUMN idempotency_key TEXT
        "#,
            r#"
        CREATE UNIQUE INDEX IF NOT EXISTS invoices_store_idempotency_key_idx
        ON invoices (store_id, idempotency_key)
        WHERE idempotency_key IS NOT NULL
            "#,
        ],
    },
    Migration {
        version: 3,
        name: "invoice_payment_amounts",
        statements: &[
            r#"
        ALTER TABLE invoices ADD COLUMN paid_sats INTEGER NOT NULL DEFAULT 0
        "#,
            r#"
        ALTER TABLE invoices ADD COLUMN confirmed_sats INTEGER NOT NULL DEFAULT 0
        "#,
            r#"
        ALTER TABLE invoices ADD COLUMN unconfirmed_sats INTEGER NOT NULL DEFAULT 0
            "#,
        ],
    },
    Migration {
        version: 4,
        name: "merchant_admin_records",
        statements: &[
            r#"
        CREATE TABLE IF NOT EXISTS refunds (
            id TEXT PRIMARY KEY NOT NULL,
            store_id TEXT NOT NULL,
            invoice_id TEXT NOT NULL,
            status TEXT NOT NULL,
            amount_sats INTEGER NOT NULL,
            destination TEXT,
            reason TEXT,
            tx_id TEXT,
            metadata TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            finalized_at TEXT
        )
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS refunds_store_created_idx
        ON refunds (store_id, created_at)
        "#,
            r#"
        CREATE TABLE IF NOT EXISTS lightning_sweeps (
            id TEXT PRIMARY KEY NOT NULL,
            store_id TEXT NOT NULL,
            backend TEXT NOT NULL,
            status TEXT NOT NULL,
            balance_sats INTEGER NOT NULL,
            amount_sats INTEGER NOT NULL,
            address TEXT NOT NULL,
            tx_id TEXT,
            error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS lightning_sweeps_store_created_idx
        ON lightning_sweeps (store_id, created_at)
        "#,
        ],
    },
    Migration {
        version: 5,
        name: "invoice_payment_link_idempotency_scope",
        statements: &[
            r#"
        ALTER TABLE invoices ADD COLUMN payment_link_id TEXT
        "#,
            r#"
        DROP INDEX IF EXISTS invoices_store_idempotency_key_idx
        "#,
            r#"
        CREATE UNIQUE INDEX IF NOT EXISTS invoices_store_admin_idempotency_key_idx
        ON invoices (store_id, idempotency_key)
        WHERE idempotency_key IS NOT NULL AND payment_link_id IS NULL
            "#,
            r#"
        CREATE UNIQUE INDEX IF NOT EXISTS invoices_store_payment_link_idempotency_key_idx
        ON invoices (store_id, payment_link_id, idempotency_key)
        WHERE idempotency_key IS NOT NULL AND payment_link_id IS NOT NULL
            "#,
        ],
    },
    Migration {
        version: 6,
        name: "refund_idempotency_and_proofs",
        statements: &[
            r#"
        ALTER TABLE refunds ADD COLUMN destination_type TEXT
        "#,
            r#"
        ALTER TABLE refunds ADD COLUMN payment_proof TEXT
        "#,
            r#"
        ALTER TABLE refunds ADD COLUMN failure_reason TEXT
        "#,
            r#"
        ALTER TABLE refunds ADD COLUMN idempotency_key TEXT
        "#,
            r#"
        CREATE UNIQUE INDEX IF NOT EXISTS refunds_store_idempotency_key_idx
        ON refunds (store_id, idempotency_key)
        WHERE idempotency_key IS NOT NULL
            "#,
        ],
    },
    Migration {
        version: 7,
        name: "refund_execution_claims",
        statements: &[
            r#"
        ALTER TABLE refunds ADD COLUMN execution_claimed_by TEXT
        "#,
            r#"
        ALTER TABLE refunds ADD COLUMN execution_lease_until TEXT
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS refunds_execution_due_idx
        ON refunds (store_id, status, execution_lease_until, created_at)
            "#,
        ],
    },
    Migration {
        version: 8,
        name: "refund_manual_approval",
        statements: &[
            r#"
        ALTER TABLE refunds ADD COLUMN approval_status TEXT NOT NULL DEFAULT 'not_required'
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS refunds_approval_status_idx
        ON refunds (store_id, approval_status, created_at)
            "#,
        ],
    },
];

const POSTGRES_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial_schema",
        statements: &[
            r#"
        CREATE TABLE IF NOT EXISTS qpayd_store_counters (
            store_id TEXT PRIMARY KEY NOT NULL,
            next_onchain_index BIGINT NOT NULL
        )
        "#,
            r#"
        CREATE TABLE IF NOT EXISTS qpayd_invoices (
            id TEXT PRIMARY KEY NOT NULL,
            store_id TEXT NOT NULL,
            status TEXT NOT NULL,
            amount TEXT NOT NULL,
            currency TEXT NOT NULL,
            btc_amount_sats BIGINT NOT NULL,
            onchain_address TEXT,
            onchain_address_index BIGINT,
            onchain_script_pubkey TEXT,
            lightning_bolt11 TEXT,
            lightning_payment_hash TEXT,
            rate_source TEXT NOT NULL,
            rate TEXT NOT NULL,
            metadata TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS qpayd_invoices_store_created_idx
        ON qpayd_invoices (store_id, created_at)
        "#,
            r#"
        CREATE TABLE IF NOT EXISTS qpayd_events (
            id TEXT PRIMARY KEY NOT NULL,
            store_id TEXT NOT NULL,
            invoice_id TEXT,
            type TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        )
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS qpayd_events_store_created_idx
        ON qpayd_events (store_id, created_at)
        "#,
            r#"
        CREATE TABLE IF NOT EXISTS qpayd_webhook_deliveries (
            id BIGSERIAL PRIMARY KEY,
            event_id TEXT NOT NULL REFERENCES qpayd_events(id),
            store_id TEXT NOT NULL,
            url TEXT NOT NULL,
            status TEXT NOT NULL,
            attempts BIGINT NOT NULL,
            next_attempt_at TEXT NOT NULL,
            last_error TEXT,
            delivered_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS qpayd_webhook_deliveries_due_idx
        ON qpayd_webhook_deliveries (status, next_attempt_at)
        "#,
        ],
    },
    Migration {
        version: 2,
        name: "invoice_idempotency_keys",
        statements: &[
            r#"
        ALTER TABLE qpayd_invoices ADD COLUMN idempotency_key TEXT
        "#,
            r#"
        CREATE UNIQUE INDEX IF NOT EXISTS qpayd_invoices_store_idempotency_key_idx
        ON qpayd_invoices (store_id, idempotency_key)
        WHERE idempotency_key IS NOT NULL
            "#,
        ],
    },
    Migration {
        version: 3,
        name: "invoice_payment_amounts",
        statements: &[
            r#"
        ALTER TABLE qpayd_invoices ADD COLUMN paid_sats BIGINT NOT NULL DEFAULT 0
        "#,
            r#"
        ALTER TABLE qpayd_invoices ADD COLUMN confirmed_sats BIGINT NOT NULL DEFAULT 0
        "#,
            r#"
        ALTER TABLE qpayd_invoices ADD COLUMN unconfirmed_sats BIGINT NOT NULL DEFAULT 0
            "#,
        ],
    },
    Migration {
        version: 4,
        name: "merchant_admin_records",
        statements: &[
            r#"
        CREATE TABLE IF NOT EXISTS qpayd_refunds (
            id TEXT PRIMARY KEY NOT NULL,
            store_id TEXT NOT NULL,
            invoice_id TEXT NOT NULL,
            status TEXT NOT NULL,
            amount_sats BIGINT NOT NULL,
            destination TEXT,
            reason TEXT,
            tx_id TEXT,
            metadata TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            finalized_at TEXT
        )
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS qpayd_refunds_store_created_idx
        ON qpayd_refunds (store_id, created_at)
        "#,
            r#"
        CREATE TABLE IF NOT EXISTS qpayd_lightning_sweeps (
            id TEXT PRIMARY KEY NOT NULL,
            store_id TEXT NOT NULL,
            backend TEXT NOT NULL,
            status TEXT NOT NULL,
            balance_sats BIGINT NOT NULL,
            amount_sats BIGINT NOT NULL,
            address TEXT NOT NULL,
            tx_id TEXT,
            error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS qpayd_lightning_sweeps_store_created_idx
        ON qpayd_lightning_sweeps (store_id, created_at)
        "#,
        ],
    },
    Migration {
        version: 5,
        name: "invoice_payment_link_idempotency_scope",
        statements: &[
            r#"
        ALTER TABLE qpayd_invoices ADD COLUMN payment_link_id TEXT
        "#,
            r#"
        DROP INDEX IF EXISTS qpayd_invoices_store_idempotency_key_idx
        "#,
            r#"
        CREATE UNIQUE INDEX IF NOT EXISTS qpayd_invoices_store_admin_idempotency_key_idx
        ON qpayd_invoices (store_id, idempotency_key)
        WHERE idempotency_key IS NOT NULL AND payment_link_id IS NULL
            "#,
            r#"
        CREATE UNIQUE INDEX IF NOT EXISTS qpayd_invoices_store_payment_link_idempotency_key_idx
        ON qpayd_invoices (store_id, payment_link_id, idempotency_key)
        WHERE idempotency_key IS NOT NULL AND payment_link_id IS NOT NULL
            "#,
        ],
    },
    Migration {
        version: 6,
        name: "refund_idempotency_and_proofs",
        statements: &[
            r#"
        ALTER TABLE qpayd_refunds ADD COLUMN destination_type TEXT
        "#,
            r#"
        ALTER TABLE qpayd_refunds ADD COLUMN payment_proof TEXT
        "#,
            r#"
        ALTER TABLE qpayd_refunds ADD COLUMN failure_reason TEXT
        "#,
            r#"
        ALTER TABLE qpayd_refunds ADD COLUMN idempotency_key TEXT
        "#,
            r#"
        CREATE UNIQUE INDEX IF NOT EXISTS qpayd_refunds_store_idempotency_key_idx
        ON qpayd_refunds (store_id, idempotency_key)
        WHERE idempotency_key IS NOT NULL
            "#,
        ],
    },
    Migration {
        version: 7,
        name: "refund_execution_claims",
        statements: &[
            r#"
        ALTER TABLE qpayd_refunds ADD COLUMN execution_claimed_by TEXT
        "#,
            r#"
        ALTER TABLE qpayd_refunds ADD COLUMN execution_lease_until TEXT
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS qpayd_refunds_execution_due_idx
        ON qpayd_refunds (store_id, status, execution_lease_until, created_at)
            "#,
        ],
    },
    Migration {
        version: 8,
        name: "refund_manual_approval",
        statements: &[
            r#"
        ALTER TABLE qpayd_refunds ADD COLUMN approval_status TEXT NOT NULL DEFAULT 'not_required'
        "#,
            r#"
        CREATE INDEX IF NOT EXISTS qpayd_refunds_approval_status_idx
        ON qpayd_refunds (store_id, approval_status, created_at)
            "#,
        ],
    },
];

async fn migrate_sqlite(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    let applied = applied_sqlite_migrations(pool).await?;
    for migration in SQLITE_MIGRATIONS {
        if applied.contains(&migration.version) {
            continue;
        }
        let mut tx = pool.begin().await?;
        for statement in migration.statements {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        sqlx::query(
            r#"
            INSERT INTO schema_migrations (version, name, applied_at)
            VALUES (?, ?, ?)
            "#,
        )
        .bind(migration.version)
        .bind(migration.name)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }

    Ok(())
}

async fn migrate_postgres(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS qpayd_schema_migrations (
            version BIGINT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    let applied = applied_pg_migrations(pool).await?;
    for migration in POSTGRES_MIGRATIONS {
        if applied.contains(&migration.version) {
            continue;
        }
        let mut tx = pool.begin().await?;
        for statement in migration.statements {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        sqlx::query(
            r#"
            INSERT INTO qpayd_schema_migrations (version, name, applied_at)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(migration.version)
        .bind(migration.name)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }

    Ok(())
}

async fn applied_sqlite_migrations(pool: &SqlitePool) -> anyhow::Result<Vec<i64>> {
    let rows = sqlx::query("SELECT version FROM schema_migrations ORDER BY version")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| row.get::<i64, _>("version"))
        .collect())
}

async fn applied_pg_migrations(pool: &PgPool) -> anyhow::Result<Vec<i64>> {
    let rows = sqlx::query("SELECT version FROM qpayd_schema_migrations ORDER BY version")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| row.get::<i64, _>("version"))
        .collect())
}

fn insert_event_query(
    event: &EventEnvelope,
) -> sqlx::query::Query<'_, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'_>> {
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO events (id, store_id, invoice_id, type, payload_json, created_at)
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&event.id)
    .bind(&event.store_id)
    .bind(event.invoice_id.map(|id| id.to_string()))
    .bind(&event.event_type)
    .bind(serde_json::to_string(event).expect("event serializes"))
    .bind(event.created_at.to_rfc3339())
}

fn enqueue_webhook_query<'a>(
    event_id: &'a str,
    store_id: &'a str,
    url: &'a str,
    now: DateTime<Utc>,
) -> sqlx::query::Query<'a, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'a>> {
    sqlx::query(
        r#"
        INSERT INTO webhook_deliveries (
            event_id, store_id, url, status, attempts, next_attempt_at, created_at, updated_at
        ) VALUES (?, ?, ?, 'pending', 0, ?, ?, ?)
        "#,
    )
    .bind(event_id)
    .bind(store_id)
    .bind(url)
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
}

fn insert_pg_event_query(
    event: &EventEnvelope,
) -> sqlx::query::Query<'_, sqlx::Postgres, sqlx::postgres::PgArguments> {
    sqlx::query(
        r#"
        INSERT INTO qpayd_events (id, store_id, invoice_id, type, payload_json, created_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT(id) DO NOTHING
        "#,
    )
    .bind(&event.id)
    .bind(&event.store_id)
    .bind(event.invoice_id.map(|id| id.to_string()))
    .bind(&event.event_type)
    .bind(serde_json::to_string(event).expect("event serializes"))
    .bind(event.created_at.to_rfc3339())
}

fn enqueue_pg_webhook_query<'a>(
    event_id: &'a str,
    store_id: &'a str,
    url: &'a str,
    now: DateTime<Utc>,
) -> sqlx::query::Query<'a, sqlx::Postgres, sqlx::postgres::PgArguments> {
    sqlx::query(
        r#"
        INSERT INTO qpayd_webhook_deliveries (
            event_id, store_id, url, status, attempts, next_attempt_at, created_at, updated_at
        ) VALUES ($1, $2, $3, 'pending', 0, $4, $5, $6)
        "#,
    )
    .bind(event_id)
    .bind(store_id)
    .bind(url)
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
}

fn invoice_from_row(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<Invoice> {
    Ok(Invoice {
        id: Uuid::parse_str(row.get::<String, _>("id").as_str())?,
        store_id: row.get("store_id"),
        status: InvoiceStatus::try_from(row.get::<String, _>("status").as_str())?,
        amount: row.get::<String, _>("amount").parse::<Decimal>()?,
        currency: row.get("currency"),
        btc_amount_sats: row.get::<i64, _>("btc_amount_sats") as u64,
        paid_sats: row.get::<i64, _>("paid_sats") as u64,
        confirmed_sats: row.get::<i64, _>("confirmed_sats") as u64,
        unconfirmed_sats: row.get::<i64, _>("unconfirmed_sats") as u64,
        onchain_address: row.get("onchain_address"),
        onchain_address_index: row
            .get::<Option<i64>, _>("onchain_address_index")
            .map(|index| index as u32),
        onchain_script_pubkey: row.get("onchain_script_pubkey"),
        lightning_bolt11: row.get("lightning_bolt11"),
        lightning_payment_hash: row.get("lightning_payment_hash"),
        idempotency_key: row.get("idempotency_key"),
        payment_link_id: row.get("payment_link_id"),
        rate_source: row.get("rate_source"),
        rate: row.get::<String, _>("rate").parse::<Decimal>()?,
        metadata: serde_json::from_str(row.get::<String, _>("metadata").as_str())?,
        expires_at: DateTime::parse_from_rfc3339(row.get::<String, _>("expires_at").as_str())?
            .with_timezone(&Utc),
        created_at: DateTime::parse_from_rfc3339(row.get::<String, _>("created_at").as_str())?
            .with_timezone(&Utc),
        updated_at: DateTime::parse_from_rfc3339(row.get::<String, _>("updated_at").as_str())?
            .with_timezone(&Utc),
    })
}

fn invoice_from_pg_row(row: sqlx::postgres::PgRow) -> anyhow::Result<Invoice> {
    Ok(Invoice {
        id: Uuid::parse_str(row.get::<String, _>("id").as_str())?,
        store_id: row.get("store_id"),
        status: InvoiceStatus::try_from(row.get::<String, _>("status").as_str())?,
        amount: row.get::<String, _>("amount").parse::<Decimal>()?,
        currency: row.get("currency"),
        btc_amount_sats: row.get::<i64, _>("btc_amount_sats") as u64,
        paid_sats: row.get::<i64, _>("paid_sats") as u64,
        confirmed_sats: row.get::<i64, _>("confirmed_sats") as u64,
        unconfirmed_sats: row.get::<i64, _>("unconfirmed_sats") as u64,
        onchain_address: row.get("onchain_address"),
        onchain_address_index: row
            .get::<Option<i64>, _>("onchain_address_index")
            .map(|index| index as u32),
        onchain_script_pubkey: row.get("onchain_script_pubkey"),
        lightning_bolt11: row.get("lightning_bolt11"),
        lightning_payment_hash: row.get("lightning_payment_hash"),
        idempotency_key: row.get("idempotency_key"),
        payment_link_id: row.get("payment_link_id"),
        rate_source: row.get("rate_source"),
        rate: row.get::<String, _>("rate").parse::<Decimal>()?,
        metadata: serde_json::from_str(row.get::<String, _>("metadata").as_str())?,
        expires_at: DateTime::parse_from_rfc3339(row.get::<String, _>("expires_at").as_str())?
            .with_timezone(&Utc),
        created_at: DateTime::parse_from_rfc3339(row.get::<String, _>("created_at").as_str())?
            .with_timezone(&Utc),
        updated_at: DateTime::parse_from_rfc3339(row.get::<String, _>("updated_at").as_str())?
            .with_timezone(&Utc),
    })
}

fn refund_from_row(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<Refund> {
    refund_from_fields(
        row.get("id"),
        row.get("store_id"),
        row.get("invoice_id"),
        row.get("status"),
        row.get("approval_status"),
        row.get::<i64, _>("amount_sats"),
        row.get("destination"),
        row.get("destination_type"),
        row.get("reason"),
        row.get("tx_id"),
        row.get("payment_proof"),
        row.get("failure_reason"),
        row.get("idempotency_key"),
        row.get("metadata"),
        row.get("created_at"),
        row.get("updated_at"),
        row.get("finalized_at"),
    )
}

fn refund_from_pg_row(row: sqlx::postgres::PgRow) -> anyhow::Result<Refund> {
    refund_from_fields(
        row.get("id"),
        row.get("store_id"),
        row.get("invoice_id"),
        row.get("status"),
        row.get("approval_status"),
        row.get::<i64, _>("amount_sats"),
        row.get("destination"),
        row.get("destination_type"),
        row.get("reason"),
        row.get("tx_id"),
        row.get("payment_proof"),
        row.get("failure_reason"),
        row.get("idempotency_key"),
        row.get("metadata"),
        row.get("created_at"),
        row.get("updated_at"),
        row.get("finalized_at"),
    )
}

fn refund_execution_candidate_from_row(
    row: sqlx::sqlite::SqliteRow,
) -> anyhow::Result<RefundExecutionCandidate> {
    Ok(RefundExecutionCandidate {
        refund: refund_from_aliased_fields(&row)?,
        invoice: invoice_from_aliased_fields(&row)?,
    })
}

fn refund_execution_candidate_from_pg_row(
    row: sqlx::postgres::PgRow,
) -> anyhow::Result<RefundExecutionCandidate> {
    Ok(RefundExecutionCandidate {
        refund: refund_from_aliased_pg_fields(&row)?,
        invoice: invoice_from_aliased_pg_fields(&row)?,
    })
}

fn refund_from_aliased_fields(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<Refund> {
    refund_from_fields(
        row.get("refund_id"),
        row.get("refund_store_id"),
        row.get("refund_invoice_id"),
        row.get("refund_status"),
        row.get("refund_approval_status"),
        row.get::<i64, _>("refund_amount_sats"),
        row.get("refund_destination"),
        row.get("refund_destination_type"),
        row.get("refund_reason"),
        row.get("refund_tx_id"),
        row.get("refund_payment_proof"),
        row.get("refund_failure_reason"),
        row.get("refund_idempotency_key"),
        row.get("refund_metadata"),
        row.get("refund_created_at"),
        row.get("refund_updated_at"),
        row.get("refund_finalized_at"),
    )
}

fn refund_from_aliased_pg_fields(row: &sqlx::postgres::PgRow) -> anyhow::Result<Refund> {
    refund_from_fields(
        row.get("refund_id"),
        row.get("refund_store_id"),
        row.get("refund_invoice_id"),
        row.get("refund_status"),
        row.get("refund_approval_status"),
        row.get::<i64, _>("refund_amount_sats"),
        row.get("refund_destination"),
        row.get("refund_destination_type"),
        row.get("refund_reason"),
        row.get("refund_tx_id"),
        row.get("refund_payment_proof"),
        row.get("refund_failure_reason"),
        row.get("refund_idempotency_key"),
        row.get("refund_metadata"),
        row.get("refund_created_at"),
        row.get("refund_updated_at"),
        row.get("refund_finalized_at"),
    )
}

fn invoice_from_aliased_fields(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<Invoice> {
    invoice_from_fields(
        row.get("invoice_id"),
        row.get("invoice_store_id"),
        row.get("invoice_status"),
        row.get("invoice_amount"),
        row.get("invoice_currency"),
        row.get::<i64, _>("invoice_btc_amount_sats"),
        row.get::<i64, _>("invoice_paid_sats"),
        row.get::<i64, _>("invoice_confirmed_sats"),
        row.get::<i64, _>("invoice_unconfirmed_sats"),
        row.get("invoice_onchain_address"),
        row.get::<Option<i64>, _>("invoice_onchain_address_index"),
        row.get("invoice_onchain_script_pubkey"),
        row.get("invoice_lightning_bolt11"),
        row.get("invoice_lightning_payment_hash"),
        row.get("invoice_idempotency_key"),
        row.get("invoice_payment_link_id"),
        row.get("invoice_rate_source"),
        row.get("invoice_rate"),
        row.get("invoice_metadata"),
        row.get("invoice_expires_at"),
        row.get("invoice_created_at"),
        row.get("invoice_updated_at"),
    )
}

fn invoice_from_aliased_pg_fields(row: &sqlx::postgres::PgRow) -> anyhow::Result<Invoice> {
    invoice_from_fields(
        row.get("invoice_id"),
        row.get("invoice_store_id"),
        row.get("invoice_status"),
        row.get("invoice_amount"),
        row.get("invoice_currency"),
        row.get::<i64, _>("invoice_btc_amount_sats"),
        row.get::<i64, _>("invoice_paid_sats"),
        row.get::<i64, _>("invoice_confirmed_sats"),
        row.get::<i64, _>("invoice_unconfirmed_sats"),
        row.get("invoice_onchain_address"),
        row.get::<Option<i64>, _>("invoice_onchain_address_index"),
        row.get("invoice_onchain_script_pubkey"),
        row.get("invoice_lightning_bolt11"),
        row.get("invoice_lightning_payment_hash"),
        row.get("invoice_idempotency_key"),
        row.get("invoice_payment_link_id"),
        row.get("invoice_rate_source"),
        row.get("invoice_rate"),
        row.get("invoice_metadata"),
        row.get("invoice_expires_at"),
        row.get("invoice_created_at"),
        row.get("invoice_updated_at"),
    )
}

#[allow(clippy::too_many_arguments)]
fn invoice_from_fields(
    id: String,
    store_id: String,
    status: String,
    amount: String,
    currency: String,
    btc_amount_sats: i64,
    paid_sats: i64,
    confirmed_sats: i64,
    unconfirmed_sats: i64,
    onchain_address: Option<String>,
    onchain_address_index: Option<i64>,
    onchain_script_pubkey: Option<String>,
    lightning_bolt11: Option<String>,
    lightning_payment_hash: Option<String>,
    idempotency_key: Option<String>,
    payment_link_id: Option<String>,
    rate_source: String,
    rate: String,
    metadata: String,
    expires_at: String,
    created_at: String,
    updated_at: String,
) -> anyhow::Result<Invoice> {
    Ok(Invoice {
        id: Uuid::parse_str(&id)?,
        store_id,
        status: InvoiceStatus::try_from(status.as_str())?,
        amount: amount.parse::<Decimal>()?,
        currency,
        btc_amount_sats: btc_amount_sats as u64,
        paid_sats: paid_sats as u64,
        confirmed_sats: confirmed_sats as u64,
        unconfirmed_sats: unconfirmed_sats as u64,
        onchain_address,
        onchain_address_index: onchain_address_index.map(|index| index as u32),
        onchain_script_pubkey,
        lightning_bolt11,
        lightning_payment_hash,
        idempotency_key,
        payment_link_id,
        rate_source,
        rate: rate.parse::<Decimal>()?,
        metadata: serde_json::from_str(&metadata)?,
        expires_at: DateTime::parse_from_rfc3339(&expires_at)?.with_timezone(&Utc),
        created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)?.with_timezone(&Utc),
    })
}

#[allow(clippy::too_many_arguments)]
fn refund_from_fields(
    id: String,
    store_id: String,
    invoice_id: String,
    status: String,
    approval_status: String,
    amount_sats: i64,
    destination: Option<String>,
    destination_type: Option<String>,
    reason: Option<String>,
    tx_id: Option<String>,
    payment_proof: Option<String>,
    failure_reason: Option<String>,
    idempotency_key: Option<String>,
    metadata: String,
    created_at: String,
    updated_at: String,
    finalized_at: Option<String>,
) -> anyhow::Result<Refund> {
    Ok(Refund {
        id: Uuid::parse_str(&id)?,
        store_id,
        invoice_id: Uuid::parse_str(&invoice_id)?,
        status: RefundStatus::try_from(status.as_str())?,
        approval_status: RefundApprovalStatus::try_from(approval_status.as_str())?,
        amount_sats: amount_sats as u64,
        destination,
        destination_type: destination_type
            .as_deref()
            .map(RefundDestinationType::try_from)
            .transpose()?,
        reason,
        tx_id,
        payment_proof,
        failure_reason,
        idempotency_key,
        metadata: serde_json::from_str(&metadata)?,
        created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)?.with_timezone(&Utc),
        finalized_at: finalized_at
            .map(|value| DateTime::parse_from_rfc3339(&value).map(|time| time.with_timezone(&Utc)))
            .transpose()?,
    })
}

fn lightning_sweep_from_row(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<LightningSweepRecord> {
    lightning_sweep_from_fields(
        row.get("id"),
        row.get("store_id"),
        row.get("backend"),
        row.get("status"),
        row.get::<i64, _>("balance_sats"),
        row.get::<i64, _>("amount_sats"),
        row.get("address"),
        row.get("tx_id"),
        row.get("error"),
        row.get("created_at"),
        row.get("updated_at"),
    )
}

fn lightning_sweep_from_pg_row(row: sqlx::postgres::PgRow) -> anyhow::Result<LightningSweepRecord> {
    lightning_sweep_from_fields(
        row.get("id"),
        row.get("store_id"),
        row.get("backend"),
        row.get("status"),
        row.get::<i64, _>("balance_sats"),
        row.get::<i64, _>("amount_sats"),
        row.get("address"),
        row.get("tx_id"),
        row.get("error"),
        row.get("created_at"),
        row.get("updated_at"),
    )
}

#[allow(clippy::too_many_arguments)]
fn lightning_sweep_from_fields(
    id: String,
    store_id: String,
    backend: String,
    status: String,
    balance_sats: i64,
    amount_sats: i64,
    address: String,
    tx_id: Option<String>,
    error: Option<String>,
    created_at: String,
    updated_at: String,
) -> anyhow::Result<LightningSweepRecord> {
    Ok(LightningSweepRecord {
        id: Uuid::parse_str(&id)?,
        store_id,
        backend,
        status: SweepStatus::try_from(status.as_str())?,
        balance_sats: balance_sats as u64,
        amount_sats: amount_sats as u64,
        address,
        tx_id,
        error,
        created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)?.with_timezone(&Utc),
    })
}

fn event_from_row(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<EventEnvelope> {
    event_from_row_ref(&row)
}

fn event_from_row_ref(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<EventEnvelope> {
    serde_json::from_str(row.get::<String, _>("payload_json").as_str()).map_err(anyhow::Error::from)
}

fn event_from_pg_row(row: sqlx::postgres::PgRow) -> anyhow::Result<EventEnvelope> {
    event_from_pg_row_ref(&row)
}

fn event_from_pg_row_ref(row: &sqlx::postgres::PgRow) -> anyhow::Result<EventEnvelope> {
    serde_json::from_str(row.get::<String, _>("payload_json").as_str()).map_err(anyhow::Error::from)
}

fn webhook_delivery_from_row(
    row: sqlx::sqlite::SqliteRow,
) -> anyhow::Result<QueuedWebhookDelivery> {
    Ok(QueuedWebhookDelivery {
        id: row.get("delivery_id"),
        event: event_from_row_ref(&row)?,
        url: row.get("delivery_url"),
        attempts: row.get::<i64, _>("delivery_attempts") as u32,
    })
}

fn pg_webhook_delivery_from_row(
    row: sqlx::postgres::PgRow,
) -> anyhow::Result<QueuedWebhookDelivery> {
    Ok(QueuedWebhookDelivery {
        id: row.get("delivery_id"),
        event: event_from_pg_row_ref(&row)?,
        url: row.get("delivery_url"),
        attempts: row.get::<i64, _>("delivery_attempts") as u32,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use rust_decimal::Decimal;
    use sqlx::Row;
    use uuid::Uuid;

    use super::{PostgresStore, SqliteStore, Store};
    use crate::{
        events::{invoice_created_event, invoice_status_event},
        invoice::{
            Invoice, InvoiceStatus, InvoiceStatusUpdate, LightningSweepRecord, PaymentAmounts,
            Refund, RefundApprovalStatus, RefundDestinationType, RefundStatus, SweepStatus,
        },
    };

    #[tokio::test]
    async fn insert_invoice_persists_event_and_webhook_delivery() {
        let store = test_store().await;
        insert_invoice_persists_event_and_webhook_delivery_for(store.as_ref()).await;
    }

    #[tokio::test]
    async fn replay_adds_a_fresh_delivery_for_existing_event() {
        let store = test_store().await;
        replay_adds_a_fresh_delivery_for_existing_event_for(store.as_ref()).await;
    }

    #[tokio::test]
    async fn duplicate_status_event_does_not_enqueue_twice() {
        let store = test_store().await;
        duplicate_status_event_does_not_enqueue_twice_for(store.as_ref()).await;
    }

    #[tokio::test]
    async fn idempotency_key_finds_original_invoice() {
        let store = test_store().await;
        idempotency_key_finds_original_invoice_for(store.as_ref()).await;
    }

    #[tokio::test]
    async fn payment_link_idempotency_keys_are_scoped() {
        let store = test_store().await;
        payment_link_idempotency_keys_are_scoped_for(store.as_ref()).await;
    }

    #[tokio::test]
    async fn expirable_invoices_include_only_unpaid_due_invoices() {
        let store = test_store().await;
        expirable_invoices_include_only_unpaid_due_invoices_for(store.as_ref()).await;
    }

    #[tokio::test]
    async fn updates_payment_amounts_without_status_event() {
        let store = test_store().await;
        updates_payment_amounts_without_status_event_for(store.as_ref()).await;
    }

    #[tokio::test]
    async fn address_indexes_are_persisted_per_store() {
        let store = test_store().await;
        address_indexes_are_persisted_per_store_for(store.as_ref()).await;
    }

    #[tokio::test]
    async fn refund_and_sweep_records_round_trip() {
        let store = test_store().await;
        refund_and_sweep_records_round_trip_for(store.as_ref()).await;
    }

    #[tokio::test]
    async fn refund_execution_claims_are_leased() {
        let store = test_store().await;
        refund_execution_claims_are_leased_for(store.as_ref()).await;
    }

    #[tokio::test]
    async fn refund_execution_start_reserves_daily_limit() {
        let store = test_store().await;
        refund_execution_start_reserves_daily_limit_for(store.as_ref()).await;
    }

    #[tokio::test]
    async fn sqlite_migrate_records_initial_schema_once() {
        let path = std::env::temp_dir().join(format!("qpayd-migration-test-{}.db", Uuid::new_v4()));
        let store = SqliteStore::connect(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();

        store.migrate().await.unwrap();
        store.migrate().await.unwrap();

        let rows = sqlx::query("SELECT version, name FROM schema_migrations ORDER BY version")
            .fetch_all(&store.pool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 8);
        assert_eq!(rows[0].get::<i64, _>("version"), 1);
        assert_eq!(rows[0].get::<String, _>("name"), "initial_schema");
        assert_eq!(rows[1].get::<i64, _>("version"), 2);
        assert_eq!(rows[1].get::<String, _>("name"), "invoice_idempotency_keys");
        assert_eq!(rows[2].get::<i64, _>("version"), 3);
        assert_eq!(rows[2].get::<String, _>("name"), "invoice_payment_amounts");
        assert_eq!(rows[3].get::<i64, _>("version"), 4);
        assert_eq!(rows[3].get::<String, _>("name"), "merchant_admin_records");
        assert_eq!(rows[4].get::<i64, _>("version"), 5);
        assert_eq!(
            rows[4].get::<String, _>("name"),
            "invoice_payment_link_idempotency_scope"
        );
        assert_eq!(rows[5].get::<i64, _>("version"), 6);
        assert_eq!(
            rows[5].get::<String, _>("name"),
            "refund_idempotency_and_proofs"
        );
        assert_eq!(rows[6].get::<i64, _>("version"), 7);
        assert_eq!(rows[6].get::<String, _>("name"), "refund_execution_claims");
        assert_eq!(rows[7].get::<i64, _>("version"), 8);
        assert_eq!(rows[7].get::<String, _>("name"), "refund_manual_approval");
    }

    #[tokio::test]
    async fn postgres_storage_contract() {
        let Some(store) = pg_test_store().await else {
            return;
        };

        let rows =
            sqlx::query("SELECT version, name FROM qpayd_schema_migrations ORDER BY version")
                .fetch_all(&store.pool)
                .await
                .unwrap();
        assert_eq!(rows.len(), 8);
        assert_eq!(rows[0].get::<i64, _>("version"), 1);
        assert_eq!(rows[0].get::<String, _>("name"), "initial_schema");
        assert_eq!(rows[1].get::<i64, _>("version"), 2);
        assert_eq!(rows[1].get::<String, _>("name"), "invoice_idempotency_keys");
        assert_eq!(rows[2].get::<i64, _>("version"), 3);
        assert_eq!(rows[2].get::<String, _>("name"), "invoice_payment_amounts");
        assert_eq!(rows[3].get::<i64, _>("version"), 4);
        assert_eq!(rows[3].get::<String, _>("name"), "merchant_admin_records");
        assert_eq!(rows[4].get::<i64, _>("version"), 5);
        assert_eq!(
            rows[4].get::<String, _>("name"),
            "invoice_payment_link_idempotency_scope"
        );
        assert_eq!(rows[5].get::<i64, _>("version"), 6);
        assert_eq!(
            rows[5].get::<String, _>("name"),
            "refund_idempotency_and_proofs"
        );
        assert_eq!(rows[6].get::<i64, _>("version"), 7);
        assert_eq!(rows[6].get::<String, _>("name"), "refund_execution_claims");
        assert_eq!(rows[7].get::<i64, _>("version"), 8);
        assert_eq!(rows[7].get::<String, _>("name"), "refund_manual_approval");

        clean_pg_store(&store).await;
        insert_invoice_persists_event_and_webhook_delivery_for(&store).await;

        clean_pg_store(&store).await;
        replay_adds_a_fresh_delivery_for_existing_event_for(&store).await;

        clean_pg_store(&store).await;
        duplicate_status_event_does_not_enqueue_twice_for(&store).await;

        clean_pg_store(&store).await;
        idempotency_key_finds_original_invoice_for(&store).await;

        clean_pg_store(&store).await;
        payment_link_idempotency_keys_are_scoped_for(&store).await;

        clean_pg_store(&store).await;
        expirable_invoices_include_only_unpaid_due_invoices_for(&store).await;

        clean_pg_store(&store).await;
        updates_payment_amounts_without_status_event_for(&store).await;

        clean_pg_store(&store).await;
        address_indexes_are_persisted_per_store_for(&store).await;

        clean_pg_store(&store).await;
        refund_and_sweep_records_round_trip_for(&store).await;

        clean_pg_store(&store).await;
        refund_execution_claims_are_leased_for(&store).await;

        clean_pg_store(&store).await;
        refund_execution_start_reserves_daily_limit_for(&store).await;
    }

    async fn insert_invoice_persists_event_and_webhook_delivery_for(store: &dyn Store) {
        let invoice = test_invoice(InvoiceStatus::New);
        let event = invoice_created_event(&invoice, invoice.created_at);

        store
            .insert_invoice(&invoice, &event, Some("https://example.com/webhook"))
            .await
            .unwrap();

        let events = store.events(&invoice.store_id, 10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, event.id);
        assert_eq!(events[0].data["paid_sats"], 0);
        assert_eq!(events[0].data["remaining_sats"], invoice.btc_amount_sats);
        assert_eq!(events[0].data["overpaid_sats"], 0);

        let due = store.due_webhook_deliveries(10, Utc::now()).await.unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].event.id, event.id);
        assert_eq!(due[0].url, "https://example.com/webhook");
    }

    async fn replay_adds_a_fresh_delivery_for_existing_event_for(store: &dyn Store) {
        let invoice = test_invoice(InvoiceStatus::New);
        let event = invoice_created_event(&invoice, invoice.created_at);

        store
            .insert_invoice(&invoice, &event, Some("https://example.com/webhook"))
            .await
            .unwrap();
        store
            .enqueue_webhook_delivery(
                &event.id,
                &invoice.store_id,
                "https://example.com/webhook",
                Utc::now(),
            )
            .await
            .unwrap();

        let due = store.due_webhook_deliveries(10, Utc::now()).await.unwrap();
        assert_eq!(due.len(), 2);
    }

    async fn duplicate_status_event_does_not_enqueue_twice_for(store: &dyn Store) {
        let invoice = test_invoice(InvoiceStatus::New);
        let created = invoice_created_event(&invoice, invoice.created_at);
        store
            .insert_invoice(&invoice, &created, Some("https://example.com/webhook"))
            .await
            .unwrap();

        let updated_at = Utc::now();
        let settled = invoice_status_event(&invoice, InvoiceStatus::Settled, updated_at);
        store
            .update_invoice_status(
                &invoice.store_id,
                invoice.id,
                InvoiceStatusUpdate {
                    status: InvoiceStatus::Settled,
                    payment: PaymentAmounts {
                        paid_sats: 10_000,
                        confirmed_sats: 10_000,
                        unconfirmed_sats: 0,
                    },
                    updated_at,
                },
                &settled,
                Some("https://example.com/webhook"),
            )
            .await
            .unwrap();
        store
            .update_invoice_status(
                &invoice.store_id,
                invoice.id,
                InvoiceStatusUpdate {
                    status: InvoiceStatus::Settled,
                    payment: PaymentAmounts {
                        paid_sats: 10_000,
                        confirmed_sats: 10_000,
                        unconfirmed_sats: 0,
                    },
                    updated_at,
                },
                &settled,
                Some("https://example.com/webhook"),
            )
            .await
            .unwrap();

        let events = store.events(&invoice.store_id, 10).await.unwrap();
        assert_eq!(events.len(), 2);

        let due = store.due_webhook_deliveries(10, Utc::now()).await.unwrap();
        assert_eq!(due.len(), 2);
    }

    async fn idempotency_key_finds_original_invoice_for(store: &dyn Store) {
        let mut invoice = test_invoice(InvoiceStatus::New);
        invoice.idempotency_key = Some("retry-key-1".to_string());
        let event = invoice_created_event(&invoice, invoice.created_at);

        store.insert_invoice(&invoice, &event, None).await.unwrap();

        let found = store
            .invoice_by_idempotency_key(&invoice.store_id, "retry-key-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, invoice.id);
        assert_eq!(found.onchain_address_index, invoice.onchain_address_index);
        assert_eq!(found.idempotency_key.as_deref(), Some("retry-key-1"));
    }

    async fn payment_link_idempotency_keys_are_scoped_for(store: &dyn Store) {
        let mut admin_invoice = test_invoice(InvoiceStatus::New);
        admin_invoice.idempotency_key = Some("shared-key".to_string());
        let admin_event = invoice_created_event(&admin_invoice, admin_invoice.created_at);
        store
            .insert_invoice(&admin_invoice, &admin_event, None)
            .await
            .unwrap();

        let mut link_invoice = test_invoice(InvoiceStatus::New);
        link_invoice.idempotency_key = Some("shared-key".to_string());
        link_invoice.payment_link_id = Some("donate-10".to_string());
        let link_event = invoice_created_event(&link_invoice, link_invoice.created_at);
        store
            .insert_invoice(&link_invoice, &link_event, None)
            .await
            .unwrap();

        let found_admin = store
            .invoice_by_idempotency_key(&admin_invoice.store_id, "shared-key")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found_admin.id, admin_invoice.id);

        let found_link = store
            .invoice_by_payment_link_idempotency_key(
                &link_invoice.store_id,
                "donate-10",
                "shared-key",
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found_link.id, link_invoice.id);

        let wrong_link = store
            .invoice_by_payment_link_idempotency_key(
                &link_invoice.store_id,
                "other-link",
                "shared-key",
            )
            .await
            .unwrap();
        assert!(wrong_link.is_none());
    }

    async fn expirable_invoices_include_only_unpaid_due_invoices_for(store: &dyn Store) {
        let now = Utc::now();
        let mut due_new = test_invoice(InvoiceStatus::New);
        due_new.expires_at = now - Duration::minutes(1);
        due_new.created_at = now - Duration::minutes(20);
        due_new.updated_at = due_new.created_at;

        let mut due_partial = test_invoice(InvoiceStatus::PartiallyPaid);
        due_partial.id = Uuid::new_v4();
        due_partial.expires_at = now - Duration::minutes(1);
        due_partial.created_at = now - Duration::minutes(20);
        due_partial.updated_at = due_partial.created_at;

        let mut payment_detected = test_invoice(InvoiceStatus::PaymentDetected);
        payment_detected.id = Uuid::new_v4();
        payment_detected.expires_at = now - Duration::minutes(1);
        payment_detected.created_at = now - Duration::minutes(20);
        payment_detected.updated_at = payment_detected.created_at;

        let mut not_due = test_invoice(InvoiceStatus::New);
        not_due.id = Uuid::new_v4();
        not_due.expires_at = now + Duration::minutes(1);

        for invoice in [&due_new, &due_partial, &payment_detected, &not_due] {
            let event = invoice_created_event(invoice, invoice.created_at);
            store.insert_invoice(invoice, &event, None).await.unwrap();
        }

        let mut ids = store
            .expirable_invoices("main", now)
            .await
            .unwrap()
            .into_iter()
            .map(|invoice| invoice.id)
            .collect::<Vec<_>>();
        ids.sort();

        let mut expected = vec![due_new.id, due_partial.id];
        expected.sort();
        assert_eq!(ids, expected);
    }

    async fn updates_payment_amounts_without_status_event_for(store: &dyn Store) {
        let invoice = test_invoice(InvoiceStatus::PartiallyPaid);
        let event = invoice_created_event(&invoice, invoice.created_at);
        store.insert_invoice(&invoice, &event, None).await.unwrap();

        let updated_at = Utc::now();
        store
            .update_invoice_payment_amounts(
                &invoice.store_id,
                invoice.id,
                PaymentAmounts {
                    paid_sats: 12_000,
                    confirmed_sats: 10_000,
                    unconfirmed_sats: 2_000,
                },
                updated_at,
            )
            .await
            .unwrap();

        let found = store
            .invoice(&invoice.store_id, invoice.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, InvoiceStatus::PartiallyPaid);
        assert_eq!(found.paid_sats, 12_000);
        assert_eq!(found.confirmed_sats, 10_000);
        assert_eq!(found.unconfirmed_sats, 2_000);

        let events = store.events(&invoice.store_id, 10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "invoice.created");
    }

    async fn address_indexes_are_persisted_per_store_for(store: &dyn Store) {
        assert_eq!(store.reserve_address_index("main").await.unwrap(), 0);
        assert_eq!(store.reserve_address_index("main").await.unwrap(), 1);
        assert_eq!(store.reserve_address_index("secondary").await.unwrap(), 0);
        assert_eq!(store.reserve_address_index("main").await.unwrap(), 2);
        assert_eq!(store.reserve_address_index("secondary").await.unwrap(), 1);
    }

    async fn refund_and_sweep_records_round_trip_for(store: &dyn Store) {
        let mut invoice = test_invoice(InvoiceStatus::Settled);
        invoice.paid_sats = 12_000;
        invoice.confirmed_sats = 12_000;
        let invoice_event = invoice_created_event(&invoice, invoice.created_at);
        store
            .insert_invoice(&invoice, &invoice_event, None)
            .await
            .unwrap();

        let now = Utc::now();
        let mut refund = Refund {
            id: Uuid::new_v4(),
            store_id: invoice.store_id.clone(),
            invoice_id: invoice.id,
            status: RefundStatus::Pending,
            approval_status: RefundApprovalStatus::NotRequired,
            amount_sats: 2_000,
            destination: Some("bc1qrefund".to_string()),
            destination_type: Some(RefundDestinationType::BitcoinAddress),
            reason: Some("overpayment".to_string()),
            tx_id: None,
            payment_proof: None,
            failure_reason: None,
            idempotency_key: Some("refund-key-1".to_string()),
            metadata: serde_json::json!({ "operator": "test" }),
            created_at: now,
            updated_at: now,
            finalized_at: None,
        };
        let refund_event = crate::events::refund_created_event(&refund, now);
        store
            .insert_refund(&refund, &refund_event, Some("https://example.com/webhook"))
            .await
            .unwrap();
        refund.status = RefundStatus::Succeeded;
        refund.tx_id = Some("tx123".to_string());
        refund.payment_proof = Some("proof123".to_string());
        refund.updated_at = Utc::now();
        refund.finalized_at = Some(refund.updated_at);
        let finalized = crate::events::refund_finalized_event(&refund, refund.updated_at);
        store
            .update_refund_status(&refund, &finalized, Some("https://example.com/webhook"))
            .await
            .unwrap();
        let found = store
            .refund(&refund.store_id, refund.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, RefundStatus::Succeeded);
        assert_eq!(found.tx_id.as_deref(), Some("tx123"));
        assert_eq!(found.payment_proof.as_deref(), Some("proof123"));
        assert_eq!(found.idempotency_key.as_deref(), Some("refund-key-1"));
        assert_eq!(store.refunds(&refund.store_id, 10).await.unwrap().len(), 1);
        let idempotent = store
            .refund_by_idempotency_key(&refund.store_id, "refund-key-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(idempotent.id, refund.id);
        let invoice_refunds = store
            .refunds_for_invoice(&refund.store_id, invoice.id)
            .await
            .unwrap();
        assert_eq!(invoice_refunds.len(), 1);
        assert_eq!(invoice_refunds[0].id, refund.id);

        let sweep = LightningSweepRecord {
            id: Uuid::new_v4(),
            store_id: invoice.store_id.clone(),
            backend: "barkd".to_string(),
            status: SweepStatus::Succeeded,
            balance_sats: 150_000,
            amount_sats: 125_000,
            address: "bc1qsweep".to_string(),
            tx_id: Some("sweeptx".to_string()),
            error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        store.insert_lightning_sweep(&sweep).await.unwrap();
        let sweeps = store.lightning_sweeps(&sweep.store_id, 10).await.unwrap();
        assert_eq!(sweeps.len(), 1);
        assert_eq!(sweeps[0].amount_sats, 125_000);
    }

    async fn refund_execution_claims_are_leased_for(store: &dyn Store) {
        let mut invoice = test_invoice(InvoiceStatus::Settled);
        invoice.paid_sats = 12_000;
        invoice.confirmed_sats = 12_000;
        let invoice_event = invoice_created_event(&invoice, invoice.created_at);
        store
            .insert_invoice(&invoice, &invoice_event, None)
            .await
            .unwrap();

        let now = Utc::now();
        let refund = Refund {
            id: Uuid::new_v4(),
            store_id: invoice.store_id.clone(),
            invoice_id: invoice.id,
            status: RefundStatus::Pending,
            approval_status: RefundApprovalStatus::NotRequired,
            amount_sats: 2_000,
            destination: Some("bc1qrefund".to_string()),
            destination_type: Some(RefundDestinationType::BitcoinAddress),
            reason: None,
            tx_id: None,
            payment_proof: None,
            failure_reason: None,
            idempotency_key: None,
            metadata: serde_json::json!({}),
            created_at: now,
            updated_at: now,
            finalized_at: None,
        };
        let refund_event = crate::events::refund_created_event(&refund, now);
        store
            .insert_refund(&refund, &refund_event, None)
            .await
            .unwrap();

        let pending = store
            .pending_refund_executions(&refund.store_id, 10, now)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].refund.id, refund.id);
        assert_eq!(pending[0].invoice.id, invoice.id);

        let lease_until = now + Duration::minutes(5);
        let claimed = store
            .claim_refund_execution(&refund.store_id, refund.id, "worker-1", lease_until, now)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.refund.id, refund.id);

        let pending = store
            .pending_refund_executions(&refund.store_id, 10, now)
            .await
            .unwrap();
        assert!(pending.is_empty());

        let second_claim = store
            .claim_refund_execution(&refund.store_id, refund.id, "worker-2", lease_until, now)
            .await
            .unwrap();
        assert!(second_claim.is_none());

        let after_lease = lease_until + Duration::seconds(1);
        let pending = store
            .pending_refund_executions(&refund.store_id, 10, after_lease)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);

        let reclaimed = store
            .claim_refund_execution(
                &refund.store_id,
                refund.id,
                "worker-2",
                after_lease + Duration::minutes(5),
                after_lease,
            )
            .await
            .unwrap();
        assert!(reclaimed.is_some());
    }

    async fn refund_execution_start_reserves_daily_limit_for(store: &dyn Store) {
        let mut invoice = test_invoice(InvoiceStatus::Settled);
        invoice.paid_sats = 12_000;
        invoice.confirmed_sats = 12_000;
        let invoice_event = invoice_created_event(&invoice, invoice.created_at);
        store
            .insert_invoice(&invoice, &invoice_event, None)
            .await
            .unwrap();

        let now = Utc::now();
        let refund_one = test_refund_for_invoice(&invoice, now);
        let mut refund_two = test_refund_for_invoice(&invoice, now);
        refund_two.id = Uuid::new_v4();
        for refund in [&refund_one, &refund_two] {
            let event = crate::events::refund_created_event(refund, refund.created_at);
            store.insert_refund(refund, &event, None).await.unwrap();
        }

        let day_start = now
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .expect("midnight is valid")
            .and_utc();
        let mut processing_one = refund_one.clone();
        processing_one.status = RefundStatus::Processing;
        processing_one.updated_at = now;
        let event_one = crate::events::refund_processing_event(&processing_one, now);
        assert!(
            store
                .try_start_refund_execution(&processing_one, day_start, 3_000, &event_one, None)
                .await
                .unwrap()
        );

        let mut processing_two = refund_two.clone();
        processing_two.status = RefundStatus::Processing;
        processing_two.updated_at = now;
        let event_two = crate::events::refund_processing_event(&processing_two, now);
        assert!(
            !store
                .try_start_refund_execution(&processing_two, day_start, 3_000, &event_two, None)
                .await
                .unwrap()
        );

        let found_one = store
            .refund(&refund_one.store_id, refund_one.id)
            .await
            .unwrap()
            .unwrap();
        let found_two = store
            .refund(&refund_two.store_id, refund_two.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found_one.status, RefundStatus::Processing);
        assert_eq!(found_two.status, RefundStatus::Pending);
    }

    async fn test_store() -> Box<dyn Store> {
        let path = std::env::temp_dir().join(format!("qpayd-test-{}.db", Uuid::new_v4()));
        let store = SqliteStore::connect(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        store.migrate().await.unwrap();
        Box::new(store)
    }

    async fn pg_test_store() -> Option<PostgresStore> {
        let url = std::env::var("PG_URL").ok()?;
        let store = PostgresStore::connect(&url).await.unwrap();
        reset_pg_store(&store).await;
        store.migrate().await.unwrap();
        store.migrate().await.unwrap();
        Some(store)
    }

    async fn reset_pg_store(store: &PostgresStore) {
        sqlx::query(
            r#"
            DROP TABLE IF EXISTS
                qpayd_webhook_deliveries,
                qpayd_events,
                qpayd_lightning_sweeps,
                qpayd_refunds,
                qpayd_invoices,
                qpayd_store_counters,
                qpayd_schema_migrations
            "#,
        )
        .execute(&store.pool)
        .await
        .unwrap();
    }

    async fn clean_pg_store(store: &PostgresStore) {
        sqlx::query("DELETE FROM qpayd_webhook_deliveries")
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM qpayd_events")
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM qpayd_lightning_sweeps")
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM qpayd_refunds")
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM qpayd_invoices")
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM qpayd_store_counters")
            .execute(&store.pool)
            .await
            .unwrap();
    }

    fn test_invoice(status: InvoiceStatus) -> Invoice {
        let now = Utc::now();
        Invoice {
            id: Uuid::new_v4(),
            store_id: "main".to_string(),
            status,
            amount: Decimal::from(10),
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
            rate_source: "kraken".to_string(),
            rate: Decimal::from(100_000),
            metadata: serde_json::json!({ "order_id": "ord_123" }),
            expires_at: now + Duration::minutes(15),
            created_at: now,
            updated_at: now,
        }
    }

    fn test_refund_for_invoice(invoice: &Invoice, now: chrono::DateTime<Utc>) -> Refund {
        Refund {
            id: Uuid::new_v4(),
            store_id: invoice.store_id.clone(),
            invoice_id: invoice.id,
            status: RefundStatus::Pending,
            approval_status: RefundApprovalStatus::NotRequired,
            amount_sats: 2_000,
            destination: Some("bc1qrefund".to_string()),
            destination_type: Some(RefundDestinationType::BitcoinAddress),
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
