//! Global state management for zk-perp
//!
//! This crate provides the GlobalState struct that manages:
//! - The hypertree (6 Merkle trees)
//! - Order books for each market
//! - State transitions and validation
//! - Witness generation for ZK proofs

use std::collections::HashMap;
use zk_perp_core::{
    types::*,
    merkle::{Hypertree, Sha256Hasher, Hash, MerkleProof, TreeId},
    transactions::*,
};
use zk_perp_orderbook::{OrderBook, MatchingEngine, MatchResult};
use serde::{Serialize, Deserialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StateError {
    #[error("Account not found: {0}")]
    AccountNotFound(AccountId),
    #[error("Market not found: {0}")]
    MarketNotFound(MarketId),
    #[error("Order not found: {0}")]
    OrderNotFound(OrderId),
    #[error("Position not found")]
    PositionNotFound,
    #[error("Insufficient balance")]
    InsufficientBalance,
    #[error("Invalid nonce: expected {expected}, got {got}")]
    InvalidNonce { expected: u64, got: u64 },
    #[error("Matching error: {0}")]
    MatchingError(String),
    #[error("Invalid signature")]
    InvalidSignature,
}

/// Witness for a single state transition (for ZK proving)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StateWitness {
    /// Pre-state root
    pub pre_root: Hash,
    /// Post-state root
    pub post_root: Hash,
    /// Account proofs
    pub account_proofs: Vec<(AccountId, MerkleProof)>,
    /// Order proofs
    pub order_proofs: Vec<(OrderId, MerkleProof)>,
    /// Position proofs
    pub position_proofs: Vec<((AccountId, MarketId), MerkleProof)>,
}

/// Result of processing a transaction
#[derive(Clone, Debug)]
pub struct ProcessResult {
    /// Transaction receipt
    pub receipt: TransactionReceipt,
    /// Witness for ZK proving
    pub witness: StateWitness,
    /// Match result (for place order)
    pub match_result: Option<MatchResult>,
}

/// Global state container
pub struct GlobalState {
    /// The hypertree containing all state
    pub hypertree: Hypertree<Sha256Hasher>,
    /// Order books per market (in-memory for fast access)
    pub orderbooks: HashMap<MarketId, OrderBook>,
    /// Accounts (in-memory cache)
    pub accounts: HashMap<AccountId, Account>,
    /// Positions (in-memory cache)
    pub positions: HashMap<(AccountId, MarketId), Position>,
    /// Markets configuration
    pub markets: HashMap<MarketId, Market>,
    /// Oracle prices
    pub oracle_prices: HashMap<MarketId, OraclePrice>,
    /// System state
    pub system: SystemState,
    /// Matching engine
    matching_engine: MatchingEngine,
}

impl Default for GlobalState {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalState {
    /// Create a new empty state
    pub fn new() -> Self {
        Self {
            hypertree: Hypertree::default(),
            orderbooks: HashMap::new(),
            accounts: HashMap::new(),
            positions: HashMap::new(),
            markets: HashMap::new(),
            oracle_prices: HashMap::new(),
            system: SystemState::default(),
            matching_engine: MatchingEngine::new(),
        }
    }

    /// Initialize with default markets (BTC-USDC, ETH-USDC)
    pub fn with_default_markets() -> Self {
        let mut state = Self::new();

        // Add BTC-USDC market
        let btc_market = markets::btc_usdc();
        state.add_market(btc_market);

        // Add ETH-USDC market
        let eth_market = markets::eth_usdc();
        state.add_market(eth_market);

        // Set initial oracle prices
        state.oracle_prices.insert(0, OraclePrice::new(0, 50000_00000000, 0)); // BTC $50k
        state.oracle_prices.insert(1, OraclePrice::new(1, 3000_00000000, 0));  // ETH $3k

        state
    }

    /// Add a market
    pub fn add_market(&mut self, market: Market) {
        let market_id = market.id;
        self.markets.insert(market_id, market);
        self.orderbooks.insert(market_id, OrderBook::new(market_id));
    }

    /// Get current state root
    pub fn root(&self) -> Hash {
        self.hypertree.root()
    }

    /// Create a new account
    pub fn create_account(&mut self, owner: PublicKey) -> AccountId {
        let id = self.system.allocate_account_id();
        let account = Account::new(id, owner, self.system.timestamp);
        self.accounts.insert(id, account.clone());
        self.hypertree.insert_account(&account);
        id
    }

    /// Get account by ID
    pub fn get_account(&self, id: AccountId) -> Option<&Account> {
        self.accounts.get(&id)
    }

    /// Get mutable account
    pub fn get_account_mut(&mut self, id: AccountId) -> Option<&mut Account> {
        self.accounts.get_mut(&id)
    }

    /// Get or create account
    pub fn get_or_create_account(&mut self, id: AccountId, owner: PublicKey) -> &mut Account {
        if !self.accounts.contains_key(&id) {
            let account = Account::new(id, owner, self.system.timestamp);
            self.accounts.insert(id, account);
            self.system.total_accounts += 1;
        }
        self.accounts.get_mut(&id).unwrap()
    }

    /// Process any transaction
    pub fn process_transaction(&mut self, tx: &Transaction) -> Result<ProcessResult, StateError> {
        match tx {
            Transaction::Deposit(deposit_tx) => self.process_deposit(deposit_tx),
            Transaction::Withdraw(withdraw_tx) => self.process_withdraw(withdraw_tx),
            Transaction::PlaceOrder(order_tx) => self.process_place_order(order_tx),
            Transaction::CancelOrder(cancel_tx) => self.process_cancel_order(cancel_tx),
            Transaction::Liquidate(liq_tx) => self.process_liquidate(liq_tx),
            Transaction::UpdateOracle(oracle_tx) => self.process_oracle_update(oracle_tx),
        }
    }

    /// Process a deposit transaction
    pub fn process_deposit(&mut self, tx: &DepositTx) -> Result<ProcessResult, StateError> {
        let pre_root = self.root();

        // Get account proof before modification
        let account_proof_before = self.hypertree.prove_account(tx.account_id);

        let account = self.accounts.get_mut(&tx.account_id)
            .ok_or(StateError::AccountNotFound(tx.account_id))?;

        // Verify nonce
        if !account.verify_nonce(tx.nonce) {
            return Err(StateError::InvalidNonce {
                expected: account.nonce + 1,
                got: tx.nonce,
            });
        }

        // Deposit funds
        account.deposit(tx.asset_id, tx.amount);
        account.increment_nonce();

        // Update hypertree
        self.hypertree.insert_account(account);

        let post_root = self.root();

        let witness = StateWitness {
            pre_root,
            post_root,
            account_proofs: vec![(tx.account_id, account_proof_before)],
            order_proofs: vec![],
            position_proofs: vec![],
        };

        Ok(ProcessResult {
            receipt: TransactionReceipt::success([0u8; 32], self.system.block_number, 0),
            witness,
            match_result: None,
        })
    }

    /// Process a withdraw transaction
    pub fn process_withdraw(&mut self, tx: &WithdrawTx) -> Result<ProcessResult, StateError> {
        let pre_root = self.root();
        let account_proof_before = self.hypertree.prove_account(tx.account_id);

        let account = self.accounts.get_mut(&tx.account_id)
            .ok_or(StateError::AccountNotFound(tx.account_id))?;

        // Verify nonce
        if !account.verify_nonce(tx.nonce) {
            return Err(StateError::InvalidNonce {
                expected: account.nonce + 1,
                got: tx.nonce,
            });
        }

        // Withdraw funds
        if !account.withdraw(tx.asset_id, tx.amount) {
            return Err(StateError::InsufficientBalance);
        }
        account.increment_nonce();

        // Update hypertree
        self.hypertree.insert_account(account);

        let post_root = self.root();

        let witness = StateWitness {
            pre_root,
            post_root,
            account_proofs: vec![(tx.account_id, account_proof_before)],
            order_proofs: vec![],
            position_proofs: vec![],
        };

        Ok(ProcessResult {
            receipt: TransactionReceipt::success([0u8; 32], self.system.block_number, 0),
            witness,
            match_result: None,
        })
    }

    /// Process a place order transaction
    pub fn process_place_order(&mut self, tx: &PlaceOrderTx) -> Result<ProcessResult, StateError> {
        let pre_root = self.root();

        let market = self.markets.get(&tx.market_id)
            .ok_or(StateError::MarketNotFound(tx.market_id))?
            .clone();

        // Collect proofs before modifications
        let account_proof_before = self.hypertree.prove_account(tx.account_id);

        let account = self.accounts.get_mut(&tx.account_id)
            .ok_or(StateError::AccountNotFound(tx.account_id))?;

        // Verify nonce
        if !account.verify_nonce(tx.nonce) {
            return Err(StateError::InvalidNonce {
                expected: account.nonce + 1,
                got: tx.nonce,
            });
        }

        // Calculate required margin
        let required_margin = market.calculate_initial_margin(tx.quantity, tx.price);

        // Check balance
        if account.free_balance(market.quote_asset) < required_margin {
            return Err(StateError::InsufficientBalance);
        }

        // Lock margin
        let balance = account.get_balance_mut(market.quote_asset)
            .ok_or(StateError::InsufficientBalance)?;
        if !balance.lock(required_margin) {
            return Err(StateError::InsufficientBalance);
        }

        // Create order
        let order_id = self.system.allocate_order_id();
        let order = tx.to_order(order_id, self.system.timestamp);

        // Match order
        let orderbook = self.orderbooks.get_mut(&tx.market_id)
            .ok_or(StateError::MarketNotFound(tx.market_id))?;

        let match_result = self.matching_engine.match_order(orderbook, order)
            .map_err(|e| StateError::MatchingError(e.to_string()))?;

        // Process fills - update positions and transfer funds
        let mut affected_accounts = vec![tx.account_id];
        for fill in &match_result.fills {
            self.process_fill(fill, &market)?;
            if !affected_accounts.contains(&fill.maker_account_id) {
                affected_accounts.push(fill.maker_account_id);
            }
        }

        // Update taker account nonce
        let account = self.accounts.get_mut(&tx.account_id).unwrap();
        account.increment_nonce();

        // Update hypertree for all affected accounts
        for &account_id in &affected_accounts {
            if let Some(account) = self.accounts.get(&account_id) {
                self.hypertree.insert_account(account);
            }
        }

        let post_root = self.root();

        let witness = StateWitness {
            pre_root,
            post_root,
            account_proofs: vec![(tx.account_id, account_proof_before)],
            order_proofs: vec![],
            position_proofs: vec![],
        };

        let receipt = TransactionReceipt::success([0u8; 32], self.system.block_number, 0)
            .with_fills(match_result.fills.clone())
            .with_order_id(order_id);

        Ok(ProcessResult {
            receipt,
            witness,
            match_result: Some(match_result),
        })
    }

    /// Process a fill (internal)
    fn process_fill(&mut self, fill: &Fill, market: &Market) -> Result<(), StateError> {
        // Calculate notional value
        let notional = (fill.quantity as u128) * (fill.price as u128) / 1_000_000_000_000_000_000;

        // Update maker account - credit/debit based on side
        if let Some(maker_account) = self.accounts.get_mut(&fill.maker_account_id) {
            // Unlock margin for maker
            if let Some(balance) = maker_account.get_balance_mut(market.quote_asset) {
                let margin_to_unlock = market.calculate_initial_margin(fill.quantity, fill.price);
                balance.unlock(margin_to_unlock);
            }

            // Apply maker fee (can be negative = rebate)
            if fill.maker_fee < 0 {
                // Rebate
                maker_account.deposit(market.quote_asset, (-fill.maker_fee) as u128);
            } else {
                // Fee
                if let Some(balance) = maker_account.get_balance_mut(market.quote_asset) {
                    balance.debit(fill.maker_fee as u128);
                }
            }
        }

        // Update taker account
        if let Some(taker_account) = self.accounts.get_mut(&fill.taker_account_id) {
            // Deduct taker fee
            if let Some(balance) = taker_account.get_balance_mut(market.quote_asset) {
                balance.debit(fill.taker_fee as u128);
            }
        }

        // Update positions for both maker and taker
        self.update_position_from_fill(fill, market)?;

        Ok(())
    }

    /// Update position from a fill
    fn update_position_from_fill(&mut self, fill: &Fill, market: &Market) -> Result<(), StateError> {
        // Determine taker's position side
        let taker_position_side = match fill.taker_side {
            Side::Bid => PositionSide::Long,
            Side::Ask => PositionSide::Short,
        };

        // Update taker position
        let taker_key = (fill.taker_account_id, market.id);
        let taker_position = self.positions.entry(taker_key).or_insert_with(|| {
            Position::new(
                fill.taker_account_id,
                market.id,
                PositionSide::None,
                0,
                0,
                0,
                1,
                fill.timestamp,
            )
        });

        if taker_position.side == PositionSide::None {
            // Open new position
            taker_position.side = taker_position_side;
            taker_position.size = fill.quantity;
            taker_position.entry_price = fill.price;
        } else if taker_position.side == taker_position_side {
            // Increase position
            taker_position.increase(fill.quantity, fill.price, 0, fill.timestamp);
        } else {
            // Reduce or flip position
            if fill.quantity >= taker_position.size {
                // Close and potentially flip
                let remaining = fill.quantity - taker_position.size;
                taker_position.decrease(taker_position.size, fill.price, fill.timestamp);
                if remaining > 0 {
                    taker_position.side = taker_position_side;
                    taker_position.size = remaining;
                    taker_position.entry_price = fill.price;
                }
            } else {
                // Partial close
                taker_position.decrease(fill.quantity, fill.price, fill.timestamp);
            }
        }

        // Update hypertree
        self.hypertree.insert_position(taker_position);

        Ok(())
    }

    /// Process a cancel order transaction
    pub fn process_cancel_order(&mut self, tx: &CancelOrderTx) -> Result<ProcessResult, StateError> {
        let pre_root = self.root();
        let account_proof_before = self.hypertree.prove_account(tx.account_id);

        // Verify account exists
        let account = self.accounts.get_mut(&tx.account_id)
            .ok_or(StateError::AccountNotFound(tx.account_id))?;

        // Verify nonce
        if !account.verify_nonce(tx.nonce) {
            return Err(StateError::InvalidNonce {
                expected: account.nonce + 1,
                got: tx.nonce,
            });
        }

        // Cancel order in orderbook
        let orderbook = self.orderbooks.get_mut(&tx.market_id)
            .ok_or(StateError::MarketNotFound(tx.market_id))?;

        let cancelled_order = self.matching_engine.cancel_order(orderbook, tx.order_id, tx.account_id)
            .map_err(|e| StateError::MatchingError(e.to_string()))?;

        // Unlock margin
        let market = self.markets.get(&tx.market_id)
            .ok_or(StateError::MarketNotFound(tx.market_id))?;

        let margin_to_unlock = market.calculate_initial_margin(
            cancelled_order.remaining_qty,
            cancelled_order.price,
        );

        let account = self.accounts.get_mut(&tx.account_id).unwrap();
        if let Some(balance) = account.get_balance_mut(market.quote_asset) {
            balance.unlock(margin_to_unlock);
        }
        account.increment_nonce();

        // Update hypertree
        self.hypertree.insert_account(account);
        self.hypertree.remove_order(&cancelled_order);

        let post_root = self.root();

        let witness = StateWitness {
            pre_root,
            post_root,
            account_proofs: vec![(tx.account_id, account_proof_before)],
            order_proofs: vec![],
            position_proofs: vec![],
        };

        Ok(ProcessResult {
            receipt: TransactionReceipt::success([0u8; 32], self.system.block_number, 0),
            witness,
            match_result: None,
        })
    }

    /// Process liquidation (simplified - full liquidation only)
    pub fn process_liquidate(&mut self, tx: &LiquidateTx) -> Result<ProcessResult, StateError> {
        let pre_root = self.root();

        // Get position
        let position_key = (tx.liquidatee_account_id, tx.market_id);
        let position = self.positions.get(&position_key)
            .ok_or(StateError::PositionNotFound)?
            .clone();

        // Get oracle price
        let oracle_price = self.oracle_prices.get(&tx.market_id)
            .ok_or(StateError::MarketNotFound(tx.market_id))?;

        // Get market
        let market = self.markets.get(&tx.market_id)
            .ok_or(StateError::MarketNotFound(tx.market_id))?
            .clone();

        // Verify position is liquidatable
        if !position.is_liquidatable(oracle_price.price, market.maintenance_margin_bps) {
            return Err(StateError::MatchingError("Position not liquidatable".to_string()));
        }

        // Close position at oracle price
        let position = self.positions.get_mut(&position_key).unwrap();
        let pnl = position.decrease(position.size, oracle_price.price, self.system.timestamp);

        // Transfer margin and PnL
        // Simplified: liquidator gets a bonus, liquidatee loses margin
        let liquidation_bonus = position.margin / 10; // 10% bonus

        // Update liquidatee account
        if let Some(account) = self.accounts.get_mut(&tx.liquidatee_account_id) {
            // Remove remaining margin (already lost)
            if let Some(balance) = account.get_balance_mut(market.quote_asset) {
                balance.deduct_locked(position.margin.saturating_sub(liquidation_bonus));
            }
        }

        // Update liquidator account
        if let Some(account) = self.accounts.get_mut(&tx.liquidator_account_id) {
            account.deposit(market.quote_asset, liquidation_bonus);
            account.increment_nonce();
        }

        // Update hypertree
        if let Some(account) = self.accounts.get(&tx.liquidatee_account_id) {
            self.hypertree.insert_account(account);
        }
        if let Some(account) = self.accounts.get(&tx.liquidator_account_id) {
            self.hypertree.insert_account(account);
        }
        self.hypertree.remove_position(tx.liquidatee_account_id, tx.market_id);

        let post_root = self.root();

        let witness = StateWitness {
            pre_root,
            post_root,
            account_proofs: vec![],
            order_proofs: vec![],
            position_proofs: vec![(position_key, self.hypertree.prove_position(position_key.0, position_key.1))],
        };

        Ok(ProcessResult {
            receipt: TransactionReceipt::success([0u8; 32], self.system.block_number, 0),
            witness,
            match_result: None,
        })
    }

    /// Process oracle update (sequencer only)
    pub fn process_oracle_update(&mut self, tx: &UpdateOracleTx) -> Result<ProcessResult, StateError> {
        let pre_root = self.root();

        self.oracle_prices.insert(tx.market_id, OraclePrice {
            market_id: tx.market_id,
            price: tx.price,
            timestamp: tx.timestamp,
            confidence_bps: tx.confidence_bps,
        });

        let post_root = self.root();

        let witness = StateWitness {
            pre_root,
            post_root,
            account_proofs: vec![],
            order_proofs: vec![],
            position_proofs: vec![],
        };

        Ok(ProcessResult {
            receipt: TransactionReceipt::success([0u8; 32], self.system.block_number, 0),
            witness,
            match_result: None,
        })
    }

    /// Advance to next block
    pub fn next_block(&mut self, timestamp: Timestamp) {
        self.system.block_number += 1;
        self.system.timestamp = timestamp;
        self.matching_engine.set_timestamp(timestamp);
    }

    /// Get current oracle price for a market
    pub fn get_oracle_price(&self, market_id: MarketId) -> Option<&OraclePrice> {
        self.oracle_prices.get(&market_id)
    }

    /// Get order book depth (bids and asks at top price levels)
    pub fn get_orderbook_depth(&self, market_id: MarketId, depth: usize) -> (Vec<(Price, Quantity)>, Vec<(Price, Quantity)>) {
        match self.orderbooks.get(&market_id) {
            Some(orderbook) => {
                let bids = orderbook.best_bids(depth)
                    .into_iter()
                    .map(|o| (o.price, o.remaining_qty))
                    .collect();
                let asks = orderbook.best_asks(depth)
                    .into_iter()
                    .map(|o| (o.price, o.remaining_qty))
                    .collect();
                (bids, asks)
            }
            None => (Vec::new(), Vec::new()),
        }
    }

    /// Get all positions for an account
    pub fn get_positions(&self, account_id: AccountId) -> Vec<Position> {
        self.positions
            .iter()
            .filter(|((aid, _), _)| *aid == account_id)
            .map(|(_, pos)| pos.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_state() {
        let state = GlobalState::with_default_markets();
        assert_eq!(state.markets.len(), 2);
        assert!(state.markets.contains_key(&0)); // BTC-USDC
        assert!(state.markets.contains_key(&1)); // ETH-USDC
    }

    #[test]
    fn test_create_account() {
        let mut state = GlobalState::with_default_markets();
        let owner = [1u8; 32];
        let id = state.create_account(owner);

        assert_eq!(id, 0);
        assert!(state.accounts.contains_key(&id));
        assert_eq!(state.accounts.get(&id).unwrap().owner, owner);
    }

    #[test]
    fn test_deposit_and_withdraw() {
        let mut state = GlobalState::with_default_markets();
        let owner = [1u8; 32];
        let account_id = state.create_account(owner);

        // Deposit
        let deposit_tx = DepositTx {
            account_id,
            asset_id: assets::USDC,
            amount: 10000_000000, // 10,000 USDC
            nonce: 1,
        };

        let result = state.process_deposit(&deposit_tx).unwrap();
        assert!(result.receipt.success);

        let account = state.accounts.get(&account_id).unwrap();
        assert_eq!(account.free_balance(assets::USDC), 10000_000000);

        // Withdraw
        let withdraw_tx = WithdrawTx {
            account_id,
            asset_id: assets::USDC,
            amount: 5000_000000,
            nonce: 2,
        };

        let result = state.process_withdraw(&withdraw_tx).unwrap();
        assert!(result.receipt.success);

        let account = state.accounts.get(&account_id).unwrap();
        assert_eq!(account.free_balance(assets::USDC), 5000_000000);
    }

    #[test]
    fn test_place_order() {
        let mut state = GlobalState::with_default_markets();
        let owner = [1u8; 32];
        let account_id = state.create_account(owner);

        // Deposit funds first
        let deposit_tx = DepositTx {
            account_id,
            asset_id: assets::USDC,
            amount: 100000_000000, // 100,000 USDC
            nonce: 1,
        };
        state.process_deposit(&deposit_tx).unwrap();

        // Place a limit order
        let order_tx = PlaceOrderTx {
            account_id,
            market_id: 0, // BTC-USDC
            side: Side::Bid,
            order_type: OrderType::Limit,
            price: 50000_00000000, // $50,000
            quantity: 100_000_000_000_000_000, // 0.1 BTC
            post_only: false,
            reduce_only: false,
            nonce: 2,
        };

        let result = state.process_place_order(&order_tx).unwrap();
        assert!(result.receipt.success);
        assert!(result.receipt.order_id.is_some());

        // Order should be in orderbook (no match since no liquidity)
        let orderbook = state.orderbooks.get(&0).unwrap();
        assert_eq!(orderbook.total_orders(), 1);
    }
}
