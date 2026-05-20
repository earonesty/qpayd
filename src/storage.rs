use std::str::FromStr;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use uuid::Uuid;

use crate::invoice::{Invoice, InvoiceStatus};

#[async_trait]
pub trait Store: Send + Sync {
    async fn migrate(&self) -> anyhow::Result<()>;
    async fn reserve_address_index(&self, store_id: &str) -> anyhow::Result<u32>;
    async fn insert_invoice(&self, invoice: &Invoice) -> anyhow::Result<()>;
    async fn invoice(&self, store_id: &str, id: Uuid) -> anyhow::Result<Option<Invoice>>;
}

#[derive(Debug)]
pub struct SqliteStore {
    pool: SqlitePool,
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

    async fn insert_invoice(&self, invoice: &Invoice) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO invoices (
                id, store_id, status, amount, currency, btc_amount_sats,
                onchain_address, onchain_address_index, lightning_bolt11,
                lightning_payment_hash, rate_source, rate, metadata, checkout_url, expires_at,
                created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(&invoice.lightning_bolt11)
        .bind(&invoice.lightning_payment_hash)
        .bind(&invoice.rate_source)
        .bind(invoice.rate.to_string())
        .bind(invoice.metadata.to_string())
        .bind(&invoice.checkout_url)
        .bind(invoice.expires_at.to_rfc3339())
        .bind(invoice.created_at.to_rfc3339())
        .bind(invoice.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn invoice(&self, store_id: &str, id: Uuid) -> anyhow::Result<Option<Invoice>> {
        let Some(row) = sqlx::query(
            r#"
            SELECT id, store_id, status, amount, currency, btc_amount_sats,
                   onchain_address, onchain_address_index, rate_source, rate,
                   lightning_bolt11, lightning_payment_hash, metadata, checkout_url,
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

        Ok(Some(Invoice {
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
        }))
    }
}
