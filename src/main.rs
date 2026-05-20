mod api;
mod config;
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
        let Some(onchain_config) = &store_config.onchain else {
            continue;
        };
        let Some(server) = onchain_config.electrum_servers.first() else {
            continue;
        };
        let invoices = store.active_onchain_invoices(&store_config.id).await?;
        for invoice in invoices {
            let observation = onchain::observe(server.clone(), invoice.clone()).await?;
            if observation.next_status != invoice.status {
                let new_status = observation.next_status;
                store
                    .update_invoice_status(
                        &invoice.store_id,
                        invoice.id,
                        new_status,
                        chrono::Utc::now(),
                    )
                    .await?;
                if let (Some(url), Some(secret_env)) =
                    (&store_config.webhook_url, &store_config.webhook_secret_env)
                {
                    let mut event_invoice = invoice.clone();
                    event_invoice.status = new_status;
                    let secret = std::env::var(secret_env)?;
                    let event = webhook::Event {
                        id: format!("evt_{}", uuid::Uuid::new_v4()),
                        event_type: format!("invoice.{}", new_status.as_str()),
                        data: event_invoice,
                        created_at: chrono::Utc::now(),
                    };
                    webhook::deliver(url, &secret, &event).await?;
                }
                tracing::info!(
                    invoice_id = %invoice.id,
                    status = new_status.as_str(),
                    "invoice status updated"
                );
            }
        }
    }
    Ok(())
}
