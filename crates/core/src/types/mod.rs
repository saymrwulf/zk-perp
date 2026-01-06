//! Core data types for zk-perp

pub mod order;
pub mod account;
pub mod market;
pub mod position;

pub use order::*;
pub use account::*;
pub use market::*;
pub use position::*;

/// Type aliases for clarity
pub type OrderId = u64;
pub type AccountId = u64;
pub type AssetId = u16;
pub type MarketId = u32;
pub type PoolId = u32;

/// Price represented as fixed-point u64 (8 decimal places)
/// Example: 50000.12345678 BTC price = 5_000_012_345_678
pub type Price = u64;

/// Quantity represented as u128 for large position sizes
/// 18 decimal places for precision
pub type Quantity = u128;

/// Timestamp in milliseconds since Unix epoch
pub type Timestamp = u64;

// Note: Hash type is defined in merkle module to avoid duplication

/// Public key for account ownership
pub type PublicKey = [u8; 32];

/// Signature (Ed25519) - using Vec for serde compatibility
pub type Signature = Vec<u8>;
