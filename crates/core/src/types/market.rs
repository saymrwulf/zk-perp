//! Market configuration types

use serde::{Deserialize, Serialize};
use super::{AssetId, MarketId, Price, Quantity, Timestamp, assets};

/// Market configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Market {
    /// Unique market ID
    pub id: MarketId,
    /// Human-readable name (e.g., "BTC-USDC")
    pub name: String,
    /// Base asset (e.g., BTC)
    pub base_asset: AssetId,
    /// Quote asset (e.g., USDC)
    pub quote_asset: AssetId,
    /// Minimum price increment (tick size)
    /// 8 decimals: 1 = 0.00000001
    pub tick_size: Price,
    /// Minimum quantity increment (lot size)
    /// 18 decimals: 1_000_000_000_000_000_000 = 1.0
    pub lot_size: Quantity,
    /// Minimum order size
    pub min_order_size: Quantity,
    /// Maximum leverage allowed (1-50)
    pub max_leverage: u8,
    /// Initial margin requirement in basis points (e.g., 200 = 2%)
    pub initial_margin_bps: u16,
    /// Maintenance margin requirement in basis points (e.g., 100 = 1%)
    pub maintenance_margin_bps: u16,
    /// Maker fee in basis points (can be negative for rebates)
    pub maker_fee_bps: i16,
    /// Taker fee in basis points
    pub taker_fee_bps: u16,
    /// Funding rate interval in seconds
    pub funding_interval: u64,
    /// Oracle ID for price feeds
    pub oracle_id: u32,
    /// Whether the market is active for trading
    pub is_active: bool,
    /// Timestamp when market was created
    pub created_at: Timestamp,
}

impl Market {
    /// Create a new market with default settings
    pub fn new(
        id: MarketId,
        name: String,
        base_asset: AssetId,
        quote_asset: AssetId,
    ) -> Self {
        Self {
            id,
            name,
            base_asset,
            quote_asset,
            tick_size: 1_00000000, // $1 tick
            lot_size: 1_000_000_000_000_000, // 0.001 lot
            min_order_size: 10_000_000_000_000_000, // 0.01 minimum
            max_leverage: 50,
            initial_margin_bps: 200, // 2%
            maintenance_margin_bps: 100, // 1%
            maker_fee_bps: -2, // -0.02% (rebate)
            taker_fee_bps: 5, // 0.05%
            funding_interval: 3600, // 1 hour
            oracle_id: id, // Use market ID as oracle ID by default
            is_active: true,
            created_at: 0,
        }
    }

    /// Calculate required initial margin for a position
    /// Returns margin in quote asset (USDC)
    pub fn calculate_initial_margin(&self, size: Quantity, price: Price) -> u128 {
        // margin = (size * price * initial_margin_bps) / (10000 * 10^18 * 10^8)
        // Simplified: margin = size * price * initial_margin_bps / 10^30
        let notional = (size as u128) * (price as u128);
        notional * (self.initial_margin_bps as u128) / 10_000 / 1_000_000_000_000_000_000
    }

    /// Calculate required maintenance margin for a position
    pub fn calculate_maintenance_margin(&self, size: Quantity, price: Price) -> u128 {
        let notional = (size as u128) * (price as u128);
        notional * (self.maintenance_margin_bps as u128) / 10_000 / 1_000_000_000_000_000_000
    }

    /// Calculate maker fee for a fill
    pub fn calculate_maker_fee(&self, size: Quantity, price: Price) -> i64 {
        let notional = (size as u128) * (price as u128) / 1_000_000_000_000_000_000;
        (notional as i64) * (self.maker_fee_bps as i64) / 10_000
    }

    /// Calculate taker fee for a fill
    pub fn calculate_taker_fee(&self, size: Quantity, price: Price) -> u64 {
        let notional = (size as u128) * (price as u128) / 1_000_000_000_000_000_000;
        (notional as u64) * (self.taker_fee_bps as u64) / 10_000
    }

    /// Validate order price against tick size
    pub fn validate_price(&self, price: Price) -> bool {
        price % self.tick_size == 0
    }

    /// Validate order quantity against lot size
    pub fn validate_quantity(&self, quantity: Quantity) -> bool {
        quantity >= self.min_order_size && quantity % self.lot_size == 0
    }
}

/// Oracle price data
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OraclePrice {
    /// Market this price is for
    pub market_id: MarketId,
    /// Price (8 decimals)
    pub price: Price,
    /// Timestamp of the price update
    pub timestamp: Timestamp,
    /// Confidence interval in basis points
    pub confidence_bps: u16,
}

impl OraclePrice {
    /// Create a new oracle price
    pub fn new(market_id: MarketId, price: Price, timestamp: Timestamp) -> Self {
        Self {
            market_id,
            price,
            timestamp,
            confidence_bps: 50, // 0.5% default confidence
        }
    }

    /// Check if price is stale (older than max_age_ms)
    pub fn is_stale(&self, current_time: Timestamp, max_age_ms: u64) -> bool {
        current_time.saturating_sub(self.timestamp) > max_age_ms
    }
}

/// Global system state stored in System Tree
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SystemState {
    /// Current block number
    pub block_number: u64,
    /// Current timestamp
    pub timestamp: Timestamp,
    /// Last funding timestamp
    pub last_funding_timestamp: Timestamp,
    /// Total number of accounts
    pub total_accounts: u64,
    /// Total number of orders ever created
    pub total_orders: u64,
    /// Next available order ID
    pub next_order_id: u64,
    /// Sequencer public key (for signature verification)
    pub sequencer_pubkey: [u8; 32],
}

impl SystemState {
    /// Allocate and return next order ID
    pub fn allocate_order_id(&mut self) -> u64 {
        let id = self.next_order_id;
        self.next_order_id += 1;
        self.total_orders += 1;
        id
    }

    /// Allocate and return next account ID
    pub fn allocate_account_id(&mut self) -> u64 {
        let id = self.total_accounts;
        self.total_accounts += 1;
        id
    }
}

/// Predefined markets
pub mod markets {
    use super::*;

    /// BTC-USDC perpetual market
    pub fn btc_usdc() -> Market {
        let mut market = Market::new(
            0,
            "BTC-USDC".to_string(),
            assets::BTC,
            assets::USDC,
        );
        market.tick_size = 1_00000000; // $1
        market.lot_size = 1_000_000_000_000_000; // 0.001 BTC
        market.min_order_size = 10_000_000_000_000_000; // 0.01 BTC
        market
    }

    /// ETH-USDC perpetual market
    pub fn eth_usdc() -> Market {
        let mut market = Market::new(
            1,
            "ETH-USDC".to_string(),
            assets::ETH,
            assets::USDC,
        );
        market.tick_size = 10000000; // $0.10
        market.lot_size = 10_000_000_000_000_000; // 0.01 ETH
        market.min_order_size = 100_000_000_000_000_000; // 0.1 ETH
        market
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_market_margin_calculation() {
        let market = markets::btc_usdc();

        // 1 BTC at $50,000
        let size = 1_000_000_000_000_000_000u128; // 1.0 in 18 decimals
        let price = 50000_00000000u64; // $50,000 in 8 decimals

        let initial_margin = market.calculate_initial_margin(size, price);
        // Expected: $50,000 * 2% = $1,000
        // In USDC (6 decimals for internal representation)
        assert!(initial_margin > 0);
    }

    #[test]
    fn test_price_validation() {
        let market = markets::btc_usdc();

        // Valid: multiple of tick size
        assert!(market.validate_price(50000_00000000));
        assert!(market.validate_price(50001_00000000));

        // Invalid: not multiple of tick size (tick = $1)
        assert!(!market.validate_price(50000_50000000)); // $50000.50
    }

    #[test]
    fn test_oracle_price_staleness() {
        let price = OraclePrice::new(0, 50000_00000000, 1000);

        // Not stale if current time is within threshold
        assert!(!price.is_stale(1500, 1000));

        // Stale if current time exceeds threshold
        assert!(price.is_stale(2500, 1000));
    }
}
