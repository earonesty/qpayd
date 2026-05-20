mod api;
mod config;
mod invoice;
mod lightning;
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
