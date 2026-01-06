//! zk-perp-core: Core types and primitives for the ZK perpetual DEX
//!
//! This crate provides:
//! - Core data types (Order, Position, Account, Market)
//! - Merkle tree implementations (Sparse Merkle Tree, Hypertree)
//! - Transaction types
//! - Batch types for ZK proving
//! - Cryptographic primitives (Ed25519 signatures)

pub mod types;
pub mod merkle;
pub mod transactions;
pub mod batch;
pub mod crypto;

// Re-export types (explicit to avoid ambiguity)
pub use types::{
    Order, OrderId, OrderType, Side, TimeInForce, Fill,
    Account, AccountId, AccountType, Balance, AssetId, assets,
    Market, MarketId, OraclePrice, SystemState, markets,
    Position, PositionSide,
    Price, Quantity, Timestamp, PublicKey, Signature,
};

// Re-export merkle types
pub use merkle::{
    Hash, ZERO_HASH, PoseidonHasher, PoseidonError, Sha256Hasher, MerkleHasher,
    SparseMerkleTree, MerkleProof, DEFAULT_TREE_DEPTH,
    Hypertree, TreeId, HypertreeWitness, system_keys,
};

// Re-export transaction types
pub use transactions::{
    Transaction, SignedTransaction, TransactionReceipt,
    DepositTx, WithdrawTx, PlaceOrderTx, CancelOrderTx, LiquidateTx, UpdateOracleTx,
};

// Re-export batch types (shared between host and guest)
pub use batch::{
    BatchInput, BatchOutput, TransactionWitness,
    AccountWitness, OrderWitness, PositionWitness,
};

// Re-export crypto types
pub use crypto::{
    Keypair, CryptoError,
    verify_signature, verify_transaction, hash_transaction,
    to_hex, from_hex,
};
