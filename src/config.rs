use std::{collections::HashSet, fs, path::Path};

use anyhow::{Context, bail};
use rust_decimal::Decimal;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    #[serde(default)]
    pub pricing: PricingConfig,
    #[serde(default)]
    pub stores: Vec<StoreConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default = "default_public_url")]
    pub public_url: String,
    #[serde(default = "default_onchain_poll_seconds")]
    pub onchain_poll_seconds: u64,
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
    pub api_token_env: String,
    pub webhook_url: Option<String>,
    pub webhook_secret_env: Option<String>,
    #[serde(default)]
    pub invoice_expiry_minutes: u32,
    #[serde(default)]
    pub min_confirmations: u32,
    pub onchain: Option<OnchainConfig>,
    pub lightning: Option<LightningConfig>,
    #[serde(default)]
    pub payment_links: Vec<PaymentLinkConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PaymentLinkConfig {
    pub id: String,
    pub amount: Decimal,
    pub currency: String,
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct LightningConfig {
    pub backend: LightningBackend,
    pub url: String,
    pub api_password_env: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightningBackend {
    Phoenixd,
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
            if store.webhook_url.is_some() && store.webhook_secret_env.is_none() {
                bail!("store {} webhook_url requires webhook_secret_env", store.id);
            }
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
            }
            if let Some(onchain) = &store.onchain {
                onchain.network.parse::<bitcoin::Network>()?;
                if onchain.electrum_servers.is_empty() {
                    bail!("store {} on-chain config needs electrum_servers", store.id);
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
                if matches!(lightning.backend, LightningBackend::Phoenixd)
                    && lightning.api_password_env.is_none()
                {
                    bail!("store {} phoenixd requires api_password_env", store.id);
                }
            }
        }

        Ok(())
    }

    pub fn store(&self, id: &str) -> Option<&StoreConfig> {
        self.stores.iter().find(|store| store.id == id)
    }
}

impl StoreConfig {
    pub fn api_token(&self) -> anyhow::Result<String> {
        std::env::var(&self.api_token_env)
            .with_context(|| format!("missing env var {}", self.api_token_env))
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
            public_url: default_public_url(),
            onchain_poll_seconds: default_onchain_poll_seconds(),
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

fn default_public_url() -> String {
    "http://127.0.0.1:8080".to_string()
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

#[cfg(test)]
mod tests {
    use super::Config;

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
}
