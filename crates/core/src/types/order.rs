//! Order types for the perpetual DEX

use serde::{Deserialize, Serialize};
use super::{AccountId, MarketId, OrderId, Price, Quantity, Timestamp};

/// Order side: buy or sell
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    /// Buy order (long)
    Bid,
    /// Sell order (short)
    Ask,
}

impl Side {
    /// Returns the opposite side
    pub fn opposite(&self) -> Self {
        match self {
            Side::Bid => Side::Ask,
            Side::Ask => Side::Bid,
        }
    }
}

/// Order type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    /// Limit order: execute at specified price or better
    Limit,
    /// Market order: execute immediately at best available price
    Market,
    /// Stop-loss: triggers market order when price reaches trigger
    StopLoss { trigger_price: Price },
    /// Take-profit: triggers market order when price reaches trigger
    TakeProfit { trigger_price: Price },
}

/// Time-in-force: how long the order remains active
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TimeInForce {
    /// Good-til-cancelled (default)
    #[default]
    GTC,
    /// Immediate-or-cancel: fill what's possible, cancel rest
    IOC,
    /// Fill-or-kill: fill entirely or cancel entirely
    FOK,
}

/// An order in the order book
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Order {
    /// Unique order ID (globally unique)
    pub id: OrderId,
    /// Account that placed this order
    pub account_id: AccountId,
    /// Market this order is for
    pub market_id: MarketId,
    /// Buy or sell
    pub side: Side,
    /// Order type (limit, market, etc.)
    pub order_type: OrderType,
    /// Limit price (0 for market orders)
    pub price: Price,
    /// Original quantity
    pub original_qty: Quantity,
    /// Remaining unfilled quantity
    pub remaining_qty: Quantity,
    /// Quantity that has been filled
    pub filled_qty: Quantity,
    /// Post-only: reject if would immediately match (maker only)
    pub post_only: bool,
    /// Reduce-only: can only reduce existing position
    pub reduce_only: bool,
    /// Time-in-force
    pub time_in_force: TimeInForce,
    /// Timestamp when order was created
    pub timestamp: Timestamp,
    /// Nonce for ordering within account (for Merkle path)
    pub nonce: u64,
}

impl Order {
    /// Create a new limit order
    pub fn new_limit(
        id: OrderId,
        account_id: AccountId,
        market_id: MarketId,
        side: Side,
        price: Price,
        quantity: Quantity,
        timestamp: Timestamp,
        nonce: u64,
    ) -> Self {
        Self {
            id,
            account_id,
            market_id,
            side,
            order_type: OrderType::Limit,
            price,
            original_qty: quantity,
            remaining_qty: quantity,
            filled_qty: 0,
            post_only: false,
            reduce_only: false,
            time_in_force: TimeInForce::GTC,
            timestamp,
            nonce,
        }
    }

    /// Create a new market order
    pub fn new_market(
        id: OrderId,
        account_id: AccountId,
        market_id: MarketId,
        side: Side,
        quantity: Quantity,
        timestamp: Timestamp,
        nonce: u64,
    ) -> Self {
        Self {
            id,
            account_id,
            market_id,
            side,
            order_type: OrderType::Market,
            price: 0,
            original_qty: quantity,
            remaining_qty: quantity,
            filled_qty: 0,
            post_only: false,
            reduce_only: false,
            time_in_force: TimeInForce::IOC,
            timestamp,
            nonce,
        }
    }

    /// Check if order is fully filled
    pub fn is_filled(&self) -> bool {
        self.remaining_qty == 0
    }

    /// Check if order is a limit order
    pub fn is_limit(&self) -> bool {
        matches!(self.order_type, OrderType::Limit)
    }

    /// Check if order is a market order
    pub fn is_market(&self) -> bool {
        matches!(self.order_type, OrderType::Market)
    }

    /// Fill a quantity from this order
    pub fn fill(&mut self, qty: Quantity) {
        debug_assert!(qty <= self.remaining_qty, "Cannot fill more than remaining");
        self.remaining_qty -= qty;
        self.filled_qty += qty;
    }

    /// Compute the path encoding for Order Book Tree
    /// Returns 9 bytes: 8 bytes for effective price + 1 byte for nonce
    pub fn orderbook_path(&self) -> [u8; 9] {
        let effective_price = match self.side {
            // Bids use inverted price so highest bids come first in tree traversal
            Side::Bid => u64::MAX - self.price,
            // Asks use direct price so lowest asks come first
            Side::Ask => self.price,
        };

        let mut path = [0u8; 9];
        path[0..8].copy_from_slice(&effective_price.to_be_bytes());
        path[8] = (self.nonce & 0xFF) as u8;
        path
    }
}

/// A fill that occurred when orders matched
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fill {
    /// The maker order that was filled against
    pub maker_order_id: OrderId,
    /// The taker order that initiated the match
    pub taker_order_id: OrderId,
    /// Price at which the fill occurred (always maker's price)
    pub price: Price,
    /// Quantity filled
    pub quantity: Quantity,
    /// Maker's account
    pub maker_account_id: AccountId,
    /// Taker's account
    pub taker_account_id: AccountId,
    /// Which side the taker was on
    pub taker_side: Side,
    /// Timestamp of the fill
    pub timestamp: Timestamp,
    /// Fee paid by maker (can be negative for rebates)
    pub maker_fee: i64,
    /// Fee paid by taker
    pub taker_fee: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_fill() {
        let mut order = Order::new_limit(
            1, 100, 1, Side::Bid, 50000_00000000, 1000000000000000000, 0, 0
        );

        assert_eq!(order.remaining_qty, 1000000000000000000);
        assert_eq!(order.filled_qty, 0);
        assert!(!order.is_filled());

        order.fill(500000000000000000);
        assert_eq!(order.remaining_qty, 500000000000000000);
        assert_eq!(order.filled_qty, 500000000000000000);
        assert!(!order.is_filled());

        order.fill(500000000000000000);
        assert!(order.is_filled());
    }

    #[test]
    fn test_orderbook_path_encoding() {
        // Bid at price 50000 should have inverted price
        let bid = Order::new_limit(1, 100, 1, Side::Bid, 50000, 1000, 0, 5);
        let bid_path = bid.orderbook_path();

        // Ask at price 50000 should have direct price
        let ask = Order::new_limit(2, 100, 1, Side::Ask, 50000, 1000, 0, 5);
        let ask_path = ask.orderbook_path();

        // Bid path should be larger (inverted) than ask path
        assert!(bid_path > ask_path);

        // Nonce should be in last byte
        assert_eq!(bid_path[8], 5);
        assert_eq!(ask_path[8], 5);
    }

    #[test]
    fn test_side_opposite() {
        assert_eq!(Side::Bid.opposite(), Side::Ask);
        assert_eq!(Side::Ask.opposite(), Side::Bid);
    }
}
