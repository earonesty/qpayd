use std::{collections::HashSet, fs, path::Path};

use anyhow::{Context, bail};
use reqwest::Url;
use rust_decimal::Decimal;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    pub database: DatabaseConfig,
    #[serde(default)]
    pub pricing: PricingConfig,
    #[serde(default)]
    pub stores: Vec<StoreConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuthConfig {
    pub api_token_env: Option<String>,
    pub admin_token_env: Option<String>,
    #[serde(alias = "refund_token_env")]
    pub payout_token_env: Option<String>,
    #[serde(default)]
    pub admin_token_can_payout: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default = "default_onchain_poll_seconds")]
    pub onchain_poll_seconds: u64,
    #[serde(default)]
    pub public_allowed_origins: Vec<String>,
    #[serde(default)]
    pub admin: AdminPortalConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AdminPortalConfig {
    #[serde(default)]
    pub enabled: bool,
    pub store_id: Option<String>,
    pub asset_source: Option<String>,
    pub asset_integrity: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PricingConfig {
    #[serde(default = "default_kraken_url")]
    pub kraken_url: String,
    #[serde(default = "default_rate_ttl_seconds")]
    pub stale_after_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StoreConfig {
    pub id: String,
    pub name: String,
    pub api_token_env: Option<String>,
    pub admin_token_env: Option<String>,
    #[serde(alias = "refund_token_env")]
    pub payout_token_env: Option<String>,
    pub admin_token_can_payout: Option<bool>,
    #[serde(default)]
    pub public_allowed_origins: Vec<String>,
    #[serde(default)]
    pub admin_allowed_origins: Vec<String>,
    pub webhook_url: Option<String>,
    pub webhook_secret_env: Option<String>,
    #[serde(default)]
    pub invoice_expiry_minutes: u32,
    #[serde(default)]
    pub min_confirmations: u32,
    pub onchain: Option<OnchainConfig>,
    pub lightning: Option<LightningConfig>,
    pub lightning_payout: Option<LightningPayoutConfig>,
    #[serde(default)]
    pub lightning_sweep: Option<LegacyLightningSweepConfig>,
    pub bitcoin_payout: Option<BitcoinPayoutConfig>,
    #[serde(default)]
    pub payment_links: Vec<PaymentLinkConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PaymentLinkConfig {
    pub id: String,
    pub amount: Decimal,
    pub currency: String,
    #[serde(default)]
    pub public_allowed_origins: Vec<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OnchainConfig {
    #[serde(default = "default_network")]
    pub network: String,
    pub descriptor: Option<String>,
    pub descriptor_env: Option<String>,
    #[serde(default)]
    pub electrum_servers: Vec<String>,
    pub address_index_namespace: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LightningConfig {
    pub backend: LightningBackend,
    pub url: String,
    pub api_password_env: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LightningPayoutConfig {
    pub backend: LightningBackend,
    pub url: String,
    pub full_api_password_env: String,
    pub sweep: Option<LightningSweepConfig>,
    pub refunds: Option<RefundExecutionConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LightningSweepConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub destination_descriptor: Option<String>,
    pub destination_descriptor_env: Option<String>,
    #[serde(default = "default_sweep_min_balance_sats")]
    pub min_balance_sats: u64,
    #[serde(default = "default_sweep_target_balance_sats")]
    pub target_balance_sats: u64,
    #[serde(default = "default_sweep_interval_seconds")]
    pub interval_seconds: u64,
    pub feerate_sat_byte: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LegacyLightningSweepConfig {
    pub backend: LightningBackend,
    pub url: String,
    pub full_api_password_env: String,
    pub destination_descriptor: Option<String>,
    pub destination_descriptor_env: Option<String>,
    #[serde(default = "default_sweep_min_balance_sats")]
    pub min_balance_sats: u64,
    #[serde(default = "default_sweep_target_balance_sats")]
    pub target_balance_sats: u64,
    #[serde(default = "default_sweep_interval_seconds")]
    pub interval_seconds: u64,
    pub feerate_sat_byte: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BitcoinPayoutConfig {
    pub backend: BitcoinPayoutBackend,
    pub url: String,
    pub wallet: Option<String>,
    pub rpc_auth_env: String,
    pub refunds: Option<RefundExecutionConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RefundExecutionConfig {
    #[serde(default)]
    pub enabled: bool,
    pub max_refund_sats: u64,
    pub daily_refund_limit_sats: u64,
    pub manual_approval_threshold_sats: Option<u64>,
    #[serde(default = "default_refund_poll_seconds")]
    pub poll_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightningBackend {
    Phoenixd,
    Barkd,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BitcoinPayoutBackend {
    Bitcoind,
}

impl LightningBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Phoenixd => "phoenixd",
            Self::Barkd => "barkd",
        }
    }
}

impl BitcoinPayoutBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bitcoind => "bitcoind",
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let body = fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        toml::from_str(&body).with_context(|| format!("failed to parse {}", path.display()))
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.database.url.starts_with("sqlite:")
            && !self.database.url.starts_with("postgres://")
            && !self.database.url.starts_with("postgresql://")
        {
            bail!("database url must start with sqlite:, postgres://, or postgresql://");
        }
        if self.pricing.stale_after_seconds == 0 {
            bail!("pricing.stale_after_seconds must be greater than zero");
        }
        validate_public_allowed_origins(
            "server.public_allowed_origins",
            &self.server.public_allowed_origins,
        )?;
        validate_auth_config(&self.auth)?;
        validate_admin_portal(&self.server.admin, &self.stores)?;

        let mut ids = HashSet::new();
        for store in &self.stores {
            if store.id.trim().is_empty() {
                bail!("store id cannot be empty");
            }
            if store.name.trim().is_empty() {
                bail!("store {} name cannot be empty", store.id);
            }
            if !ids.insert(store.id.as_str()) {
                bail!("duplicate store id {}", store.id);
            }
            let api_token_env = store.api_token_env(&self.auth)?;
            validate_token_env(&format!("store {} api_token_env", store.id), api_token_env)?;
            if let Some(admin_token_env) = &store.admin_token_env {
                validate_token_env(
                    &format!("store {} admin_token_env", store.id),
                    admin_token_env,
                )?;
            }
            let admin_token_env = store.admin_token_env(&self.auth)?;
            if (store.admin_token_env.is_some() || self.auth.admin_token_env.is_some())
                && admin_token_env == api_token_env
            {
                bail!(
                    "store {} admin_token_env must be separate from api_token_env",
                    store.id
                );
            }
            if let Some(payout_token_env) = &store.payout_token_env {
                validate_token_env(
                    &format!("store {} payout_token_env", store.id),
                    payout_token_env,
                )?;
            }
            let payout_token_env = store.payout_token_env(&self.auth)?;
            if (store.payout_token_env.is_some() || self.auth.payout_token_env.is_some())
                && payout_token_env == api_token_env
            {
                bail!(
                    "store {} payout_token_env must be separate from api_token_env",
                    store.id
                );
            }
            if (store.payout_token_env.is_some() || self.auth.payout_token_env.is_some())
                && payout_token_env == admin_token_env
            {
                bail!(
                    "store {} payout_token_env must be separate from admin_token_env",
                    store.id
                );
            }
            if store.webhook_url.is_some() && store.webhook_secret_env.is_none() {
                bail!("store {} webhook_url requires webhook_secret_env", store.id);
            }
            validate_public_allowed_origins(
                &format!("store {} public_allowed_origins", store.id),
                &store.public_allowed_origins,
            )?;
            validate_public_allowed_origins(
                &format!("store {} admin_allowed_origins", store.id),
                &store.admin_allowed_origins,
            )?;
            if store.onchain.is_none() && store.lightning.is_none() {
                bail!("store {} has no payment methods", store.id);
            }
            let mut payment_link_ids = HashSet::new();
            for payment_link in &store.payment_links {
                if payment_link.id.trim().is_empty() {
                    bail!("store {} payment link id cannot be empty", store.id);
                }
                if payment_link.id.contains('/') {
                    bail!(
                        "store {} payment link {} cannot contain /",
                        store.id,
                        payment_link.id
                    );
                }
                if !payment_link_ids.insert(payment_link.id.as_str()) {
                    bail!(
                        "store {} has duplicate payment link id {}",
                        store.id,
                        payment_link.id
                    );
                }
                if payment_link.amount <= Decimal::ZERO {
                    bail!(
                        "store {} payment link {} amount must be positive",
                        store.id,
                        payment_link.id
                    );
                }
                if payment_link.currency.trim().is_empty() {
                    bail!(
                        "store {} payment link {} currency cannot be empty",
                        store.id,
                        payment_link.id
                    );
                }
                validate_public_allowed_origins(
                    &format!(
                        "store {} payment link {} public_allowed_origins",
                        store.id, payment_link.id
                    ),
                    &payment_link.public_allowed_origins,
                )?;
            }
            if let Some(onchain) = &store.onchain {
                onchain.network.parse::<bitcoin::Network>()?;
                if onchain.electrum_servers.is_empty() {
                    bail!("store {} on-chain config needs electrum_servers", store.id);
                }
                if let Some(namespace) = &onchain.address_index_namespace
                    && (namespace.trim().is_empty() || namespace.trim() != namespace)
                {
                    bail!(
                        "store {} onchain address_index_namespace must be non-empty and cannot have surrounding whitespace",
                        store.id
                    );
                }
                onchain
                    .descriptor()?
                    .parse::<miniscript::Descriptor<miniscript::DescriptorPublicKey>>()
                    .with_context(|| format!("invalid descriptor for store {}", store.id))?;
            }
            if let Some(lightning) = &store.lightning {
                if lightning.url.trim().is_empty() {
                    bail!("store {} lightning url cannot be empty", store.id);
                }
                if lightning.api_password_env.is_none() {
                    bail!(
                        "store {} {:?} requires api_password_env",
                        store.id,
                        lightning.backend
                    );
                }
            }
            if store.lightning_payout.is_some() && store.lightning_sweep.is_some() {
                bail!(
                    "store {} cannot use both lightning_payout and legacy lightning_sweep",
                    store.id
                );
            }
            if let Some(payout) = store.effective_lightning_payout() {
                if let Some(lightning) = &store.lightning
                    && lightning.api_password_env.as_deref()
                        == Some(payout.full_api_password_env.as_str())
                {
                    bail!(
                        "store {} lightning_payout full_api_password_env must be separate from lightning api_password_env",
                        store.id
                    );
                }
                if payout.url.trim().is_empty() {
                    bail!("store {} lightning_payout url cannot be empty", store.id);
                }
                if payout.full_api_password_env.trim().is_empty() {
                    bail!(
                        "store {} lightning_payout full_api_password_env cannot be empty",
                        store.id
                    );
                }
                if let Some(sweep) = &payout.sweep {
                    validate_lightning_sweep(&store.id, sweep)?;
                }
                if let Some(refunds) = &payout.refunds
                    && refunds.enabled
                {
                    validate_refund_execution(&store.id, "lightning_payout.refunds", refunds)?;
                }
            }
            if let Some(payout) = &store.bitcoin_payout {
                let _backend = payout.backend.as_str();
                if payout.url.trim().is_empty() {
                    bail!("store {} bitcoin_payout url cannot be empty", store.id);
                }
                if payout.rpc_auth_env.trim().is_empty() {
                    bail!(
                        "store {} bitcoin_payout rpc_auth_env cannot be empty",
                        store.id
                    );
                }
                if let Some(wallet) = &payout.wallet
                    && wallet.trim() != wallet
                {
                    bail!(
                        "store {} bitcoin_payout wallet cannot contain surrounding whitespace",
                        store.id
                    );
                }
                if let Some(refunds) = &payout.refunds
                    && refunds.enabled
                {
                    validate_refund_execution(&store.id, "bitcoin_payout.refunds", refunds)?;
                }
            }
        }

        Ok(())
    }

    pub fn store(&self, id: &str) -> Option<&StoreConfig> {
        self.stores.iter().find(|store| store.id == id)
    }
}

impl LightningSweepConfig {
    pub fn destination_descriptor(&self) -> anyhow::Result<String> {
        match (
            &self.destination_descriptor,
            &self.destination_descriptor_env,
        ) {
            (Some(_), Some(_)) => {
                bail!("use destination_descriptor or destination_descriptor_env, not both")
            }
            (Some(descriptor), None) => Ok(descriptor.clone()),
            (None, Some(env)) => {
                std::env::var(env).with_context(|| format!("missing env var {}", env))
            }
            (None, None) => {
                bail!("sweep requires destination_descriptor or destination_descriptor_env")
            }
        }
    }
}

impl LegacyLightningSweepConfig {
    fn to_lightning_payout(&self) -> LightningPayoutConfig {
        LightningPayoutConfig {
            backend: self.backend.clone(),
            url: self.url.clone(),
            full_api_password_env: self.full_api_password_env.clone(),
            sweep: Some(LightningSweepConfig {
                enabled: true,
                destination_descriptor: self.destination_descriptor.clone(),
                destination_descriptor_env: self.destination_descriptor_env.clone(),
                min_balance_sats: self.min_balance_sats,
                target_balance_sats: self.target_balance_sats,
                interval_seconds: self.interval_seconds,
                feerate_sat_byte: self.feerate_sat_byte,
            }),
            refunds: None,
        }
    }
}

impl StoreConfig {
    pub fn api_token(&self, auth: &AuthConfig) -> anyhow::Result<String> {
        token_from_env(self.api_token_env(auth)?)
    }

    pub fn admin_token(&self, auth: &AuthConfig) -> anyhow::Result<String> {
        token_from_env(self.admin_token_env(auth)?)
    }

    pub fn payout_token(&self, auth: &AuthConfig) -> anyhow::Result<String> {
        token_from_env(self.payout_token_env(auth)?)
    }

    pub fn admin_token_can_payout(&self, auth: &AuthConfig) -> bool {
        self.admin_token_can_payout
            .unwrap_or(auth.admin_token_can_payout)
    }

    pub fn api_token_env<'a>(&'a self, auth: &'a AuthConfig) -> anyhow::Result<&'a str> {
        self.api_token_env
            .as_deref()
            .or(auth.api_token_env.as_deref())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "store {} requires api_token_env or auth.api_token_env",
                    self.id
                )
            })
    }

    pub fn admin_token_env<'a>(&'a self, auth: &'a AuthConfig) -> anyhow::Result<&'a str> {
        if let Some(env) = self
            .admin_token_env
            .as_deref()
            .or(auth.admin_token_env.as_deref())
        {
            Ok(env)
        } else {
            self.api_token_env(auth)
        }
    }

    pub fn payout_token_env<'a>(&'a self, auth: &'a AuthConfig) -> anyhow::Result<&'a str> {
        if let Some(env) = self
            .payout_token_env
            .as_deref()
            .or(auth.payout_token_env.as_deref())
        {
            Ok(env)
        } else {
            self.admin_token_env(auth)
        }
    }

    pub fn effective_lightning_payout(&self) -> Option<LightningPayoutConfig> {
        self.lightning_payout.clone().or_else(|| {
            self.lightning_sweep
                .as_ref()
                .map(|sweep| sweep.to_lightning_payout())
        })
    }

    pub fn expiry_minutes(&self) -> u32 {
        if self.invoice_expiry_minutes == 0 {
            15
        } else {
            self.invoice_expiry_minutes
        }
    }

    pub fn confirmations(&self) -> u32 {
        if self.min_confirmations == 0 {
            1
        } else {
            self.min_confirmations
        }
    }

    pub fn payment_link(&self, id: &str) -> Option<&PaymentLinkConfig> {
        self.payment_links.iter().find(|link| link.id == id)
    }
}

fn token_from_env(env: &str) -> anyhow::Result<String> {
    std::env::var(env).with_context(|| format!("missing env var {}", env))
}

impl OnchainConfig {
    pub fn descriptor(&self) -> anyhow::Result<String> {
        match (&self.descriptor, &self.descriptor_env) {
            (Some(_), Some(_)) => bail!("use descriptor or descriptor_env, not both"),
            (Some(descriptor), None) => Ok(descriptor.clone()),
            (None, Some(env)) => {
                std::env::var(env).with_context(|| format!("missing env var {}", env))
            }
            (None, None) => bail!("on-chain config requires descriptor or descriptor_env"),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            onchain_poll_seconds: default_onchain_poll_seconds(),
            public_allowed_origins: Vec::new(),
            admin: AdminPortalConfig::default(),
        }
    }
}

impl Default for PricingConfig {
    fn default() -> Self {
        Self {
            kraken_url: default_kraken_url(),
            stale_after_seconds: default_rate_ttl_seconds(),
        }
    }
}

fn default_listen() -> String {
    "127.0.0.1:8080".to_string()
}

fn default_onchain_poll_seconds() -> u64 {
    30
}

fn default_kraken_url() -> String {
    "https://api.kraken.com/0/public/Ticker".to_string()
}

fn default_rate_ttl_seconds() -> u64 {
    60
}

fn default_network() -> String {
    "bitcoin".to_string()
}

fn default_sweep_min_balance_sats() -> u64 {
    100_000
}

fn default_sweep_target_balance_sats() -> u64 {
    25_000
}

fn default_sweep_interval_seconds() -> u64 {
    3600
}

fn default_true() -> bool {
    true
}

fn default_refund_poll_seconds() -> u64 {
    30
}

fn validate_auth_config(auth: &AuthConfig) -> anyhow::Result<()> {
    if let Some(api_token_env) = &auth.api_token_env {
        validate_token_env("auth.api_token_env", api_token_env)?;
    }
    if let Some(admin_token_env) = &auth.admin_token_env {
        validate_token_env("auth.admin_token_env", admin_token_env)?;
        if auth.api_token_env.as_ref() == Some(admin_token_env) {
            bail!("auth.admin_token_env must be separate from auth.api_token_env");
        }
    }
    if let Some(payout_token_env) = &auth.payout_token_env {
        validate_token_env("auth.payout_token_env", payout_token_env)?;
        if auth.api_token_env.as_ref() == Some(payout_token_env) {
            bail!("auth.payout_token_env must be separate from auth.api_token_env");
        }
        if auth.admin_token_env.as_ref() == Some(payout_token_env) {
            bail!("auth.payout_token_env must be separate from auth.admin_token_env");
        }
    }
    Ok(())
}

fn validate_lightning_sweep(store_id: &str, sweep: &LightningSweepConfig) -> anyhow::Result<()> {
    if sweep.min_balance_sats == 0 {
        bail!("store {store_id} lightning_payout.sweep min_balance_sats must be greater than zero");
    }
    if sweep.target_balance_sats >= sweep.min_balance_sats {
        bail!(
            "store {store_id} lightning_payout.sweep target_balance_sats must be less than min_balance_sats"
        );
    }
    if sweep.interval_seconds == 0 {
        bail!("store {store_id} lightning_payout.sweep interval_seconds must be greater than zero");
    }
    sweep
        .destination_descriptor()?
        .parse::<miniscript::Descriptor<miniscript::DescriptorPublicKey>>()
        .with_context(|| {
            format!("invalid lightning_payout.sweep destination descriptor for store {store_id}")
        })?;
    Ok(())
}

fn validate_refund_execution(
    store_id: &str,
    scope: &str,
    refunds: &RefundExecutionConfig,
) -> anyhow::Result<()> {
    if refunds.max_refund_sats == 0 {
        bail!("store {store_id} {scope} max_refund_sats must be greater than zero");
    }
    if refunds.daily_refund_limit_sats == 0 {
        bail!("store {store_id} {scope} daily_refund_limit_sats must be greater than zero");
    }
    if refunds.daily_refund_limit_sats < refunds.max_refund_sats {
        bail!("store {store_id} {scope} daily_refund_limit_sats must be at least max_refund_sats");
    }
    if let Some(threshold) = refunds.manual_approval_threshold_sats
        && threshold == 0
    {
        bail!("store {store_id} {scope} manual_approval_threshold_sats must be greater than zero");
    }
    if refunds.poll_seconds == 0 {
        bail!("store {store_id} {scope} poll_seconds must be greater than zero");
    }
    Ok(())
}

fn validate_token_env(scope: &str, env: &str) -> anyhow::Result<()> {
    if env.trim() != env || env.is_empty() {
        bail!("{scope} cannot be empty or contain surrounding whitespace");
    }
    Ok(())
}

fn validate_public_allowed_origins(scope: &str, origins: &[String]) -> anyhow::Result<()> {
    for origin in origins {
        let trimmed = origin.trim();
        if trimmed.is_empty() {
            bail!("{scope} cannot contain an empty origin");
        }
        if trimmed != origin {
            bail!("{scope} origin {origin:?} cannot contain surrounding whitespace");
        }
        let Some(authority) = trimmed
            .strip_prefix("https://")
            .or_else(|| trimmed.strip_prefix("http://"))
        else {
            bail!("{scope} origin {origin:?} must start with http:// or https://");
        };
        let authority = authority.trim_end_matches('/');
        if authority.is_empty()
            || authority.contains('/')
            || authority.contains('?')
            || authority.contains('#')
        {
            bail!("{scope} origin {origin:?} must be a browser origin without a path");
        }
    }
    Ok(())
}

fn validate_admin_portal(admin: &AdminPortalConfig, stores: &[StoreConfig]) -> anyhow::Result<()> {
    if !admin.enabled {
        return Ok(());
    }
    let asset_source = admin
        .asset_source
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("server.admin.asset_source is required when enabled"))?;
    if asset_source.trim() != asset_source || asset_source.is_empty() {
        bail!("server.admin.asset_source cannot be empty or contain surrounding whitespace");
    }
    if asset_source.contains("@latest") {
        bail!("server.admin.asset_source must pin an exact asset version");
    }
    if asset_source.starts_with("https://") {
        let url =
            Url::parse(asset_source).context("server.admin.asset_source is not a valid URL")?;
        if url.host_str().is_none() {
            bail!("server.admin.asset_source URL must include a host");
        }
        let integrity = admin.asset_integrity.as_deref().ok_or_else(|| {
            anyhow::anyhow!("server.admin.asset_integrity is required for remote assets")
        })?;
        validate_script_integrity(integrity)?;
    } else if asset_source.starts_with("http://") {
        bail!("server.admin.asset_source remote assets must use https");
    } else if !asset_source.starts_with('/') {
        bail!("server.admin.asset_source must be an https URL or absolute same-origin path");
    }

    if let Some(store_id) = &admin.store_id
        && !stores.iter().any(|store| store.id == *store_id)
    {
        bail!("server.admin.store_id {store_id:?} does not match a configured store");
    }
    Ok(())
}

fn validate_script_integrity(integrity: &str) -> anyhow::Result<()> {
    if integrity.trim() != integrity || integrity.is_empty() {
        bail!("server.admin.asset_integrity cannot be empty or contain surrounding whitespace");
    }
    let Some((algorithm, digest)) = integrity.split_once('-') else {
        bail!("server.admin.asset_integrity must use browser SRI format");
    };
    if !matches!(algorithm, "sha256" | "sha384" | "sha512") || digest.is_empty() {
        bail!("server.admin.asset_integrity must use sha256, sha384, or sha512");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Config, RefundExecutionConfig, validate_refund_execution};

    #[test]
    fn validates_public_payment_links() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"

            [stores.onchain]
            network = "bitcoin"
            descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            electrum_servers = ["ssl://electrum.blockstream.info:50002"]

            [[stores.payment_links]]
            id = "donate-10"
            amount = "10.00"
            currency = "USD"
            metadata = { kind = "donation" }
            "#,
        )
        .unwrap();

        config.validate().unwrap();
        let link = config
            .store("main")
            .unwrap()
            .payment_link("donate-10")
            .unwrap();
        assert_eq!(link.currency, "USD");
        assert_eq!(link.metadata["kind"], "donation");
    }

    #[test]
    fn validates_non_empty_onchain_address_index_namespace() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "primary"
            name = "Primary Store"
            api_token_env = "QPAYD_PRIMARY_API_TOKEN"

            [stores.onchain]
            network = "bitcoin"
            descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            address_index_namespace = "shared-wallet"
            electrum_servers = ["ssl://electrum.blockstream.info:50002"]
            "#,
        )
        .unwrap();

        config.validate().unwrap();
    }

    #[test]
    fn rejects_empty_onchain_address_index_namespace() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "primary"
            name = "Primary Store"
            api_token_env = "QPAYD_PRIMARY_API_TOKEN"

            [stores.onchain]
            network = "bitcoin"
            descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            address_index_namespace = ""
            electrum_servers = ["ssl://electrum.blockstream.info:50002"]
            "#,
        )
        .unwrap();

        let error = config.validate().unwrap_err().to_string();
        assert!(
            error.contains("address_index_namespace must be non-empty")
                || error.contains("onchain address_index_namespace")
        );
    }

    #[test]
    fn rejects_whitespace_surrounded_onchain_address_index_namespace() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "primary"
            name = "Primary Store"
            api_token_env = "QPAYD_PRIMARY_API_TOKEN"

            [stores.onchain]
            network = "bitcoin"
            descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            address_index_namespace = " shared-wallet "
            electrum_servers = ["ssl://electrum.blockstream.info:50002"]
            "#,
        )
        .unwrap();

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("cannot have surrounding whitespace"));
    }

    #[test]
    fn validates_admin_portal_asset_policy() {
        let config: Config = toml::from_str(
            r#"
            [server.admin]
            enabled = true
            asset_source = "https://cdn.jsdelivr.net/npm/@qpayd/admin@0.4.0/src/index.js"
            asset_integrity = "sha384-testdigest"

            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"

            [stores.onchain]
            network = "bitcoin"
            descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            electrum_servers = ["ssl://electrum.blockstream.info:50002"]
            "#,
        )
        .unwrap();

        config.validate().unwrap();
    }

    #[test]
    fn rejects_remote_admin_portal_without_integrity() {
        let config: Config = toml::from_str(
            r#"
            [server.admin]
            enabled = true
            asset_source = "https://cdn.jsdelivr.net/npm/@qpayd/admin@0.4.0/src/index.js"

            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"

            [stores.onchain]
            network = "bitcoin"
            descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            electrum_servers = ["ssl://electrum.blockstream.info:50002"]
            "#,
        )
        .unwrap();

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("asset_integrity is required"));
    }

    #[test]
    fn rejects_duplicate_public_payment_links() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"

            [stores.onchain]
            network = "bitcoin"
            descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            electrum_servers = ["ssl://electrum.blockstream.info:50002"]

            [[stores.payment_links]]
            id = "donate"
            amount = "10.00"
            currency = "USD"

            [[stores.payment_links]]
            id = "donate"
            amount = "20.00"
            currency = "USD"
            "#,
        )
        .unwrap();

        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_public_allowed_origin_paths() {
        let config: Config = toml::from_str(
            r#"
            [server]
            public_allowed_origins = ["https://example.com/pay"]

            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"

            [stores.onchain]
            network = "bitcoin"
            descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            electrum_servers = ["ssl://electrum.blockstream.info:50002"]
            "#,
        )
        .unwrap();

        assert!(config.validate().is_err());
    }

    #[test]
    fn validates_scoped_store_token_envs() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"
            admin_token_env = "QPAYD_MAIN_ADMIN_TOKEN"
            payout_token_env = "QPAYD_MAIN_PAYOUT_TOKEN"

            [stores.onchain]
            network = "bitcoin"
            descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            electrum_servers = ["ssl://electrum.blockstream.info:50002"]
            "#,
        )
        .unwrap();

        config.validate().unwrap();
    }

    #[test]
    fn validates_global_token_envs() {
        let config: Config = toml::from_str(
            r#"
            [auth]
            api_token_env = "QPAYD_API_TOKEN"
            admin_token_env = "QPAYD_ADMIN_TOKEN"
            payout_token_env = "QPAYD_PAYOUT_TOKEN"

            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"

            [stores.onchain]
            network = "bitcoin"
            descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            electrum_servers = ["ssl://electrum.blockstream.info:50002"]
            "#,
        )
        .unwrap();

        config.validate().unwrap();
    }

    #[test]
    fn rejects_shared_scoped_store_token_envs() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"
            payout_token_env = "QPAYD_MAIN_API_TOKEN"

            [stores.onchain]
            network = "bitcoin"
            descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            electrum_servers = ["ssl://electrum.blockstream.info:50002"]
            "#,
        )
        .unwrap();

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("payout_token_env must be separate"));
    }

    #[test]
    fn rejects_shared_global_token_envs() {
        let config: Config = toml::from_str(
            r#"
            [auth]
            api_token_env = "QPAYD_API_TOKEN"
            payout_token_env = "QPAYD_API_TOKEN"

            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"

            [stores.onchain]
            network = "bitcoin"
            descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            electrum_servers = ["ssl://electrum.blockstream.info:50002"]
            "#,
        )
        .unwrap();

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("auth.payout_token_env must be separate"));
    }

    #[test]
    fn rejects_shared_lightning_invoice_and_sweep_secret_env() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"

            [stores.onchain]
            network = "bitcoin"
            descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            electrum_servers = ["ssl://electrum.blockstream.info:50002"]

            [stores.lightning]
            backend = "phoenixd"
            url = "http://127.0.0.1:9740"
            api_password_env = "PHOENIXD_PASSWORD"

            [stores.lightning_payout]
            backend = "phoenixd"
            url = "http://127.0.0.1:9740"
            full_api_password_env = "PHOENIXD_PASSWORD"

            [stores.lightning_payout.sweep]
            destination_descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            "#,
        )
        .unwrap();

        assert!(config.validate().is_err());
    }

    #[test]
    fn validates_barkd_lightning_backend() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"

            [stores.lightning]
            backend = "barkd"
            url = "http://127.0.0.1:3000"
            api_password_env = "BARKD_AUTH_TOKEN"
            "#,
        )
        .unwrap();

        config.validate().unwrap();
    }

    #[test]
    fn validates_barkd_lightning_sweep_backend() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"

            [stores.lightning]
            backend = "barkd"
            url = "http://127.0.0.1:3000"
            api_password_env = "BARKD_AUTH_TOKEN"

            [stores.lightning_payout]
            backend = "barkd"
            url = "http://127.0.0.1:3000"
            full_api_password_env = "BARKD_SWEEP_AUTH_TOKEN"

            [stores.lightning_payout.sweep]
            destination_descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            "#,
        )
        .unwrap();

        config.validate().unwrap();
    }

    #[test]
    fn validates_legacy_lightning_sweep_config() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"

            [stores.lightning]
            backend = "phoenixd"
            url = "http://127.0.0.1:9740"
            api_password_env = "PHOENIXD_LIMITED_PASSWORD"

            [stores.lightning_sweep]
            backend = "phoenixd"
            url = "http://127.0.0.1:9740"
            full_api_password_env = "PHOENIXD_FULL_PASSWORD"
            destination_descriptor = "wpkh([3842548f/84'/0'/0']xpub6BemYiVNp19a1XmM4Q7cRpWqWzSvEYHbHBWbGTtDtFeZ4896wYfHzXnuRmgBSK8fEsqGiHa25de7hsoh3cRK3EonL8vd9kWUE7oVGLTshha/0/*)#flualjt8"
            "#,
        )
        .unwrap();

        config.validate().unwrap();
        assert!(config.stores[0].effective_lightning_payout().is_some());
    }

    #[test]
    fn validates_lightning_payout_refund_config() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"

            [stores.lightning]
            backend = "barkd"
            url = "http://127.0.0.1:3000"
            api_password_env = "BARKD_AUTH_TOKEN"

            [stores.lightning_payout]
            backend = "barkd"
            url = "http://127.0.0.1:3000"
            full_api_password_env = "BARKD_FULL_AUTH_TOKEN"

            [stores.lightning_payout.refunds]
            enabled = true
            max_refund_sats = 100000
            daily_refund_limit_sats = 500000
            manual_approval_threshold_sats = 250000
            "#,
        )
        .unwrap();

        config.validate().unwrap();
    }

    #[test]
    fn validates_bitcoin_and_lightning_payout_refund_backends_per_store() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"

            [stores.lightning]
            backend = "barkd"
            url = "http://127.0.0.1:3000"
            api_password_env = "BARKD_AUTH_TOKEN"

            [stores.lightning_payout]
            backend = "barkd"
            url = "http://127.0.0.1:3000"
            full_api_password_env = "BARKD_FULL_AUTH_TOKEN"

            [stores.lightning_payout.refunds]
            enabled = true
            max_refund_sats = 100000
            daily_refund_limit_sats = 500000

            [stores.bitcoin_payout]
            backend = "bitcoind"
            url = "http://127.0.0.1:8332"
            rpc_auth_env = "BITCOIND_REFUND_RPC_AUTH"

            [stores.bitcoin_payout.refunds]
            enabled = true
            max_refund_sats = 100000
            daily_refund_limit_sats = 500000
            "#,
        )
        .unwrap();

        config.validate().unwrap();
    }

    #[test]
    fn rejects_payout_refund_limits_below_single_refund() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"

            [stores.lightning]
            backend = "barkd"
            url = "http://127.0.0.1:3000"
            api_password_env = "BARKD_AUTH_TOKEN"

            [stores.lightning_payout]
            backend = "barkd"
            url = "http://127.0.0.1:3000"
            full_api_password_env = "BARKD_FULL_AUTH_TOKEN"

            [stores.lightning_payout.refunds]
            enabled = true
            max_refund_sats = 100000
            daily_refund_limit_sats = 50000
            "#,
        )
        .unwrap();

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("daily_refund_limit_sats"));
    }

    #[test]
    fn rejects_zero_refund_max_refund_sats() {
        let mut refunds = valid_refund_execution_config();
        refunds.max_refund_sats = 0;

        let error = validate_refund_execution("main", "lightning_payout.refunds", &refunds)
            .unwrap_err()
            .to_string();
        assert!(error.contains("max_refund_sats"));
    }

    #[test]
    fn rejects_zero_refund_daily_refund_limit_sats() {
        let mut refunds = valid_refund_execution_config();
        refunds.daily_refund_limit_sats = 0;

        let error = validate_refund_execution("main", "lightning_payout.refunds", &refunds)
            .unwrap_err()
            .to_string();
        assert!(error.contains("daily_refund_limit_sats"));
    }

    #[test]
    fn rejects_zero_refund_manual_approval_threshold_sats() {
        let mut refunds = valid_refund_execution_config();
        refunds.manual_approval_threshold_sats = Some(0);

        let error = validate_refund_execution("main", "lightning_payout.refunds", &refunds)
            .unwrap_err()
            .to_string();
        assert!(error.contains("manual_approval_threshold_sats"));
    }

    #[test]
    fn rejects_zero_refund_poll_seconds() {
        let mut refunds = valid_refund_execution_config();
        refunds.poll_seconds = 0;

        let error = validate_refund_execution("main", "lightning_payout.refunds", &refunds)
            .unwrap_err()
            .to_string();
        assert!(error.contains("poll_seconds"));
    }

    #[test]
    fn rejects_shared_lightning_invoice_and_payout_secret_env() {
        let config: Config = toml::from_str(
            r#"
            [database]
            url = "sqlite::memory:"

            [[stores]]
            id = "main"
            name = "Main Store"
            api_token_env = "QPAYD_MAIN_API_TOKEN"

            [stores.lightning]
            backend = "barkd"
            url = "http://127.0.0.1:3000"
            api_password_env = "BARKD_AUTH_TOKEN"

            [stores.lightning_payout]
            backend = "barkd"
            url = "http://127.0.0.1:3000"
            full_api_password_env = "BARKD_AUTH_TOKEN"

            [stores.lightning_payout.refunds]
            enabled = true
            max_refund_sats = 100000
            daily_refund_limit_sats = 500000
            "#,
        )
        .unwrap();

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("full_api_password_env must be separate"));
    }

    fn valid_refund_execution_config() -> RefundExecutionConfig {
        RefundExecutionConfig {
            enabled: true,
            max_refund_sats: 100_000,
            daily_refund_limit_sats: 500_000,
            manual_approval_threshold_sats: Some(250_000),
            poll_seconds: 30,
        }
    }
}
