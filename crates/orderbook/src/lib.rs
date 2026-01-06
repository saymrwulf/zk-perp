//! Order book and matching engine for zk-perp
//!
//! This crate provides:
//! - BTreeMap-based order book with price-time priority
//! - Matching engine that produces fills
//! - Order management (add, cancel, modify)

pub mod book;
pub mod matching;

pub use book::*;
pub use matching::*;
