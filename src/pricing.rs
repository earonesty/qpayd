use std::{collections::HashMap, str::FromStr, sync::Arc, time::Duration};

use anyhow::{Context, bail};
use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::config::PricingConfig;

#[derive(Debug, Clone)]
pub struct Rate {
    pub source: String,
    pub value: Decimal,
}

#[async_trait]
pub trait RateSource: Send + Sync {
    async fn btc_rate(&self, quote: &str) -> anyhow::Result<Rate>;
}

#[derive(Debug, Clone)]
pub struct KrakenRateSource {
    config: PricingConfig,
    client: reqwest::Client,
    cache: Arc<RwLock<HashMap<String, CachedRate>>>,
}

#[derive(Debug, Clone)]
struct CachedRate {
    rate: Rate,
    fetched_at: std::time::Instant,
}

impl KrakenRateSource {
    pub fn new(config: PricingConfig) -> Self {
        Self {
            config,
            client: crate::http::client(),
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl RateSource for KrakenRateSource {
    async fn btc_rate(&self, quote: &str) -> anyhow::Result<Rate> {
        let quote = quote.to_uppercase();
        if quote == "BTC" || quote == "XBT" {
            return Ok(Rate {
                source: "identity".to_string(),
                value: Decimal::ONE,
            });
        }

        if let Some(rate) = self.fresh_cached_rate(&quote).await {
            return Ok(rate);
        }

        match self.fetch_btc_rate(&quote).await {
            Ok(rate) => {
                self.cache.write().await.insert(
                    quote,
                    CachedRate {
                        rate: rate.clone(),
                        fetched_at: std::time::Instant::now(),
                    },
                );
                Ok(rate)
            }
            Err(error) => {
                if let Some(rate) = self.fresh_cached_rate(&quote).await {
                    return Ok(rate);
                }
                Err(error)
            }
        }
    }
}

impl KrakenRateSource {
    async fn fresh_cached_rate(&self, quote: &str) -> Option<Rate> {
        let max_age = Duration::from_secs(self.config.stale_after_seconds);
        self.cache
            .read()
            .await
            .get(quote)
            .filter(|cached| cached.fetched_at.elapsed() <= max_age)
            .map(|cached| cached.rate.clone())
    }

    async fn fetch_btc_rate(&self, quote: &str) -> anyhow::Result<Rate> {
        let pair = format!("XBT{quote}");
        let response: KrakenTickerResponse = self
            .client
            .get(&self.config.kraken_url)
            .query(&[("pair", pair.as_str())])
            .send()
            .await
            .context("kraken ticker request failed")?
            .error_for_status()
            .context("kraken ticker returned an error")?
            .json()
            .await
            .context("failed to decode kraken ticker response")?;

        if !response.error.is_empty() {
            bail!("kraken ticker error: {}", response.error.join(", "));
        }

        let ticker = response
            .result
            .values()
            .next()
            .context("kraken ticker response had no result")?;
        let price = ticker
            .c
            .first()
            .context("kraken ticker response had no last trade price")?;
        let value = Decimal::from_str(price).context("invalid kraken decimal price")?;

        Ok(Rate {
            source: "kraken".to_string(),
            value,
        })
    }
}

#[derive(Debug, Deserialize)]
struct KrakenTickerResponse {
    error: Vec<String>,
    result: std::collections::HashMap<String, KrakenTicker>,
}

#[derive(Debug, Deserialize)]
struct KrakenTicker {
    c: Vec<String>,
}
