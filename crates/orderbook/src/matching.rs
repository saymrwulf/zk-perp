//! Matching engine with price-time priority

use zk_perp_core::types::{Order, OrderId, Price, Quantity, Side, AccountId, Fill, Timestamp, OrderType};
use super::book::OrderBook;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MatchingError {
    #[error("Order not found: {0}")]
    OrderNotFound(OrderId),
    #[error("Invalid order: {0}")]
    InvalidOrder(String),
    #[error("Post-only order would take liquidity")]
    PostOnlyWouldTake,
    #[error("No liquidity available")]
    NoLiquidity,
}

/// Result of matching an order
#[derive(Clone, Debug)]
pub struct MatchResult {
    /// Fills that occurred
    pub fills: Vec<Fill>,
    /// Remaining order (if any, for limit orders that didn't fully fill)
    pub remaining_order: Option<Order>,
    /// Total quantity filled
    pub filled_quantity: Quantity,
    /// Average fill price (weighted by quantity)
    pub average_price: Option<Price>,
}

impl MatchResult {
    /// Create an empty match result
    pub fn empty() -> Self {
        Self {
            fills: Vec::new(),
            remaining_order: None,
            filled_quantity: 0,
            average_price: None,
        }
    }

    /// Check if any fills occurred
    pub fn has_fills(&self) -> bool {
        !self.fills.is_empty()
    }

    /// Calculate average price from fills
    fn calculate_average_price(fills: &[Fill]) -> Option<Price> {
        if fills.is_empty() {
            return None;
        }

        let total_value: u128 = fills.iter()
            .map(|f| (f.price as u128) * (f.quantity as u128))
            .sum();
        let total_qty: u128 = fills.iter()
            .map(|f| f.quantity as u128)
            .sum();

        if total_qty == 0 {
            None
        } else {
            Some((total_value / total_qty) as Price)
        }
    }
}

/// Matching engine
pub struct MatchingEngine {
    /// Current timestamp for fills
    timestamp: Timestamp,
    /// Maker fee in basis points (can be negative)
    maker_fee_bps: i16,
    /// Taker fee in basis points
    taker_fee_bps: u16,
}

impl Default for MatchingEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MatchingEngine {
    /// Create a new matching engine
    pub fn new() -> Self {
        Self {
            timestamp: 0,
            maker_fee_bps: -2, // -0.02% rebate
            taker_fee_bps: 5,  // 0.05%
        }
    }

    /// Create with custom fees
    pub fn with_fees(maker_fee_bps: i16, taker_fee_bps: u16) -> Self {
        Self {
            timestamp: 0,
            maker_fee_bps,
            taker_fee_bps,
        }
    }

    /// Set the current timestamp
    pub fn set_timestamp(&mut self, timestamp: Timestamp) {
        self.timestamp = timestamp;
    }

    /// Match an incoming order against the book
    ///
    /// This implements price-time priority:
    /// - For buys: match against lowest asks first
    /// - For sells: match against highest bids first
    /// - Within a price level: match against oldest orders first (FIFO)
    pub fn match_order(
        &self,
        book: &mut OrderBook,
        mut incoming: Order,
    ) -> Result<MatchResult, MatchingError> {
        // Validate order
        if incoming.remaining_qty == 0 {
            return Err(MatchingError::InvalidOrder("Zero quantity".to_string()));
        }

        let mut fills = Vec::new();
        let mut total_filled: Quantity = 0;

        // Get the opposite side of the book
        let opposite_side = incoming.side.opposite();

        // Match until order is filled or no more liquidity
        while incoming.remaining_qty > 0 {
            // Get best price on opposite side
            let best_price = match incoming.side {
                Side::Bid => book.best_ask(),  // Buyer matches against asks
                Side::Ask => book.best_bid(),  // Seller matches against bids
            };

            let best_price = match best_price {
                Some(p) => p,
                None => break, // No liquidity
            };

            // Check if prices cross (can trade)
            let can_trade = match incoming.side {
                Side::Bid => {
                    // Buyer willing to pay at least the ask price
                    incoming.order_type == OrderType::Market || incoming.price >= best_price
                }
                Side::Ask => {
                    // Seller willing to accept at most the bid price
                    incoming.order_type == OrderType::Market || incoming.price <= best_price
                }
            };

            if !can_trade {
                break;
            }

            // Post-only check: reject if would immediately match
            if incoming.post_only && fills.is_empty() {
                return Err(MatchingError::PostOnlyWouldTake);
            }

            // Get the price level
            let opposite_book = match opposite_side {
                Side::Bid => &mut book.bids,
                Side::Ask => &mut book.asks,
            };

            let level = match opposite_book.get_mut(&best_price) {
                Some(l) => l,
                None => break,
            };

            // Match against orders at this level (FIFO - time priority)
            while !level.is_empty() && incoming.remaining_qty > 0 {
                let maker = level.front_mut().unwrap();

                // Calculate fill quantity
                let fill_qty = incoming.remaining_qty.min(maker.remaining_qty);

                // Calculate fees
                let notional = (fill_qty as u128) * (best_price as u128) / 1_000_000_000_000_000_000;
                let maker_fee = (notional as i64) * (self.maker_fee_bps as i64) / 10_000;
                let taker_fee = (notional as u64) * (self.taker_fee_bps as u64) / 10_000;

                // Create fill
                let fill = Fill {
                    maker_order_id: maker.id,
                    taker_order_id: incoming.id,
                    price: best_price, // Always execute at maker's price
                    quantity: fill_qty,
                    maker_account_id: maker.account_id,
                    taker_account_id: incoming.account_id,
                    taker_side: incoming.side,
                    timestamp: self.timestamp,
                    maker_fee,
                    taker_fee,
                };

                fills.push(fill);
                total_filled += fill_qty;

                // Update quantities
                maker.fill(fill_qty);
                incoming.fill(fill_qty);

                // Remove fully filled maker order
                if maker.is_filled() {
                    let filled_maker = level.pop_front().unwrap();
                    book.order_locations.remove(&filled_maker.id);
                }
            }

            // Remove empty price level
            if level.is_empty() {
                opposite_book.remove(&best_price);
            }
        }

        // Handle remaining order
        let remaining_order = if incoming.remaining_qty > 0 {
            match incoming.order_type {
                OrderType::Limit => {
                    // Add remaining to book
                    book.add_order(incoming.clone());
                    Some(incoming)
                }
                OrderType::Market => {
                    // Market orders don't rest in book
                    None
                }
                _ => None, // Stop/TP orders handled separately
            }
        } else {
            None
        };

        let average_price = MatchResult::calculate_average_price(&fills);

        Ok(MatchResult {
            fills,
            remaining_order,
            filled_quantity: total_filled,
            average_price,
        })
    }

    /// Process a cancel order
    pub fn cancel_order(
        &self,
        book: &mut OrderBook,
        order_id: OrderId,
        account_id: AccountId,
    ) -> Result<Order, MatchingError> {
        // Find and verify ownership
        let order = book.get_order(order_id)
            .ok_or(MatchingError::OrderNotFound(order_id))?;

        if order.account_id != account_id {
            return Err(MatchingError::InvalidOrder(
                "Order does not belong to account".to_string()
            ));
        }

        // Cancel the order
        book.cancel_order(order_id)
            .ok_or(MatchingError::OrderNotFound(order_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zk_perp_core::types::TimeInForce;

    fn make_order(id: OrderId, account_id: AccountId, side: Side, price: Price, qty: Quantity) -> Order {
        Order {
            id,
            account_id,
            market_id: 0,
            side,
            order_type: OrderType::Limit,
            price,
            original_qty: qty,
            remaining_qty: qty,
            filled_qty: 0,
            post_only: false,
            reduce_only: false,
            time_in_force: TimeInForce::GTC,
            timestamp: 0,
            nonce: 0,
        }
    }

    fn make_market_order(id: OrderId, account_id: AccountId, side: Side, qty: Quantity) -> Order {
        Order {
            id,
            account_id,
            market_id: 0,
            side,
            order_type: OrderType::Market,
            price: 0,
            original_qty: qty,
            remaining_qty: qty,
            filled_qty: 0,
            post_only: false,
            reduce_only: false,
            time_in_force: TimeInForce::IOC,
            timestamp: 0,
            nonce: 0,
        }
    }

    #[test]
    fn test_no_match_empty_book() {
        let mut book = OrderBook::new(0);
        let engine = MatchingEngine::new();

        let order = make_order(1, 1, Side::Bid, 100, 10);
        let result = engine.match_order(&mut book, order).unwrap();

        assert!(!result.has_fills());
        assert!(result.remaining_order.is_some());
        assert_eq!(book.total_orders(), 1);
    }

    #[test]
    fn test_full_match() {
        let mut book = OrderBook::new(0);
        let engine = MatchingEngine::new();

        // Add a sell order
        book.add_order(make_order(1, 1, Side::Ask, 100, 10));

        // Buy order that matches
        let buy = make_order(2, 2, Side::Bid, 100, 10);
        let result = engine.match_order(&mut book, buy).unwrap();

        assert_eq!(result.fills.len(), 1);
        assert_eq!(result.fills[0].quantity, 10);
        assert_eq!(result.fills[0].price, 100);
        assert_eq!(result.filled_quantity, 10);
        assert!(result.remaining_order.is_none());
        assert!(book.is_empty());
    }

    #[test]
    fn test_partial_match() {
        let mut book = OrderBook::new(0);
        let engine = MatchingEngine::new();

        // Add a small sell order
        book.add_order(make_order(1, 1, Side::Ask, 100, 5));

        // Larger buy order
        let buy = make_order(2, 2, Side::Bid, 100, 10);
        let result = engine.match_order(&mut book, buy).unwrap();

        assert_eq!(result.fills.len(), 1);
        assert_eq!(result.fills[0].quantity, 5);
        assert_eq!(result.filled_quantity, 5);

        // Remaining order should be in book
        let remaining = result.remaining_order.unwrap();
        assert_eq!(remaining.remaining_qty, 5);
        assert_eq!(book.total_orders(), 1);
    }

    #[test]
    fn test_price_priority() {
        let mut book = OrderBook::new(0);
        let engine = MatchingEngine::new();

        // Add asks at different prices
        book.add_order(make_order(1, 1, Side::Ask, 102, 10)); // Worse price
        book.add_order(make_order(2, 2, Side::Ask, 100, 10)); // Best price

        // Buy order should match best price first
        let buy = make_order(3, 3, Side::Bid, 105, 15);
        let result = engine.match_order(&mut book, buy).unwrap();

        assert_eq!(result.fills.len(), 2);
        assert_eq!(result.fills[0].price, 100); // Best price first
        assert_eq!(result.fills[0].maker_order_id, 2);
        assert_eq!(result.fills[1].price, 102);
        assert_eq!(result.fills[1].maker_order_id, 1);
    }

    #[test]
    fn test_time_priority() {
        let mut book = OrderBook::new(0);
        let engine = MatchingEngine::new();

        // Add asks at same price (different accounts)
        book.add_order(make_order(1, 1, Side::Ask, 100, 10)); // First
        book.add_order(make_order(2, 2, Side::Ask, 100, 10)); // Second

        // Buy order should match first order first
        let buy = make_order(3, 3, Side::Bid, 100, 5);
        let result = engine.match_order(&mut book, buy).unwrap();

        assert_eq!(result.fills.len(), 1);
        assert_eq!(result.fills[0].maker_order_id, 1); // First order
    }

    #[test]
    fn test_market_order() {
        let mut book = OrderBook::new(0);
        let engine = MatchingEngine::new();

        book.add_order(make_order(1, 1, Side::Ask, 100, 10));

        // Market buy
        let market_buy = make_market_order(2, 2, Side::Bid, 5);
        let result = engine.match_order(&mut book, market_buy).unwrap();

        assert_eq!(result.fills.len(), 1);
        assert!(result.remaining_order.is_none()); // Market orders don't rest
    }

    #[test]
    fn test_post_only_rejection() {
        let mut book = OrderBook::new(0);
        let engine = MatchingEngine::new();

        book.add_order(make_order(1, 1, Side::Ask, 100, 10));

        // Post-only buy that would match
        let mut post_only = make_order(2, 2, Side::Bid, 100, 5);
        post_only.post_only = true;

        let result = engine.match_order(&mut book, post_only);
        assert!(matches!(result, Err(MatchingError::PostOnlyWouldTake)));
    }

    #[test]
    fn test_no_cross_prices_dont_match() {
        let mut book = OrderBook::new(0);
        let engine = MatchingEngine::new();

        book.add_order(make_order(1, 1, Side::Ask, 100, 10));

        // Bid below ask price
        let bid = make_order(2, 2, Side::Bid, 99, 10);
        let result = engine.match_order(&mut book, bid).unwrap();

        assert!(!result.has_fills());
        assert!(result.remaining_order.is_some());
        assert_eq!(book.total_orders(), 2); // Both orders in book
    }

    #[test]
    fn test_cancel_order() {
        let mut book = OrderBook::new(0);
        let engine = MatchingEngine::new();

        book.add_order(make_order(1, 42, Side::Bid, 100, 10));

        // Cancel by owner
        let cancelled = engine.cancel_order(&mut book, 1, 42).unwrap();
        assert_eq!(cancelled.id, 1);
        assert!(book.is_empty());
    }

    #[test]
    fn test_cancel_wrong_owner() {
        let mut book = OrderBook::new(0);
        let engine = MatchingEngine::new();

        book.add_order(make_order(1, 42, Side::Bid, 100, 10));

        // Try to cancel by different account
        let result = engine.cancel_order(&mut book, 1, 99);
        assert!(matches!(result, Err(MatchingError::InvalidOrder(_))));
    }
}
