use std::str::FromStr;

use anyhow::{Context, bail};
use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::Deserialize;

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
}

impl KrakenRateSource {
    pub fn new(config: PricingConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
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
