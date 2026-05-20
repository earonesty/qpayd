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
    invoice::{Invoice, InvoiceStatus},
};

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
    async fn active_onchain_invoices(&self, store_id: &str) -> anyhow::Result<Vec<Invoice>>;
    async fn active_lightning_invoices(&self, store_id: &str) -> anyhow::Result<Vec<Invoice>>;
    async fn update_invoice_status(
        &self,
        store_id: &str,
        id: Uuid,
        status: InvoiceStatus,
        updated_at: DateTime<Utc>,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
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
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS store_counters (
                store_id TEXT PRIMARY KEY NOT NULL,
                next_onchain_index INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
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
                checkout_url TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS invoices_store_created_idx
            ON invoices (store_id, created_at)
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
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
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS events_store_created_idx
            ON events (store_id, created_at)
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
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
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS webhook_deliveries_due_idx
            ON webhook_deliveries (status, next_attempt_at)
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
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
                onchain_address, onchain_address_index, onchain_script_pubkey,
                lightning_bolt11, lightning_payment_hash, rate_source, rate,
                metadata, checkout_url, expires_at, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(invoice.id.to_string())
        .bind(&invoice.store_id)
        .bind(invoice.status.as_str())
        .bind(invoice.amount.to_string())
        .bind(&invoice.currency)
        .bind(invoice.btc_amount_sats as i64)
        .bind(&invoice.onchain_address)
        .bind(invoice.onchain_address_index.map(|index| index as i64))
        .bind(&invoice.onchain_script_pubkey)
        .bind(&invoice.lightning_bolt11)
        .bind(&invoice.lightning_payment_hash)
        .bind(&invoice.rate_source)
        .bind(invoice.rate.to_string())
        .bind(invoice.metadata.to_string())
        .bind(&invoice.checkout_url)
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
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, metadata, checkout_url,
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

    async fn active_onchain_invoices(&self, store_id: &str) -> anyhow::Result<Vec<Invoice>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, status, amount, currency, btc_amount_sats,
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, metadata, checkout_url,
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
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, metadata, checkout_url,
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

    async fn update_invoice_status(
        &self,
        store_id: &str,
        id: Uuid,
        status: InvoiceStatus,
        updated_at: DateTime<Utc>,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            UPDATE invoices
            SET status = ?, updated_at = ?
            WHERE store_id = ? AND id = ?
            "#,
        )
        .bind(status.as_str())
        .bind(updated_at.to_rfc3339())
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
}

#[async_trait]
impl Store for PostgresStore {
    async fn migrate(&self) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS qpayd_store_counters (
                store_id TEXT PRIMARY KEY NOT NULL,
                next_onchain_index BIGINT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
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
                checkout_url TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS qpayd_invoices_store_created_idx
            ON qpayd_invoices (store_id, created_at)
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
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
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS qpayd_events_store_created_idx
            ON qpayd_events (store_id, created_at)
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
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
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS qpayd_webhook_deliveries_due_idx
            ON qpayd_webhook_deliveries (status, next_attempt_at)
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
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
                onchain_address, onchain_address_index, onchain_script_pubkey,
                lightning_bolt11, lightning_payment_hash, rate_source, rate,
                metadata, checkout_url, expires_at, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
            "#,
        )
        .bind(invoice.id.to_string())
        .bind(&invoice.store_id)
        .bind(invoice.status.as_str())
        .bind(invoice.amount.to_string())
        .bind(&invoice.currency)
        .bind(invoice.btc_amount_sats as i64)
        .bind(&invoice.onchain_address)
        .bind(invoice.onchain_address_index.map(|index| index as i64))
        .bind(&invoice.onchain_script_pubkey)
        .bind(&invoice.lightning_bolt11)
        .bind(&invoice.lightning_payment_hash)
        .bind(&invoice.rate_source)
        .bind(invoice.rate.to_string())
        .bind(invoice.metadata.to_string())
        .bind(&invoice.checkout_url)
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
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, metadata, checkout_url,
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

    async fn active_onchain_invoices(&self, store_id: &str) -> anyhow::Result<Vec<Invoice>> {
        let rows = sqlx::query(
            r#"
            SELECT id, store_id, status, amount, currency, btc_amount_sats,
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, metadata, checkout_url,
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
                   onchain_address, onchain_address_index, onchain_script_pubkey,
                   rate_source, rate, lightning_bolt11, lightning_payment_hash, metadata, checkout_url,
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

    async fn update_invoice_status(
        &self,
        store_id: &str,
        id: Uuid,
        status: InvoiceStatus,
        updated_at: DateTime<Utc>,
        event: &EventEnvelope,
        webhook_url: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            UPDATE qpayd_invoices
            SET status = $1, updated_at = $2
            WHERE store_id = $3 AND id = $4
            "#,
        )
        .bind(status.as_str())
        .bind(updated_at.to_rfc3339())
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
        onchain_address: row.get("onchain_address"),
        onchain_address_index: row
            .get::<Option<i64>, _>("onchain_address_index")
            .map(|index| index as u32),
        onchain_script_pubkey: row.get("onchain_script_pubkey"),
        lightning_bolt11: row.get("lightning_bolt11"),
        lightning_payment_hash: row.get("lightning_payment_hash"),
        rate_source: row.get("rate_source"),
        rate: row.get::<String, _>("rate").parse::<Decimal>()?,
        metadata: serde_json::from_str(row.get::<String, _>("metadata").as_str())?,
        checkout_url: row.get("checkout_url"),
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
        onchain_address: row.get("onchain_address"),
        onchain_address_index: row
            .get::<Option<i64>, _>("onchain_address_index")
            .map(|index| index as u32),
        onchain_script_pubkey: row.get("onchain_script_pubkey"),
        lightning_bolt11: row.get("lightning_bolt11"),
        lightning_payment_hash: row.get("lightning_payment_hash"),
        rate_source: row.get("rate_source"),
        rate: row.get::<String, _>("rate").parse::<Decimal>()?,
        metadata: serde_json::from_str(row.get::<String, _>("metadata").as_str())?,
        checkout_url: row.get("checkout_url"),
        expires_at: DateTime::parse_from_rfc3339(row.get::<String, _>("expires_at").as_str())?
            .with_timezone(&Utc),
        created_at: DateTime::parse_from_rfc3339(row.get::<String, _>("created_at").as_str())?
            .with_timezone(&Utc),
        updated_at: DateTime::parse_from_rfc3339(row.get::<String, _>("updated_at").as_str())?
            .with_timezone(&Utc),
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
    use uuid::Uuid;

    use super::{PostgresStore, SqliteStore, Store};
    use crate::{
        events::{invoice_created_event, invoice_status_event},
        invoice::{Invoice, InvoiceStatus},
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
    async fn postgres_storage_contract() {
        let Some(store) = pg_test_store().await else {
            return;
        };

        clean_pg_store(&store).await;
        insert_invoice_persists_event_and_webhook_delivery_for(&store).await;

        clean_pg_store(&store).await;
        replay_adds_a_fresh_delivery_for_existing_event_for(&store).await;

        clean_pg_store(&store).await;
        duplicate_status_event_does_not_enqueue_twice_for(&store).await;
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
                InvoiceStatus::Settled,
                updated_at,
                &settled,
                Some("https://example.com/webhook"),
            )
            .await
            .unwrap();
        store
            .update_invoice_status(
                &invoice.store_id,
                invoice.id,
                InvoiceStatus::Settled,
                updated_at,
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
        store.migrate().await.unwrap();
        Some(store)
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
            onchain_address: Some("bc1qexample".to_string()),
            onchain_address_index: Some(0),
            onchain_script_pubkey: Some("0014".to_string()),
            lightning_bolt11: None,
            lightning_payment_hash: None,
            rate_source: "kraken".to_string(),
            rate: Decimal::from(100_000),
            metadata: serde_json::json!({ "order_id": "ord_123" }),
            checkout_url: "https://pay.example.com/i/main/test".to_string(),
            expires_at: now + Duration::minutes(15),
            created_at: now,
            updated_at: now,
        }
    }
}
