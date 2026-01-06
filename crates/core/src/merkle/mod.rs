//! Merkle tree implementations for zk-perp
//!
//! This module provides:
//! - Poseidon hash function wrapper (ZK-friendly)
//! - Sparse Merkle Tree implementation
//! - Hypertree: 6-tree composition for full protocol state

pub mod poseidon;
pub mod tree;
pub mod hypertree;

pub use poseidon::*;
pub use tree::*;
pub use hypertree::*;
