//! Transaction types for the perpetual DEX

use serde::{Deserialize, Serialize};
use crate::types::*;
use crate::merkle::Hash;

/// A signed transaction
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedTransaction {
    /// The transaction payload
    pub tx: Transaction,
    /// Ed25519 signature
    pub signature: Signature,
}

/// Transaction types
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Transaction {
    /// Deposit funds to account
    Deposit(DepositTx),
    /// Withdraw funds from account
    Withdraw(WithdrawTx),
    /// Place a new order
    PlaceOrder(PlaceOrderTx),
    /// Cancel an existing order
    CancelOrder(CancelOrderTx),
    /// Liquidate an unhealthy position
    Liquidate(LiquidateTx),
    /// Update oracle price (sequencer only)
    UpdateOracle(UpdateOracleTx),
}

impl Transaction {
    /// Get the account ID that signed this transaction
    pub fn signer(&self) -> AccountId {
        match self {
            Transaction::Deposit(tx) => tx.account_id,
            Transaction::Withdraw(tx) => tx.account_id,
            Transaction::PlaceOrder(tx) => tx.account_id,
            Transaction::CancelOrder(tx) => tx.account_id,
            Transaction::Liquidate(tx) => tx.liquidator_account_id,
            Transaction::UpdateOracle(_) => 0, // Sequencer
        }
    }

    /// Get the nonce for this transaction
    pub fn nonce(&self) -> u64 {
        match self {
            Transaction::Deposit(tx) => tx.nonce,
            Transaction::Withdraw(tx) => tx.nonce,
            Transaction::PlaceOrder(tx) => tx.nonce,
            Transaction::CancelOrder(tx) => tx.nonce,
            Transaction::Liquidate(tx) => tx.nonce,
            Transaction::UpdateOracle(tx) => tx.nonce,
        }
    }
}

/// Deposit transaction
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DepositTx {
    /// Account to deposit to
    pub account_id: AccountId,
    /// Asset to deposit
    pub asset_id: AssetId,
    /// Amount to deposit
    pub amount: u128,
    /// Nonce
    pub nonce: u64,
}

/// Withdraw transaction
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WithdrawTx {
    /// Account to withdraw from
    pub account_id: AccountId,
    /// Asset to withdraw
    pub asset_id: AssetId,
    /// Amount to withdraw
    pub amount: u128,
    /// Nonce
    pub nonce: u64,
}

/// Place order transaction
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaceOrderTx {
    /// Account placing the order
    pub account_id: AccountId,
    /// Market to trade
    pub market_id: MarketId,
    /// Buy or sell
    pub side: Side,
    /// Order type
    pub order_type: OrderType,
    /// Limit price (8 decimals)
    pub price: Price,
    /// Quantity (18 decimals)
    pub quantity: Quantity,
    /// Post-only flag
    pub post_only: bool,
    /// Reduce-only flag
    pub reduce_only: bool,
    /// Nonce
    pub nonce: u64,
}

impl PlaceOrderTx {
    /// Convert to an Order
    pub fn to_order(&self, id: OrderId, timestamp: Timestamp) -> Order {
        Order {
            id,
            account_id: self.account_id,
            market_id: self.market_id,
            side: self.side,
            order_type: self.order_type,
            price: self.price,
            original_qty: self.quantity,
            remaining_qty: self.quantity,
            filled_qty: 0,
            post_only: self.post_only,
            reduce_only: self.reduce_only,
            time_in_force: TimeInForce::GTC,
            timestamp,
            nonce: self.nonce,
        }
    }
}

/// Cancel order transaction
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CancelOrderTx {
    /// Account that owns the order
    pub account_id: AccountId,
    /// Order to cancel
    pub order_id: OrderId,
    /// Market the order is in
    pub market_id: MarketId,
    /// Nonce
    pub nonce: u64,
}

/// Liquidate transaction
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LiquidateTx {
    /// Account performing liquidation (receives bonus)
    pub liquidator_account_id: AccountId,
    /// Account being liquidated
    pub liquidatee_account_id: AccountId,
    /// Market of the position to liquidate
    pub market_id: MarketId,
    /// Amount to liquidate (can be partial)
    pub quantity: Quantity,
    /// Nonce
    pub nonce: u64,
}

/// Update oracle price transaction (sequencer only)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateOracleTx {
    /// Market to update
    pub market_id: MarketId,
    /// New price
    pub price: Price,
    /// Timestamp of price
    pub timestamp: Timestamp,
    /// Confidence (basis points)
    pub confidence_bps: u16,
    /// Nonce
    pub nonce: u64,
}

/// Transaction receipt (result of execution)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransactionReceipt {
    /// Transaction hash
    pub tx_hash: Hash,
    /// Whether transaction succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
    /// Gas used (for future use)
    pub gas_used: u64,
    /// Fills that occurred (for place order)
    pub fills: Vec<Fill>,
    /// New order ID if order was placed
    pub order_id: Option<OrderId>,
    /// Block number
    pub block_number: u64,
    /// Transaction index in block
    pub tx_index: u32,
}

impl TransactionReceipt {
    /// Create a successful receipt
    pub fn success(tx_hash: Hash, block_number: u64, tx_index: u32) -> Self {
        Self {
            tx_hash,
            success: true,
            error: None,
            gas_used: 0,
            fills: Vec::new(),
            order_id: None,
            block_number,
            tx_index,
        }
    }

    /// Create a failed receipt
    pub fn failure(tx_hash: Hash, error: String, block_number: u64, tx_index: u32) -> Self {
        Self {
            tx_hash,
            success: false,
            error: Some(error),
            gas_used: 0,
            fills: Vec::new(),
            order_id: None,
            block_number,
            tx_index,
        }
    }

    /// Add fills to receipt
    pub fn with_fills(mut self, fills: Vec<Fill>) -> Self {
        self.fills = fills;
        self
    }

    /// Add order ID to receipt
    pub fn with_order_id(mut self, order_id: OrderId) -> Self {
        self.order_id = Some(order_id);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_place_order_to_order() {
        let tx = PlaceOrderTx {
            account_id: 1,
            market_id: 0,
            side: Side::Bid,
            order_type: OrderType::Limit,
            price: 50000_00000000,
            quantity: 1_000_000_000_000_000_000,
            post_only: false,
            reduce_only: false,
            nonce: 1,
        };

        let order = tx.to_order(100, 12345);

        assert_eq!(order.id, 100);
        assert_eq!(order.account_id, 1);
        assert_eq!(order.market_id, 0);
        assert_eq!(order.side, Side::Bid);
        assert_eq!(order.price, 50000_00000000);
        assert_eq!(order.remaining_qty, 1_000_000_000_000_000_000);
        assert_eq!(order.timestamp, 12345);
    }

    #[test]
    fn test_transaction_signer() {
        let deposit = Transaction::Deposit(DepositTx {
            account_id: 42,
            asset_id: 0,
            amount: 1000,
            nonce: 1,
        });

        assert_eq!(deposit.signer(), 42);
        assert_eq!(deposit.nonce(), 1);
    }
}
