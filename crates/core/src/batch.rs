//! Batch types for ZK proving
//!
//! These types are shared between the host (prover) and guest (zkVM).

use serde::{Serialize, Deserialize};
use crate::merkle::{Hash, MerkleProof};
use crate::transactions::Transaction;
use crate::types::{Account, Order, Position};

/// Batch input for ZK verification
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BatchInput {
    /// Root hash before batch execution
    pub pre_state_root: Hash,
    /// Root hash after batch execution
    pub post_state_root: Hash,
    /// Transactions in this batch
    pub transactions: Vec<Transaction>,
    /// Merkle witnesses for each transaction
    pub witnesses: Vec<TransactionWitness>,
}

/// Witness data for a single transaction
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TransactionWitness {
    /// Account proof (before)
    pub account_proof_before: Option<AccountWitness>,
    /// Account proof (after)
    pub account_proof_after: Option<AccountWitness>,
    /// Order proofs for matching
    pub order_proofs: Vec<OrderWitness>,
    /// Position proof (before)
    pub position_proof_before: Option<PositionWitness>,
    /// Position proof (after)
    pub position_proof_after: Option<PositionWitness>,
}

/// Witness for an account
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountWitness {
    pub account: Account,
    pub proof: MerkleProof,
}

/// Witness for an order
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderWitness {
    pub order: Order,
    pub proof: MerkleProof,
}

/// Witness for a position
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PositionWitness {
    pub position: Position,
    pub proof: MerkleProof,
}

/// Public output committed to the journal
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BatchOutput {
    /// Pre-state root (verified)
    pub pre_state_root: Hash,
    /// Post-state root (verified)
    pub post_state_root: Hash,
    /// Hash of all transactions
    pub batch_hash: Hash,
    /// Number of transactions processed
    pub tx_count: u32,
}
