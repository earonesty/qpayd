mod api;
mod config;
mod events;
mod http;
mod invoice;
mod lightning;
mod onchain;
mod payout;
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
    config::{Config, RefundExecutionConfig, StoreConfig},
    invoice::{InvoiceStatusUpdate, PaymentAmounts, Refund, RefundDestinationType, RefundStatus},
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
    Refunds,
    RefundsOnce,
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
        Command::Refunds => {
            config.validate()?;
            let store = connect_store(&config.database.url).await?;
            refund_execution_loop(config, store).await;
            Ok(())
        }
        Command::RefundsOnce => {
            config.validate()?;
            let store = connect_store(&config.database.url).await?;
            refund_execution_once(&config, store).await
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
    tokio::spawn(refund_execution_loop(config.clone(), state.store.clone()));

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
        .filter_map(|store| {
            let payout = store.effective_lightning_payout()?;
            let sweep = payout.sweep?;
            sweep.enabled.then_some(sweep.interval_seconds)
        })
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
        let Some(payout_config) = store_config.effective_lightning_payout() else {
            continue;
        };
        let Some(sweep_config) = &payout_config.sweep else {
            continue;
        };
        if !sweep_config.enabled {
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
        match lightning::sweep_to_address(&payout_config, sweep_config, destination).await? {
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

async fn refund_execution_loop(config: Config, store: Arc<dyn Store>) {
    use std::collections::HashMap;

    let tick_seconds = config
        .stores
        .iter()
        .filter_map(refund_execution_poll_seconds)
        .min();
    let Some(tick_seconds) = tick_seconds else {
        return;
    };

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(tick_seconds));
    let mut last_runs: HashMap<String, std::time::Instant> = HashMap::new();
    loop {
        interval.tick().await;
        let now = std::time::Instant::now();
        for store_config in &config.stores {
            let Some(poll_seconds) = refund_execution_poll_seconds(store_config) else {
                continue;
            };
            if let Some(last_run) = last_runs.get(&store_config.id)
                && now.duration_since(*last_run).as_secs() < poll_seconds
            {
                continue;
            }
            last_runs.insert(store_config.id.clone(), now);
            if let Err(error) = refund_execution_once_for_store(store_config, store.clone()).await {
                tracing::warn!(store_id = %store_config.id, %error, "refund execution failed");
            }
        }
    }
}

async fn refund_execution_once(config: &Config, store: Arc<dyn Store>) -> anyhow::Result<()> {
    for store_config in &config.stores {
        refund_execution_once_for_store(store_config, store.clone()).await?;
    }
    Ok(())
}

async fn refund_execution_once_for_store(
    store_config: &StoreConfig,
    store: Arc<dyn Store>,
) -> anyhow::Result<()> {
    if refund_execution_poll_seconds(store_config).is_none() {
        return Ok(());
    }
    let worker_id = format!("qpayd-{}-{}", std::process::id(), uuid::Uuid::new_v4());
    let pending = store
        .pending_refund_executions(&store_config.id, 20, chrono::Utc::now())
        .await?;
    for candidate in pending {
        let claim_now = chrono::Utc::now();
        let lease_until = claim_now + chrono::Duration::minutes(5);
        let Some(candidate) = store
            .claim_refund_execution(
                &store_config.id,
                candidate.refund.id,
                &worker_id,
                lease_until,
                claim_now,
            )
            .await?
        else {
            continue;
        };
        let invoice_id = candidate.invoice.id;
        let refund_id = candidate.refund.id;
        if let Err(error) =
            execute_claimed_refund(store.clone(), store_config, candidate.refund).await
        {
            tracing::warn!(
                store_id = %store_config.id,
                invoice_id = %invoice_id,
                refund_id = %refund_id,
                %error,
                "claimed refund execution failed"
            );
        }
    }
    Ok(())
}

async fn execute_claimed_refund(
    store: Arc<dyn Store>,
    store_config: &StoreConfig,
    mut refund: Refund,
) -> anyhow::Result<()> {
    let Some(refunds_config) = refund_execution_config(store_config, refund.destination_type)
    else {
        fail_refund_execution(
            store,
            store_config,
            refund,
            "no enabled payout backend accepted refund destination".to_string(),
        )
        .await?;
        return Ok(());
    };
    if refund.amount_sats > refunds_config.max_refund_sats {
        let reason = format!(
            "refund amount {} exceeds max_refund_sats {}",
            refund.amount_sats, refunds_config.max_refund_sats
        );
        fail_refund_execution(store, store_config, refund, reason).await?;
        return Ok(());
    }
    let day_start = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight is valid")
        .and_utc();
    let now = chrono::Utc::now();
    refund.status = RefundStatus::Processing;
    refund.failure_reason = None;
    refund.updated_at = now;
    refund.finalized_at = None;
    let event = crate::events::refund_processing_event(&refund, now);
    if !store
        .try_start_refund_execution(
            &refund,
            day_start,
            refunds_config.daily_refund_limit_sats,
            &event,
            store_config.webhook_url.as_deref(),
        )
        .await?
    {
        tracing::info!(
            store_id = %store_config.id,
            refund_id = %refund.id,
            amount_sats = refund.amount_sats,
            daily_refund_limit_sats = refunds_config.daily_refund_limit_sats,
            "refund execution waiting for daily limit"
        );
        return Ok(());
    }

    match payout::execute_refund(store_config, &refund).await {
        Ok(result) => {
            let now = chrono::Utc::now();
            refund.status = RefundStatus::Succeeded;
            refund.tx_id = result.tx_id;
            refund.payment_proof = result.payment_proof;
            refund.failure_reason = None;
            refund.updated_at = now;
            refund.finalized_at = Some(now);
            let event = crate::events::refund_finalized_event(&refund, now);
            store
                .update_refund_status(&refund, &event, store_config.webhook_url.as_deref())
                .await?;
        }
        Err(error) if is_terminal_refund_execution_error(&error) => {
            fail_refund_execution(store, store_config, refund, error.to_string()).await?;
        }
        Err(error) => {
            let now = chrono::Utc::now();
            refund.status = RefundStatus::Pending;
            refund.updated_at = now;
            refund.finalized_at = None;
            let event = crate::events::refund_retry_event(&refund, now);
            store
                .update_refund_status(&refund, &event, store_config.webhook_url.as_deref())
                .await?;
            tracing::warn!(
                store_id = %store_config.id,
                refund_id = %refund.id,
                %error,
                "refund payout backend failed; refund returned to pending for retry"
            );
        }
    }
    Ok(())
}

async fn fail_refund_execution(
    store: Arc<dyn Store>,
    store_config: &StoreConfig,
    mut refund: Refund,
    reason: String,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now();
    refund.status = RefundStatus::Failed;
    refund.failure_reason = Some(reason);
    refund.updated_at = now;
    let event = crate::events::refund_failed_event(&refund, now);
    store
        .update_refund_status(&refund, &event, store_config.webhook_url.as_deref())
        .await
}

fn is_terminal_refund_execution_error(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.contains("no payout driver accepted")
        || message.contains("has no enabled")
        || message.contains("refund has no destination")
        || message.contains("bitcoin URI has no address")
}

fn refund_execution_poll_seconds(store: &StoreConfig) -> Option<u64> {
    enabled_refund_configs(store)
        .into_iter()
        .map(|refunds| refunds.poll_seconds)
        .min()
}

fn refund_execution_config(
    store: &StoreConfig,
    destination_type: Option<RefundDestinationType>,
) -> Option<RefundExecutionConfig> {
    let configs = enabled_refund_configs_for_destination(store, destination_type);
    match destination_type {
        Some(
            RefundDestinationType::BitcoinAddress
            | RefundDestinationType::BitcoinUri
            | RefundDestinationType::LightningInvoice
            | RefundDestinationType::Lnurl,
        ) => configs.into_iter().next(),
        Some(RefundDestinationType::Unknown) | None => {
            if configs.len() == 1 {
                configs.into_iter().next()
            } else {
                None
            }
        }
    }
}

fn enabled_refund_configs(store: &StoreConfig) -> Vec<RefundExecutionConfig> {
    let mut configs = Vec::new();
    if let Some(refunds) = store
        .effective_lightning_payout()
        .and_then(|payout| payout.refunds)
        && refunds.enabled
    {
        configs.push(refunds);
    }
    if let Some(refunds) = store
        .bitcoin_payout
        .as_ref()
        .and_then(|payout| payout.refunds.clone())
        && refunds.enabled
    {
        configs.push(refunds);
    }
    configs
}

fn enabled_refund_configs_for_destination(
    store: &StoreConfig,
    destination_type: Option<RefundDestinationType>,
) -> Vec<RefundExecutionConfig> {
    let mut configs = Vec::new();
    if matches!(
        destination_type,
        Some(RefundDestinationType::LightningInvoice | RefundDestinationType::Lnurl)
            | Some(RefundDestinationType::Unknown)
            | None
    ) && let Some(refunds) = store
        .effective_lightning_payout()
        .and_then(|payout| payout.refunds)
        && refunds.enabled
    {
        configs.push(refunds);
    }
    if matches!(
        destination_type,
        Some(RefundDestinationType::BitcoinAddress | RefundDestinationType::BitcoinUri)
            | Some(RefundDestinationType::Unknown)
            | None
    ) && let Some(refunds) = store
        .bitcoin_payout
        .as_ref()
        .and_then(|payout| payout.refunds.clone())
        && refunds.enabled
    {
        configs.push(refunds);
    }
    configs
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

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use std::sync::Arc;

    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        routing::post,
    };
    use chrono::{Duration, Utc};
    use rust_decimal::Decimal;
    use uuid::Uuid;

    use super::{Config, refund_execution_once};
    use crate::{
        events::{invoice_created_event, refund_created_event},
        invoice::{
            Invoice, InvoiceStatus, Refund, RefundApprovalStatus, RefundDestinationType,
            RefundStatus,
        },
        storage::{SqliteStore, Store},
    };

    #[tokio::test]
    async fn refund_executor_finalizes_bitcoin_refund() {
        let server = test_bitcoind_server().await;
        let auth_env = format!("QPAYD_TEST_EXECUTOR_BITCOIND_{}", Uuid::new_v4().simple());
        unsafe {
            std::env::set_var(&auth_env, "user:pass");
        }
        let config = test_refund_config(&server, &auth_env, 50_000);
        let store = Arc::new(SqliteStore::connect("sqlite::memory:").await.unwrap());
        store.migrate().await.unwrap();
        let invoice = test_invoice();
        let refund = test_refund(&invoice);
        insert_invoice_and_refund(&store, &invoice, &refund).await;

        let app_store: Arc<dyn Store> = store.clone();
        refund_execution_once(&config, app_store).await.unwrap();

        let found = store
            .refund(&refund.store_id, refund.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, RefundStatus::Succeeded);
        assert_eq!(found.tx_id.as_deref(), Some("executor-refund-txid"));
    }

    #[tokio::test]
    async fn refund_executor_defers_refund_over_daily_limit() {
        let server = test_bitcoind_server().await;
        let auth_env = format!("QPAYD_TEST_EXECUTOR_BITCOIND_{}", Uuid::new_v4().simple());
        unsafe {
            std::env::set_var(&auth_env, "user:pass");
        }
        let config = test_refund_config(&server, &auth_env, 1_000);
        let store = Arc::new(SqliteStore::connect("sqlite::memory:").await.unwrap());
        store.migrate().await.unwrap();
        let invoice = test_invoice();
        let refund = test_refund(&invoice);
        insert_invoice_and_refund(&store, &invoice, &refund).await;

        let app_store: Arc<dyn Store> = store.clone();
        refund_execution_once(&config, app_store).await.unwrap();

        let found = store
            .refund(&refund.store_id, refund.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, RefundStatus::Pending);
        assert_eq!(found.tx_id, None);
    }

    #[tokio::test]
    async fn refund_executor_fails_terminal_destination_error() {
        let server = test_bitcoind_server().await;
        let auth_env = format!("QPAYD_TEST_EXECUTOR_BITCOIND_{}", Uuid::new_v4().simple());
        unsafe {
            std::env::set_var(&auth_env, "user:pass");
        }
        let config = test_refund_config(&server, &auth_env, 50_000);
        let store = Arc::new(SqliteStore::connect("sqlite::memory:").await.unwrap());
        store.migrate().await.unwrap();
        let invoice = test_invoice();
        let mut refund = test_refund(&invoice);
        refund.destination = Some("bitcoin:".to_string());
        insert_invoice_and_refund(&store, &invoice, &refund).await;

        let app_store: Arc<dyn Store> = store.clone();
        refund_execution_once(&config, app_store).await.unwrap();

        let found = store
            .refund(&refund.store_id, refund.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, RefundStatus::Failed);
        assert_eq!(found.tx_id, None);
        assert!(
            found
                .failure_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("bitcoin URI has no address"))
        );
    }

    #[tokio::test]
    async fn refund_executor_retries_backend_failure() {
        let server = test_failing_bitcoind_server().await;
        let auth_env = format!("QPAYD_TEST_EXECUTOR_BITCOIND_{}", Uuid::new_v4().simple());
        unsafe {
            std::env::set_var(&auth_env, "user:pass");
        }
        let config = test_refund_config(&server, &auth_env, 50_000);
        let store = Arc::new(SqliteStore::connect("sqlite::memory:").await.unwrap());
        store.migrate().await.unwrap();
        let invoice = test_invoice();
        let refund = test_refund(&invoice);
        insert_invoice_and_refund(&store, &invoice, &refund).await;

        let app_store: Arc<dyn Store> = store.clone();
        refund_execution_once(&config, app_store).await.unwrap();

        let found = store
            .refund(&refund.store_id, refund.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, RefundStatus::Pending);
        assert_eq!(found.tx_id, None);
    }

    async fn test_bitcoind_server() -> String {
        async fn send_to_address(
            headers: HeaderMap,
            Json(body): Json<serde_json::Value>,
        ) -> Result<Json<serde_json::Value>, StatusCode> {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Basic dXNlcjpwYXNz")
            );
            assert_eq!(body["method"], "sendtoaddress");
            assert_eq!(body["params"][0], "bc1qrefund");
            assert_eq!(body["params"][1], "0.00002");
            Ok(Json(serde_json::json!({
                "result": "executor-refund-txid",
                "error": null,
                "id": "qpayd-refund"
            })))
        }

        let app = Router::new().route("/wallet/refunds", post(send_to_address));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    async fn test_failing_bitcoind_server() -> String {
        async fn send_to_address() -> StatusCode {
            StatusCode::SERVICE_UNAVAILABLE
        }

        let app = Router::new().route("/wallet/refunds", post(send_to_address));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    fn test_refund_config(server: &str, auth_env: &str, daily_limit_sats: u64) -> Config {
        toml::from_str(&format!(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main"
            api_token_env = "QPAYD_API_TOKEN"

            [stores.bitcoin_payout]
            backend = "bitcoind"
            url = "{server}"
            wallet = "refunds"
            rpc_auth_env = "{auth_env}"

            [stores.bitcoin_payout.refunds]
            enabled = true
            max_refund_sats = 10000
            daily_refund_limit_sats = {daily_limit_sats}
            poll_seconds = 30
            "#
        ))
        .unwrap()
    }

    async fn insert_invoice_and_refund(store: &SqliteStore, invoice: &Invoice, refund: &Refund) {
        let invoice_event = invoice_created_event(invoice, invoice.created_at);
        store
            .insert_invoice(invoice, &invoice_event, None)
            .await
            .unwrap();
        let refund_event = refund_created_event(refund, refund.created_at);
        store
            .insert_refund(refund, &refund_event, None)
            .await
            .unwrap();
    }

    fn test_invoice() -> Invoice {
        let now = Utc::now();
        Invoice {
            id: Uuid::new_v4(),
            store_id: "main".to_string(),
            status: InvoiceStatus::Settled,
            amount: Decimal::from(10),
            currency: "USD".to_string(),
            btc_amount_sats: 10_000,
            paid_sats: 12_000,
            confirmed_sats: 12_000,
            unconfirmed_sats: 0,
            onchain_address: None,
            onchain_address_index: None,
            onchain_script_pubkey: None,
            lightning_bolt11: None,
            lightning_payment_hash: None,
            idempotency_key: None,
            payment_link_id: None,
            rate_source: "test".to_string(),
            rate: Decimal::from(100_000),
            metadata: serde_json::json!({}),
            expires_at: now + Duration::minutes(15),
            created_at: now,
            updated_at: now,
        }
    }

    fn test_refund(invoice: &Invoice) -> Refund {
        let now = Utc::now();
        Refund {
            id: Uuid::new_v4(),
            store_id: invoice.store_id.clone(),
            invoice_id: invoice.id,
            status: RefundStatus::Pending,
            approval_status: RefundApprovalStatus::NotRequired,
            amount_sats: 2_000,
            destination: Some("bitcoin:bc1qrefund".to_string()),
            destination_type: Some(RefundDestinationType::BitcoinUri),
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
        match webhook::deliver(
            &delivery.url,
            &secret,
            &delivery.event.id,
            &delivery.event.event_type,
            &body,
        )
        .await
        {
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
