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
use bitcoin::secp256k1::Secp256k1;
use clap::{Parser, Subcommand};
use miniscript::{Descriptor, DescriptorPublicKey};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    api::AppState,
    config::Config,
    invoice::{InvoiceStatusUpdate, PaymentAmounts},
    pricing::KrakenRateSource,
    storage::{Store, connect_store},
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
    Sweep,
    Migrate,
    Check,
    SyncOnce,
    SweepOnce,
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
        Command::Sweep => {
            config.validate()?;
            lightning_sweep_loop(config).await;
            Ok(())
        }
        Command::Migrate => {
            config.validate()?;
            let store = connect_store(&config.database.url).await?;
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
            let store = connect_store(&config.database.url).await?;
            sync_once(config, store).await
        }
        Command::SweepOnce => {
            config.validate()?;
            let mut last_runs = std::collections::HashMap::new();
            lightning_sweep_once(&config, &mut last_runs).await
        }
    }
}

async fn serve(config: Config) -> anyhow::Result<()> {
    config.validate()?;

    let store = connect_store(&config.database.url).await?;
    if migrate_on_boot() {
        store.migrate().await?;
    }

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

fn migrate_on_boot() -> bool {
    std::env::var("QPAYD_MIGRATE_ON_BOOT")
        .map(|value| {
            matches!(
                value.as_str(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(false)
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
        if let Some(onchain_config) = &store_config.onchain {
            let invoices = store.active_onchain_invoices(&store_config.id).await?;
            for invoice in invoices {
                let observation =
                    onchain::observe(onchain_config.electrum_servers.clone(), invoice.clone())
                        .await?;
                tracing::debug!(
                    invoice_id = %invoice.id,
                    electrum_server = %observation.server,
                    "observed on-chain invoice"
                );
                update_invoice_status_event(
                    store.clone(),
                    store_config.webhook_url.as_deref(),
                    invoice,
                    observation.next_status,
                    PaymentAmounts {
                        paid_sats: observation.confirmed_sats + observation.unconfirmed_sats,
                        confirmed_sats: observation.confirmed_sats,
                        unconfirmed_sats: observation.unconfirmed_sats,
                    },
                )
                .await?;
            }
        }

        if let Some(lightning_config) = &store_config.lightning {
            let invoices = store.active_lightning_invoices(&store_config.id).await?;
            for invoice in invoices {
                let observation = lightning::observe(lightning_config, &invoice).await?;
                update_invoice_status_event(
                    store.clone(),
                    store_config.webhook_url.as_deref(),
                    invoice,
                    observation.next_status,
                    PaymentAmounts {
                        paid_sats: observation.received_sats,
                        confirmed_sats: observation.received_sats,
                        unconfirmed_sats: 0,
                    },
                )
                .await?;
            }
        }

        let invoices = store
            .expirable_invoices(&store_config.id, chrono::Utc::now())
            .await?;
        for invoice in invoices {
            let payment = PaymentAmounts::from_invoice(&invoice);
            update_invoice_status_event(
                store.clone(),
                store_config.webhook_url.as_deref(),
                invoice,
                invoice::InvoiceStatus::Expired,
                payment,
            )
            .await?;
        }
    }
    Ok(())
}

async fn lightning_sweep_loop(config: Config) {
    let tick_seconds = config
        .stores
        .iter()
        .filter_map(|store| store.lightning_sweep.as_ref())
        .map(|sweep| sweep.interval_seconds)
        .min();
    let Some(tick_seconds) = tick_seconds else {
        return;
    };

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(tick_seconds));
    let mut last_runs = std::collections::HashMap::new();
    loop {
        interval.tick().await;
        if let Err(error) = lightning_sweep_once(&config, &mut last_runs).await {
            tracing::warn!(%error, "lightning sweep failed");
        }
    }
}

async fn lightning_sweep_once(
    config: &Config,
    last_runs: &mut std::collections::HashMap<String, std::time::Instant>,
) -> anyhow::Result<()> {
    for store_config in &config.stores {
        let Some(sweep_config) = &store_config.lightning_sweep else {
            continue;
        };
        let now = std::time::Instant::now();
        if let Some(last_run) = last_runs.get(&store_config.id)
            && now.duration_since(*last_run).as_secs() < sweep_config.interval_seconds
        {
            continue;
        }
        last_runs.insert(store_config.id.clone(), now);
        let network = store_config
            .onchain
            .as_ref()
            .map(|onchain| onchain.network.as_str())
            .unwrap_or("bitcoin")
            .parse::<bitcoin::Network>()
            .with_context(|| format!("invalid bitcoin network for store {}", store_config.id))?;
        let destination = derive_lightning_sweep_address(sweep_config, network)?;
        match lightning::sweep_to_address(sweep_config, destination).await? {
            Some(result) => tracing::info!(
                store_id = %store_config.id,
                balance_sats = result.balance_sats,
                amount_sats = result.amount_sats,
                address = %result.address,
                tx_id = result.tx_id.as_deref().unwrap_or(""),
                "lightning balance swept"
            ),
            None => tracing::debug!(store_id = %store_config.id, "lightning sweep skipped"),
        }
    }
    Ok(())
}

fn derive_lightning_sweep_address(
    config: &crate::config::LightningSweepConfig,
    network: bitcoin::Network,
) -> anyhow::Result<String> {
    let descriptor = config
        .destination_descriptor()?
        .parse::<Descriptor<DescriptorPublicKey>>()
        .context("invalid lightning sweep destination descriptor")?;
    let secp = Secp256k1::verification_only();
    let derived = descriptor
        .derived_descriptor(&secp, 0)
        .context("failed to derive lightning sweep destination descriptor")?;
    let address = derived
        .address(network)
        .context("lightning sweep destination descriptor does not produce an address")?;
    Ok(address.to_string())
}

async fn update_invoice_status_event(
    store: Arc<dyn Store>,
    webhook_url: Option<&str>,
    mut invoice: invoice::Invoice,
    new_status: invoice::InvoiceStatus,
    payment: PaymentAmounts,
) -> anyhow::Result<()> {
    let payment_changed = PaymentAmounts::from_invoice(&invoice) != payment;
    if new_status == invoice.status {
        if payment_changed {
            store
                .update_invoice_payment_amounts(
                    &invoice.store_id,
                    invoice.id,
                    payment,
                    chrono::Utc::now(),
                )
                .await?;
        }
        return Ok(());
    }
    let updated_at = chrono::Utc::now();
    invoice.paid_sats = payment.paid_sats;
    invoice.confirmed_sats = payment.confirmed_sats;
    invoice.unconfirmed_sats = payment.unconfirmed_sats;
    let event = events::invoice_status_event(&invoice, new_status, updated_at);
    store
        .update_invoice_status(
            &invoice.store_id,
            invoice.id,
            InvoiceStatusUpdate {
                status: new_status,
                payment,
                updated_at,
            },
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
