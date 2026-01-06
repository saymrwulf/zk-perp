//! Oracle implementations for zk-perp
//!
//! This module provides price feeds for the perpetual DEX. In production,
//! this would connect to Chainlink or other decentralized oracle networks.
//! For the PoC, we provide:
//!
//! - `MockOracle`: Static prices for testing
//! - `SimulatedOracle`: Dynamic price simulation with realistic movements
//! - `PriceAggregator`: Combines multiple price sources

use std::collections::{HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};
use zk_perp_core::types::{MarketId, Price, Timestamp, OraclePrice};
use serde::{Serialize, Deserialize};
use thiserror::Error;

/// Oracle errors
#[derive(Error, Debug)]
pub enum OracleError {
    #[error("Price not available for market {0}")]
    PriceNotAvailable(MarketId),
    #[error("Price is stale: age {age_ms}ms exceeds max {max_age_ms}ms")]
    StalePrice { age_ms: u64, max_age_ms: u64 },
    #[error("Invalid price: {0}")]
    InvalidPrice(String),
}

/// Trait for price sources
pub trait PriceSource: Send + Sync {
    /// Get the current price for a market
    fn get_price(&self, market_id: MarketId) -> Result<OraclePrice, OracleError>;

    /// Get prices for all supported markets
    fn get_all_prices(&self) -> Vec<OraclePrice>;

    /// Check if this source supports a market
    fn supports_market(&self, market_id: MarketId) -> bool;
}

/// Mock oracle with static prices for testing
#[derive(Default, Clone)]
pub struct MockOracle {
    prices: HashMap<MarketId, OraclePrice>,
}

impl MockOracle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with default market prices
    pub fn with_defaults() -> Self {
        let mut oracle = Self::new();
        let now = current_timestamp();

        // BTC-USDC at $50,000
        oracle.set_price(0, 50_000_00000000, now);
        // ETH-USDC at $3,000
        oracle.set_price(1, 3_000_00000000, now);

        oracle
    }

    /// Set price for a market
    pub fn set_price(&mut self, market_id: MarketId, price: Price, timestamp: Timestamp) {
        self.prices.insert(market_id, OraclePrice::new(market_id, price, timestamp));
    }

    /// Update timestamp for all prices to current time
    pub fn refresh_timestamps(&mut self) {
        let now = current_timestamp();
        for price in self.prices.values_mut() {
            price.timestamp = now;
        }
    }
}

impl PriceSource for MockOracle {
    fn get_price(&self, market_id: MarketId) -> Result<OraclePrice, OracleError> {
        self.prices
            .get(&market_id)
            .cloned()
            .ok_or(OracleError::PriceNotAvailable(market_id))
    }

    fn get_all_prices(&self) -> Vec<OraclePrice> {
        self.prices.values().cloned().collect()
    }

    fn supports_market(&self, market_id: MarketId) -> bool {
        self.prices.contains_key(&market_id)
    }
}

/// Simulated oracle that generates realistic price movements
///
/// Uses a random walk with mean reversion for each market.
/// Useful for testing and demos.
pub struct SimulatedOracle {
    /// Current prices
    prices: HashMap<MarketId, OraclePrice>,
    /// Price history for each market (limited to last N prices)
    history: HashMap<MarketId, VecDeque<OraclePrice>>,
    /// Configuration per market
    configs: HashMap<MarketId, MarketConfig>,
    /// Maximum history length
    max_history: usize,
    /// Random state for reproducibility
    seed: u64,
}

/// Configuration for simulated price movement
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketConfig {
    /// Base/anchor price for mean reversion
    pub anchor_price: Price,
    /// Volatility (standard deviation as percentage, e.g., 2.0 = 2%)
    pub volatility_pct: f64,
    /// Mean reversion strength (0 = random walk, 1 = immediate reversion)
    pub mean_reversion: f64,
    /// Maximum deviation from anchor (as percentage)
    pub max_deviation_pct: f64,
}

impl Default for MarketConfig {
    fn default() -> Self {
        Self {
            anchor_price: 50_000_00000000, // $50,000
            volatility_pct: 0.5, // 0.5% per tick
            mean_reversion: 0.1, // 10% reversion per tick
            max_deviation_pct: 20.0, // Max 20% from anchor
        }
    }
}

impl SimulatedOracle {
    /// Create a new simulated oracle
    pub fn new() -> Self {
        Self {
            prices: HashMap::new(),
            history: HashMap::new(),
            configs: HashMap::new(),
            max_history: 1000,
            seed: 12345,
        }
    }

    /// Create with default BTC and ETH markets
    pub fn with_defaults() -> Self {
        let mut oracle = Self::new();

        // BTC-USDC (market 0)
        oracle.add_market(0, MarketConfig {
            anchor_price: 50_000_00000000, // $50,000
            volatility_pct: 0.3,
            mean_reversion: 0.05,
            max_deviation_pct: 15.0,
        });

        // ETH-USDC (market 1)
        oracle.add_market(1, MarketConfig {
            anchor_price: 3_000_00000000, // $3,000
            volatility_pct: 0.4,
            mean_reversion: 0.05,
            max_deviation_pct: 20.0,
        });

        oracle
    }

    /// Add a market with configuration
    pub fn add_market(&mut self, market_id: MarketId, config: MarketConfig) {
        let now = current_timestamp();
        let initial_price = OraclePrice::new(market_id, config.anchor_price, now);

        self.prices.insert(market_id, initial_price.clone());
        self.history.insert(market_id, VecDeque::from([initial_price]));
        self.configs.insert(market_id, config);
    }

    /// Simulate price movement for all markets
    pub fn tick(&mut self) {
        let now = current_timestamp();

        for (&market_id, config) in &self.configs.clone() {
            if let Some(current) = self.prices.get(&market_id) {
                let new_price = self.simulate_price_movement(current.price, config);
                let oracle_price = OraclePrice::new(market_id, new_price, now);

                self.prices.insert(market_id, oracle_price.clone());

                // Add to history
                if let Some(history) = self.history.get_mut(&market_id) {
                    history.push_back(oracle_price);
                    while history.len() > self.max_history {
                        history.pop_front();
                    }
                }
            }
        }

        // Advance random state
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    }

    /// Get price history for a market
    pub fn get_history(&self, market_id: MarketId, limit: usize) -> Vec<OraclePrice> {
        self.history
            .get(&market_id)
            .map(|h| h.iter().rev().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    /// Set price directly (for testing)
    pub fn set_price(&mut self, market_id: MarketId, price: Price) {
        let now = current_timestamp();
        if let Some(p) = self.prices.get_mut(&market_id) {
            p.price = price;
            p.timestamp = now;
        }
    }

    fn simulate_price_movement(&mut self, current_price: Price, config: &MarketConfig) -> Price {
        // Generate pseudo-random number between -1 and 1
        let random = ((self.seed >> 32) as f64 / u32::MAX as f64) * 2.0 - 1.0;

        // Calculate current deviation from anchor
        let current_f = current_price as f64;
        let anchor_f = config.anchor_price as f64;
        let deviation = (current_f - anchor_f) / anchor_f;

        // Mean reversion component (pulls price back toward anchor)
        let reversion = -deviation * config.mean_reversion;

        // Random walk component
        let volatility = random * config.volatility_pct / 100.0;

        // Combined price change
        let total_change = reversion + volatility;
        let new_price_f = current_f * (1.0 + total_change);

        // Clamp to max deviation
        let max_price = anchor_f * (1.0 + config.max_deviation_pct / 100.0);
        let min_price = anchor_f * (1.0 - config.max_deviation_pct / 100.0);
        let clamped_price = new_price_f.max(min_price).min(max_price);

        clamped_price as Price
    }
}

impl PriceSource for SimulatedOracle {
    fn get_price(&self, market_id: MarketId) -> Result<OraclePrice, OracleError> {
        self.prices
            .get(&market_id)
            .cloned()
            .ok_or(OracleError::PriceNotAvailable(market_id))
    }

    fn get_all_prices(&self) -> Vec<OraclePrice> {
        self.prices.values().cloned().collect()
    }

    fn supports_market(&self, market_id: MarketId) -> bool {
        self.configs.contains_key(&market_id)
    }
}

/// Price aggregator that combines multiple sources
///
/// Useful for getting median prices from multiple sources for robustness.
pub struct PriceAggregator {
    sources: Vec<Box<dyn PriceSource>>,
    /// Maximum age for a price to be considered valid (in ms)
    max_price_age_ms: u64,
}

impl PriceAggregator {
    pub fn new(max_price_age_ms: u64) -> Self {
        Self {
            sources: Vec::new(),
            max_price_age_ms,
        }
    }

    /// Add a price source
    pub fn add_source(&mut self, source: Box<dyn PriceSource>) {
        self.sources.push(source);
    }

    /// Get aggregated price (median of all sources)
    pub fn get_aggregated_price(&self, market_id: MarketId) -> Result<OraclePrice, OracleError> {
        let now = current_timestamp();
        let mut prices: Vec<Price> = Vec::new();

        for source in &self.sources {
            if let Ok(price) = source.get_price(market_id) {
                // Check staleness
                let age = now.saturating_sub(price.timestamp);
                if age <= self.max_price_age_ms {
                    prices.push(price.price);
                }
            }
        }

        if prices.is_empty() {
            return Err(OracleError::PriceNotAvailable(market_id));
        }

        // Sort and take median
        prices.sort();
        let median_price = prices[prices.len() / 2];

        Ok(OraclePrice::new(market_id, median_price, now))
    }

    /// Check if any source supports a market
    pub fn supports_market(&self, market_id: MarketId) -> bool {
        self.sources.iter().any(|s| s.supports_market(market_id))
    }
}

/// Get current Unix timestamp in milliseconds
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_oracle() {
        let mut oracle = MockOracle::new();
        let now = current_timestamp();

        oracle.set_price(0, 50_000_00000000, now);

        let price = oracle.get_price(0).expect("Should have price");
        assert_eq!(price.price, 50_000_00000000);
        assert_eq!(price.market_id, 0);
    }

    #[test]
    fn test_mock_oracle_defaults() {
        let oracle = MockOracle::with_defaults();

        let btc = oracle.get_price(0).expect("Should have BTC price");
        assert_eq!(btc.price, 50_000_00000000);

        let eth = oracle.get_price(1).expect("Should have ETH price");
        assert_eq!(eth.price, 3_000_00000000);
    }

    #[test]
    fn test_simulated_oracle() {
        let mut oracle = SimulatedOracle::with_defaults();

        let initial_btc = oracle.get_price(0).expect("Should have BTC").price;

        // Tick several times
        for _ in 0..100 {
            oracle.tick();
        }

        let final_btc = oracle.get_price(0).expect("Should have BTC").price;

        // Price should have changed but not too much (mean reversion)
        let change_pct = ((final_btc as f64 - initial_btc as f64) / initial_btc as f64).abs() * 100.0;
        assert!(change_pct < 20.0, "Price change should be bounded: {}%", change_pct);
    }

    #[test]
    fn test_price_history() {
        let mut oracle = SimulatedOracle::with_defaults();

        for _ in 0..10 {
            oracle.tick();
        }

        let history = oracle.get_history(0, 5);
        assert_eq!(history.len(), 5);
    }

    #[test]
    fn test_price_source_trait() {
        let oracle = MockOracle::with_defaults();

        // Use as trait object
        let source: &dyn PriceSource = &oracle;
        assert!(source.supports_market(0));
        assert!(source.supports_market(1));
        assert!(!source.supports_market(99));

        let prices = source.get_all_prices();
        assert_eq!(prices.len(), 2);
    }

    #[test]
    fn test_price_not_available() {
        let oracle = MockOracle::new();
        let result = oracle.get_price(99);
        assert!(matches!(result, Err(OracleError::PriceNotAvailable(99))));
    }
}
