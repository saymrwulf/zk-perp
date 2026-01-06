//! Order book implementation with BTreeMap for price levels

use std::collections::{BTreeMap, HashMap, VecDeque};
use zk_perp_core::types::{Order, OrderId, Price, Quantity, Side, MarketId};
use serde::{Deserialize, Serialize};

/// Location of an order in the book
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderLocation {
    /// Which side of the book
    pub side: Side,
    /// Price level
    pub price: Price,
}

/// A single price level containing orders at that price
#[derive(Clone, Debug, Default)]
pub struct PriceLevel {
    /// Price of this level
    pub price: Price,
    /// Orders at this price in FIFO order (time priority)
    pub orders: VecDeque<Order>,
    /// Total quantity at this level
    pub total_quantity: Quantity,
}

impl PriceLevel {
    /// Create a new price level
    pub fn new(price: Price) -> Self {
        Self {
            price,
            orders: VecDeque::new(),
            total_quantity: 0,
        }
    }

    /// Add an order to this level (at the back = lowest time priority)
    pub fn add_order(&mut self, order: Order) {
        self.total_quantity += order.remaining_qty;
        self.orders.push_back(order);
    }

    /// Remove an order by ID
    pub fn remove_order(&mut self, order_id: OrderId) -> Option<Order> {
        let pos = self.orders.iter().position(|o| o.id == order_id)?;
        let order = self.orders.remove(pos)?;
        self.total_quantity -= order.remaining_qty;
        Some(order)
    }

    /// Get the first order (highest time priority)
    pub fn front(&self) -> Option<&Order> {
        self.orders.front()
    }

    /// Get mutable reference to first order
    pub fn front_mut(&mut self) -> Option<&mut Order> {
        self.orders.front_mut()
    }

    /// Remove and return the first order
    pub fn pop_front(&mut self) -> Option<Order> {
        let order = self.orders.pop_front()?;
        self.total_quantity -= order.remaining_qty;
        Some(order)
    }

    /// Check if this level is empty
    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }

    /// Number of orders at this level
    pub fn len(&self) -> usize {
        self.orders.len()
    }

    /// Iterate over orders at this level
    pub fn orders(&self) -> impl Iterator<Item = &Order> {
        self.orders.iter()
    }
}

/// Order book for a single market
#[derive(Clone, Debug)]
pub struct OrderBook {
    /// Market ID
    pub market_id: MarketId,
    /// Bid side: price -> orders (highest price = best bid)
    pub bids: BTreeMap<Price, PriceLevel>,
    /// Ask side: price -> orders (lowest price = best ask)
    pub asks: BTreeMap<Price, PriceLevel>,
    /// Order ID -> location for O(1) lookups
    pub order_locations: HashMap<OrderId, OrderLocation>,
    /// Sequence number for ordering
    sequence: u64,
}

impl OrderBook {
    /// Create a new empty order book
    pub fn new(market_id: MarketId) -> Self {
        Self {
            market_id,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            order_locations: HashMap::new(),
            sequence: 0,
        }
    }

    /// Get next sequence number
    pub fn next_sequence(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }

    /// Get the best bid price
    pub fn best_bid(&self) -> Option<Price> {
        self.bids.keys().next_back().copied()
    }

    /// Get the best ask price
    pub fn best_ask(&self) -> Option<Price> {
        self.asks.keys().next().copied()
    }

    /// Get the spread (best_ask - best_bid)
    pub fn spread(&self) -> Option<Price> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) if ask > bid => Some(ask - bid),
            _ => None,
        }
    }

    /// Get the mid price
    pub fn mid_price(&self) -> Option<Price> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((bid + ask) / 2),
            (Some(bid), None) => Some(bid),
            (None, Some(ask)) => Some(ask),
            (None, None) => None,
        }
    }

    /// Get the best N bids (highest prices first)
    pub fn best_bids(&self, depth: usize) -> Vec<&Order> {
        let mut orders = Vec::new();
        for level in self.bids.values().rev() {
            for order in level.orders() {
                orders.push(order);
                if orders.len() >= depth {
                    return orders;
                }
            }
        }
        orders
    }

    /// Get the best N asks (lowest prices first)
    pub fn best_asks(&self, depth: usize) -> Vec<&Order> {
        let mut orders = Vec::new();
        for level in self.asks.values() {
            for order in level.orders() {
                orders.push(order);
                if orders.len() >= depth {
                    return orders;
                }
            }
        }
        orders
    }

    /// Add an order to the book
    pub fn add_order(&mut self, order: Order) {
        let side = order.side;
        let price = order.price;
        let order_id = order.id;

        let book = match side {
            Side::Bid => &mut self.bids,
            Side::Ask => &mut self.asks,
        };

        book.entry(price)
            .or_insert_with(|| PriceLevel::new(price))
            .add_order(order);

        self.order_locations.insert(order_id, OrderLocation { side, price });
    }

    /// Cancel an order by ID
    pub fn cancel_order(&mut self, order_id: OrderId) -> Option<Order> {
        let location = self.order_locations.remove(&order_id)?;

        let book = match location.side {
            Side::Bid => &mut self.bids,
            Side::Ask => &mut self.asks,
        };

        let level = book.get_mut(&location.price)?;
        let order = level.remove_order(order_id)?;

        // Remove empty price level
        if level.is_empty() {
            book.remove(&location.price);
        }

        Some(order)
    }

    /// Get an order by ID
    pub fn get_order(&self, order_id: OrderId) -> Option<&Order> {
        let location = self.order_locations.get(&order_id)?;

        let book = match location.side {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
        };

        let level = book.get(&location.price)?;
        level.orders.iter().find(|o| o.id == order_id)
    }

    /// Get mutable order by ID
    pub fn get_order_mut(&mut self, order_id: OrderId) -> Option<&mut Order> {
        let location = self.order_locations.get(&order_id)?.clone();

        let book = match location.side {
            Side::Bid => &mut self.bids,
            Side::Ask => &mut self.asks,
        };

        let level = book.get_mut(&location.price)?;
        level.orders.iter_mut().find(|o| o.id == order_id)
    }

    /// Get the book side for a given side
    fn get_book(&self, side: Side) -> &BTreeMap<Price, PriceLevel> {
        match side {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
        }
    }

    /// Get mutable book side
    fn get_book_mut(&mut self, side: Side) -> &mut BTreeMap<Price, PriceLevel> {
        match side {
            Side::Bid => &mut self.bids,
            Side::Ask => &mut self.asks,
        }
    }

    /// Get total quantity at a price level
    pub fn quantity_at_price(&self, side: Side, price: Price) -> Quantity {
        self.get_book(side)
            .get(&price)
            .map(|l| l.total_quantity)
            .unwrap_or(0)
    }

    /// Get order book depth (up to n levels)
    pub fn depth(&self, levels: usize) -> BookDepth {
        let bids: Vec<(Price, Quantity)> = self.bids
            .iter()
            .rev() // Highest price first
            .take(levels)
            .map(|(p, l)| (*p, l.total_quantity))
            .collect();

        let asks: Vec<(Price, Quantity)> = self.asks
            .iter() // Lowest price first
            .take(levels)
            .map(|(p, l)| (*p, l.total_quantity))
            .collect();

        BookDepth { bids, asks }
    }

    /// Total number of orders in the book
    pub fn total_orders(&self) -> usize {
        self.order_locations.len()
    }

    /// Check if book is empty
    pub fn is_empty(&self) -> bool {
        self.order_locations.is_empty()
    }

    /// Get all orders (for iteration)
    pub fn all_orders(&self) -> impl Iterator<Item = &Order> {
        self.bids.values()
            .flat_map(|l| l.orders.iter())
            .chain(self.asks.values().flat_map(|l| l.orders.iter()))
    }
}

/// Order book depth snapshot
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BookDepth {
    /// Bid levels: (price, quantity), highest price first
    pub bids: Vec<(Price, Quantity)>,
    /// Ask levels: (price, quantity), lowest price first
    pub asks: Vec<(Price, Quantity)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use zk_perp_core::types::OrderType;

    fn make_order(id: OrderId, side: Side, price: Price, qty: Quantity) -> Order {
        Order {
            id,
            account_id: 1,
            market_id: 0,
            side,
            order_type: OrderType::Limit,
            price,
            original_qty: qty,
            remaining_qty: qty,
            filled_qty: 0,
            post_only: false,
            reduce_only: false,
            time_in_force: zk_perp_core::types::TimeInForce::GTC,
            timestamp: 0,
            nonce: 0,
        }
    }

    #[test]
    fn test_empty_book() {
        let book = OrderBook::new(0);
        assert!(book.is_empty());
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), None);
        assert_eq!(book.spread(), None);
    }

    #[test]
    fn test_add_orders() {
        let mut book = OrderBook::new(0);

        book.add_order(make_order(1, Side::Bid, 100, 10));
        book.add_order(make_order(2, Side::Bid, 99, 20));
        book.add_order(make_order(3, Side::Ask, 101, 15));
        book.add_order(make_order(4, Side::Ask, 102, 25));

        assert_eq!(book.best_bid(), Some(100));
        assert_eq!(book.best_ask(), Some(101));
        assert_eq!(book.spread(), Some(1));
        assert_eq!(book.total_orders(), 4);
    }

    #[test]
    fn test_cancel_order() {
        let mut book = OrderBook::new(0);

        book.add_order(make_order(1, Side::Bid, 100, 10));
        book.add_order(make_order(2, Side::Bid, 100, 20));

        assert_eq!(book.quantity_at_price(Side::Bid, 100), 30);

        let cancelled = book.cancel_order(1);
        assert!(cancelled.is_some());
        assert_eq!(cancelled.unwrap().id, 1);
        assert_eq!(book.quantity_at_price(Side::Bid, 100), 20);
    }

    #[test]
    fn test_price_time_priority() {
        let mut book = OrderBook::new(0);

        // Add orders at same price
        book.add_order(make_order(1, Side::Bid, 100, 10)); // First
        book.add_order(make_order(2, Side::Bid, 100, 20)); // Second

        let level = book.bids.get(&100).unwrap();
        assert_eq!(level.front().unwrap().id, 1); // First order has priority
    }

    #[test]
    fn test_depth() {
        let mut book = OrderBook::new(0);

        book.add_order(make_order(1, Side::Bid, 100, 10));
        book.add_order(make_order(2, Side::Bid, 99, 20));
        book.add_order(make_order(3, Side::Ask, 101, 15));
        book.add_order(make_order(4, Side::Ask, 102, 25));

        let depth = book.depth(5);

        assert_eq!(depth.bids.len(), 2);
        assert_eq!(depth.bids[0], (100, 10)); // Best bid first
        assert_eq!(depth.bids[1], (99, 20));

        assert_eq!(depth.asks.len(), 2);
        assert_eq!(depth.asks[0], (101, 15)); // Best ask first
        assert_eq!(depth.asks[1], (102, 25));
    }
}
