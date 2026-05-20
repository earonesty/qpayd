mod api;
mod config;
mod events;
mod invoice;
mod lightning;
mod onchain;
mod pricing;
mod storage;
mod webhook;

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context;
use axum::Router;
use clap::{Parser, Subcommand};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    api::AppState,
    config::Config,
    pricing::KrakenRateSource,
    storage::{SqliteStore, Store},
};

#[derive(Debug, Parser)]
#[command(name = "qpayd", about = "Bitcoin and Lightning payment daemon")]
struct Cli {
    #[arg(short, long, default_value = "qpayd.toml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve,
    Migrate,
    Check,
    SyncOnce,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "qpayd=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();
    let config = Config::load(&cli.config)?;

    match cli.command {
        Command::Serve => serve(config).await,
        Command::Migrate => {
            let store = SqliteStore::connect(&config.database.url).await?;
            store.migrate().await?;
            println!("database migrated");
            Ok(())
        }
        Command::Check => {
            config.validate()?;
            println!("configuration ok");
            Ok(())
        }
        Command::SyncOnce => {
            config.validate()?;
            let store = Arc::new(SqliteStore::connect(&config.database.url).await?);
            store.migrate().await?;
            sync_once(config, store).await
        }
    }
}

async fn serve(config: Config) -> anyhow::Result<()> {
    config.validate()?;

    let store = Arc::new(SqliteStore::connect(&config.database.url).await?);
    store.migrate().await?;

    let state = AppState {
        config: Arc::new(config.clone()),
        store,
        pricing: Arc::new(KrakenRateSource::new(config.pricing.clone())),
    };
    tokio::spawn(sync_loop(config.clone(), state.store.clone()));
    tokio::spawn(webhook_loop(config.clone(), state.store.clone()));

    let app: Router = api::router(state).layer(TraceLayer::new_for_http());
    let addr: SocketAddr = config
        .server
        .listen
        .parse()
        .with_context(|| format!("invalid listen address {}", config.server.listen))?;

    tracing::info!(%addr, "qpayd listening");
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn sync_loop(config: Config, store: Arc<dyn Store>) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(
        config.server.onchain_poll_seconds,
    ));
    loop {
        interval.tick().await;
        if let Err(error) = sync_once(config.clone(), store.clone()).await {
            tracing::warn!(%error, "on-chain sync failed");
        }
    }
}

async fn sync_once(config: Config, store: Arc<dyn Store>) -> anyhow::Result<()> {
    for store_config in &config.stores {
        if let Some(onchain_config) = &store_config.onchain
            && let Some(server) = onchain_config.electrum_servers.first()
        {
            let invoices = store.active_onchain_invoices(&store_config.id).await?;
            for invoice in invoices {
                let observation = onchain::observe(server.clone(), invoice.clone()).await?;
                update_invoice_status_event(
                    store.clone(),
                    store_config.webhook_url.as_deref(),
                    invoice,
                    observation.next_status,
                )
                .await?;
            }
        }

        if let Some(lightning_config) = &store_config.lightning {
            let invoices = store.active_lightning_invoices(&store_config.id).await?;
            for invoice in invoices {
                let next_status = lightning::observe(lightning_config, &invoice).await?;
                update_invoice_status_event(
                    store.clone(),
                    store_config.webhook_url.as_deref(),
                    invoice,
                    next_status,
                )
                .await?;
            }
        }
    }
    Ok(())
}

async fn update_invoice_status_event(
    store: Arc<dyn Store>,
    webhook_url: Option<&str>,
    invoice: invoice::Invoice,
    new_status: invoice::InvoiceStatus,
) -> anyhow::Result<()> {
    if new_status == invoice.status {
        return Ok(());
    }
    let updated_at = chrono::Utc::now();
    let event = events::invoice_status_event(&invoice, new_status, updated_at);
    store
        .update_invoice_status(
            &invoice.store_id,
            invoice.id,
            new_status,
            updated_at,
            &event,
            webhook_url,
        )
        .await?;
    tracing::info!(
        invoice_id = %invoice.id,
        status = new_status.as_str(),
        "invoice status updated"
    );
    Ok(())
}

async fn webhook_loop(config: Config, store: Arc<dyn Store>) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        interval.tick().await;
        if let Err(error) = deliver_due_webhooks(&config, store.clone()).await {
            tracing::warn!(%error, "webhook delivery failed");
        }
    }
}

async fn deliver_due_webhooks(config: &Config, store: Arc<dyn Store>) -> anyhow::Result<()> {
    let deliveries = store.due_webhook_deliveries(25, chrono::Utc::now()).await?;
    for delivery in deliveries {
        let Some(store_config) = config.store(&delivery.event.store_id) else {
            continue;
        };
        let Some(secret_env) = &store_config.webhook_secret_env else {
            continue;
        };
        let secret = std::env::var(secret_env)?;
        let body = serde_json::to_vec(&delivery.event)?;
        match webhook::deliver(&delivery.url, &secret, &body).await {
            Ok(()) => {
                store
                    .mark_webhook_delivered(delivery.id, chrono::Utc::now())
                    .await?;
            }
            Err(error) => {
                let attempts = delivery.attempts + 1;
                let delay_seconds = retry_delay_seconds(attempts);
                let now = chrono::Utc::now();
                store
                    .mark_webhook_failed(
                        delivery.id,
                        attempts,
                        now + chrono::Duration::seconds(delay_seconds),
                        &error.to_string(),
                        now,
                    )
                    .await?;
            }
        }
    }
    Ok(())
}

fn retry_delay_seconds(attempts: u32) -> i64 {
    let exponent = attempts.saturating_sub(1).min(8);
    30 * 2_i64.pow(exponent)
}
