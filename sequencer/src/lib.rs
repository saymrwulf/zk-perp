//! Sequencer for zk-perp
//!
//! The sequencer is the heart of the zk-perp system. It:
//! - Receives and validates signed transactions
//! - Executes transactions against the state
//! - Batches transactions and triggers ZK proof generation
//! - Stores batches and proofs in the DA layer
//! - Provides an HTTP API for clients
//!
//! ## Architecture
//!
//! ```text
//! Client -> HTTP API -> Sequencer -> State
//!                          |            |
//!                          v            v
//!                       Batcher -> DA Layer
//!                          |
//!                          v
//!                       Prover (RISC Zero)
//! ```

pub mod api;

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time;
use tracing::{info, error, debug};

use zk_perp_core::{
    transactions::*,
    types::*,
    merkle::Hash,
    crypto::verify_transaction,
    BatchInput, TransactionWitness,
};
use zk_perp_state::GlobalState;
use zk_perp_oracle::{MockOracle, PriceSource};
use zk_perp_da::{AppendLog, DaStats, DaError};
use zk_perp_host::Prover;

/// Sequencer configuration
#[derive(Clone, Debug)]
pub struct SequencerConfig {
    /// Maximum transactions per batch
    pub max_batch_size: usize,
    /// Batch timeout in milliseconds (trigger batch if timeout reached)
    pub batch_timeout_ms: u64,
    /// HTTP server port
    pub port: u16,
    /// Data directory for DA layer
    pub data_dir: String,
    /// Use mock proving (faster, for development)
    pub use_mock_prover: bool,
    /// Enable signature verification
    pub verify_signatures: bool,
}

impl Default for SequencerConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 100,
            batch_timeout_ms: 5000, // 5 seconds
            port: 8080,
            data_dir: "./data".to_string(),
            use_mock_prover: true, // Default to mock for faster development
            verify_signatures: true,
        }
    }
}

/// Pending transaction in the mempool
#[derive(Clone, Debug)]
pub struct PendingTx {
    pub signed_tx: SignedTransaction,
    pub received_at: Instant,
}

/// Result of batch processing
#[derive(Clone, Debug)]
pub struct BatchResult {
    pub batch_id: u64,
    pub tx_count: usize,
    pub pre_root: Hash,
    pub post_root: Hash,
    pub proof_generated: bool,
}

/// Sequencer state and logic
pub struct Sequencer {
    pub config: SequencerConfig,
    pub state: Arc<RwLock<GlobalState>>,
    pub oracle: Arc<RwLock<MockOracle>>,
    pub da: Arc<RwLock<AppendLog>>,
    pub prover: Prover,
    /// Pending transactions (mempool)
    pub pending_txs: Arc<RwLock<Vec<PendingTx>>>,
    /// Account public keys for signature verification
    pub account_keys: Arc<RwLock<std::collections::HashMap<AccountId, PublicKey>>>,
    /// Stats
    pub stats: Arc<RwLock<SequencerStats>>,
}

/// Sequencer statistics
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SequencerStats {
    pub total_transactions: u64,
    pub total_batches: u64,
    pub failed_transactions: u64,
    pub total_proofs_generated: u64,
}

impl Sequencer {
    /// Create a new sequencer with file-based DA
    pub fn new(config: SequencerConfig) -> Result<Self, DaError> {
        let da = AppendLog::new(&config.data_dir)?;

        let prover = if config.use_mock_prover {
            Prover::mock()
        } else {
            Prover::new()
        };

        let mut oracle = MockOracle::with_defaults();
        oracle.refresh_timestamps();

        Ok(Self {
            config,
            state: Arc::new(RwLock::new(GlobalState::with_default_markets())),
            oracle: Arc::new(RwLock::new(oracle)),
            da: Arc::new(RwLock::new(da)),
            prover,
            pending_txs: Arc::new(RwLock::new(Vec::new())),
            account_keys: Arc::new(RwLock::new(std::collections::HashMap::new())),
            stats: Arc::new(RwLock::new(SequencerStats::default())),
        })
    }

    /// Create a sequencer with in-memory DA (for testing)
    pub fn in_memory(config: SequencerConfig) -> Self {
        let prover = if config.use_mock_prover {
            Prover::mock()
        } else {
            Prover::new()
        };

        let mut oracle = MockOracle::with_defaults();
        oracle.refresh_timestamps();

        Self {
            config,
            state: Arc::new(RwLock::new(GlobalState::with_default_markets())),
            oracle: Arc::new(RwLock::new(oracle)),
            da: Arc::new(RwLock::new(AppendLog::in_memory())),
            prover,
            pending_txs: Arc::new(RwLock::new(Vec::new())),
            account_keys: Arc::new(RwLock::new(std::collections::HashMap::new())),
            stats: Arc::new(RwLock::new(SequencerStats::default())),
        }
    }

    /// Register an account with its public key
    pub async fn register_account(&self, public_key: PublicKey) -> AccountId {
        let account_id = {
            let mut state = self.state.write().await;
            state.create_account(public_key)
        };

        // Store public key for signature verification
        self.account_keys.write().await.insert(account_id, public_key);

        info!("Registered account {} with public key", account_id);
        account_id
    }

    /// Submit a signed transaction
    pub async fn submit_transaction(&self, signed_tx: SignedTransaction) -> Result<TransactionReceipt, SequencerError> {
        // Validate signature if enabled
        if self.config.verify_signatures {
            self.verify_signature(&signed_tx).await?;
        }

        // Validate transaction
        self.validate_transaction(&signed_tx.tx).await?;

        // Execute transaction
        let receipt = self.execute_transaction(&signed_tx.tx).await?;

        // Add to pending batch
        if receipt.success {
            self.pending_txs.write().await.push(PendingTx {
                signed_tx,
                received_at: Instant::now(),
            });

            // Check if we should trigger a batch
            self.maybe_trigger_batch().await?;
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_transactions += 1;
            if !receipt.success {
                stats.failed_transactions += 1;
            }
        }

        Ok(receipt)
    }

    /// Verify transaction signature
    async fn verify_signature(&self, signed_tx: &SignedTransaction) -> Result<(), SequencerError> {
        let account_id = signed_tx.tx.signer();

        // Get public key for account
        let public_key = self.account_keys.read().await
            .get(&account_id)
            .copied()
            .ok_or(SequencerError::AccountNotFound(account_id))?;

        // Verify signature
        let is_valid = verify_transaction(signed_tx, &public_key)
            .map_err(|e| SequencerError::InvalidSignature(e.to_string()))?;

        if !is_valid {
            return Err(SequencerError::InvalidSignature("Signature verification failed".to_string()));
        }

        Ok(())
    }

    /// Validate transaction before execution
    async fn validate_transaction(&self, tx: &Transaction) -> Result<(), SequencerError> {
        let state = self.state.read().await;

        match tx {
            Transaction::Deposit(deposit) => {
                // Check account exists
                if state.get_account(deposit.account_id).is_none() {
                    return Err(SequencerError::AccountNotFound(deposit.account_id));
                }
            }
            Transaction::Withdraw(withdraw) => {
                // Check account exists and has sufficient balance
                let account = state.get_account(withdraw.account_id)
                    .ok_or(SequencerError::AccountNotFound(withdraw.account_id))?;

                if account.free_balance(withdraw.asset_id) < withdraw.amount {
                    return Err(SequencerError::InsufficientBalance);
                }
            }
            Transaction::PlaceOrder(order) => {
                // Check account exists
                if state.get_account(order.account_id).is_none() {
                    return Err(SequencerError::AccountNotFound(order.account_id));
                }
            }
            Transaction::CancelOrder(cancel) => {
                // Check account exists
                if state.get_account(cancel.account_id).is_none() {
                    return Err(SequencerError::AccountNotFound(cancel.account_id));
                }
            }
            Transaction::Liquidate(liquidate) => {
                // Check both accounts exist
                if state.get_account(liquidate.liquidator_account_id).is_none() {
                    return Err(SequencerError::AccountNotFound(liquidate.liquidator_account_id));
                }
                if state.get_account(liquidate.liquidatee_account_id).is_none() {
                    return Err(SequencerError::AccountNotFound(liquidate.liquidatee_account_id));
                }
            }
            Transaction::UpdateOracle(_) => {
                // Oracle updates are always valid (from sequencer)
            }
        }

        Ok(())
    }

    /// Execute a transaction against state
    async fn execute_transaction(&self, tx: &Transaction) -> Result<TransactionReceipt, SequencerError> {
        let mut state = self.state.write().await;

        let result = match tx {
            Transaction::Deposit(deposit_tx) => {
                state.process_deposit(deposit_tx)
                    .map_err(|e| SequencerError::ExecutionError(e.to_string()))?
            }
            Transaction::Withdraw(withdraw_tx) => {
                state.process_withdraw(withdraw_tx)
                    .map_err(|e| SequencerError::ExecutionError(e.to_string()))?
            }
            Transaction::PlaceOrder(order_tx) => {
                state.process_place_order(order_tx)
                    .map_err(|e| SequencerError::ExecutionError(e.to_string()))?
            }
            Transaction::CancelOrder(cancel_tx) => {
                state.process_cancel_order(cancel_tx)
                    .map_err(|e| SequencerError::ExecutionError(e.to_string()))?
            }
            Transaction::Liquidate(liquidate_tx) => {
                state.process_liquidate(liquidate_tx)
                    .map_err(|e| SequencerError::ExecutionError(e.to_string()))?
            }
            Transaction::UpdateOracle(oracle_tx) => {
                state.process_oracle_update(oracle_tx)
                    .map_err(|e| SequencerError::ExecutionError(e.to_string()))?
            }
        };

        Ok(result.receipt)
    }

    /// Check if we should trigger batch processing
    async fn maybe_trigger_batch(&self) -> Result<(), SequencerError> {
        let pending_count = self.pending_txs.read().await.len();

        if pending_count >= self.config.max_batch_size {
            debug!("Batch size threshold reached, triggering batch");
            self.process_batch().await?;
        }

        Ok(())
    }

    /// Force process current pending transactions as a batch
    pub async fn process_batch(&self) -> Result<Option<BatchResult>, SequencerError> {
        let mut pending = self.pending_txs.write().await;

        if pending.is_empty() {
            return Ok(None);
        }

        // Take all pending transactions
        let batch_txs: Vec<_> = pending.drain(..).collect();
        let tx_count = batch_txs.len();

        drop(pending); // Release lock

        info!("Processing batch with {} transactions", tx_count);

        // Get current state root
        let state = self.state.read().await;
        let post_root = state.root();

        // Get pre-root from DA (or use zero for first batch)
        let da = self.da.read().await;
        let pre_root = if da.last_batch_id() == 0 {
            [0u8; 32] // Genesis
        } else {
            da.current_state_root()
        };
        drop(da);

        // Compute batch hash
        let batch_hash = self.compute_batch_hash(&batch_txs);

        // Store in DA
        let batch_id = {
            let mut da = self.da.write().await;
            let transactions: Vec<Transaction> = batch_txs.iter().map(|ptx| ptx.signed_tx.tx.clone()).collect();
            da.append_batch(transactions, pre_root, post_root, batch_hash)
                .map_err(|e| SequencerError::DaError(e.to_string()))?
        };

        info!("Batch {} stored in DA", batch_id);

        // Generate ZK proof
        let proof_generated = self.generate_batch_proof(batch_id, &batch_txs, pre_root, post_root).await?;

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_batches += 1;
            if proof_generated {
                stats.total_proofs_generated += 1;
            }
        }

        Ok(Some(BatchResult {
            batch_id,
            tx_count,
            pre_root,
            post_root,
            proof_generated,
        }))
    }

    /// Generate ZK proof for a batch
    async fn generate_batch_proof(
        &self,
        batch_id: u64,
        batch_txs: &[PendingTx],
        pre_root: Hash,
        post_root: Hash,
    ) -> Result<bool, SequencerError> {
        info!("Generating ZK proof for batch {}", batch_id);

        let transactions: Vec<Transaction> = batch_txs.iter()
            .map(|ptx| ptx.signed_tx.tx.clone())
            .collect();

        let witnesses: Vec<TransactionWitness> = (0..transactions.len())
            .map(|_| TransactionWitness::default())
            .collect();

        let batch_input = BatchInput {
            pre_state_root: pre_root,
            post_state_root: post_root,
            transactions,
            witnesses,
        };

        // Generate proof (this can take time for real proving)
        match self.prover.prove(batch_input) {
            Ok(receipt) => {
                info!("Proof generated for batch {}", batch_id);

                // Store proof in DA
                let mut da = self.da.write().await;
                da.append_proof(batch_id, receipt.seal, [0u32; 8])
                    .map_err(|e| SequencerError::DaError(e.to_string()))?;

                Ok(true)
            }
            Err(e) => {
                error!("Failed to generate proof for batch {}: {}", batch_id, e);
                Ok(false)
            }
        }
    }

    /// Compute hash of batch transactions
    fn compute_batch_hash(&self, batch_txs: &[PendingTx]) -> Hash {
        use sha2::{Sha256, Digest};
        use zk_perp_core::merkle::ZERO_HASH;

        let mut hasher = Sha256::new();

        for ptx in batch_txs {
            let tx_bytes = bincode::serialize(&ptx.signed_tx.tx).unwrap_or_default();
            hasher.update(&tx_bytes);
        }

        let result = hasher.finalize();
        let mut hash = ZERO_HASH;
        hash.copy_from_slice(&result);
        hash
    }

    /// Update oracle prices
    pub async fn update_oracle_prices(&self) {
        let oracle = self.oracle.read().await;
        let prices = oracle.get_all_prices();
        drop(oracle);

        for price in prices {
            let oracle_tx = UpdateOracleTx {
                market_id: price.market_id,
                price: price.price,
                timestamp: price.timestamp,
                confidence_bps: 100, // 1% confidence
                nonce: 0, // Oracle updates don't use nonces
            };

            let signed_tx = SignedTransaction {
                tx: Transaction::UpdateOracle(oracle_tx),
                signature: vec![], // Sequencer doesn't need to sign oracle updates
            };

            // Process without signature verification
            let _ = self.execute_transaction(&signed_tx.tx).await;
        }
    }

    /// Get account info
    pub async fn get_account(&self, account_id: AccountId) -> Option<Account> {
        let state = self.state.read().await;
        state.get_account(account_id).cloned()
    }

    /// Get order book depth for a market
    pub async fn get_orderbook(&self, market_id: MarketId, depth: usize) -> OrderBookSnapshot {
        let state = self.state.read().await;
        let (bids, asks) = state.get_orderbook_depth(market_id, depth);

        OrderBookSnapshot {
            market_id,
            bids,
            asks,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        }
    }

    /// Get positions for an account
    pub async fn get_positions(&self, account_id: AccountId) -> Vec<Position> {
        let state = self.state.read().await;
        state.get_positions(account_id)
    }

    /// Get DA statistics
    pub async fn get_da_stats(&self) -> DaStats {
        let da = self.da.read().await;
        da.stats()
    }

    /// Get sequencer statistics
    pub async fn get_stats(&self) -> SequencerStats {
        self.stats.read().await.clone()
    }

    /// Start background batch timer
    pub fn start_batch_timer(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let sequencer = self.clone();
        let timeout = Duration::from_millis(self.config.batch_timeout_ms);

        tokio::spawn(async move {
            let mut interval = time::interval(timeout);

            loop {
                interval.tick().await;

                let pending_count = sequencer.pending_txs.read().await.len();

                if pending_count > 0 {
                    debug!("Batch timeout reached with {} pending transactions", pending_count);

                    if let Err(e) = sequencer.process_batch().await {
                        error!("Failed to process batch: {}", e);
                    }
                }
            }
        })
    }
}

/// Order book snapshot for API
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct OrderBookSnapshot {
    pub market_id: MarketId,
    pub bids: Vec<(Price, Quantity)>,
    pub asks: Vec<(Price, Quantity)>,
    pub timestamp: Timestamp,
}

/// Sequencer errors
#[derive(Debug, thiserror::Error)]
pub enum SequencerError {
    #[error("Account not found: {0}")]
    AccountNotFound(AccountId),
    #[error("Invalid signature: {0}")]
    InvalidSignature(String),
    #[error("Insufficient balance")]
    InsufficientBalance,
    #[error("Execution error: {0}")]
    ExecutionError(String),
    #[error("DA error: {0}")]
    DaError(String),
    #[error("Proof generation error: {0}")]
    ProofError(String),
}

