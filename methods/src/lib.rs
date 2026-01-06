//! RISC Zero methods for zk-perp
//!
//! This crate contains the compiled zkVM guest program ELF and its image ID.
//! The guest program verifies:
//! - State transitions
//! - Order matching (price-time priority)
//! - Margin requirements
//! - Liquidations
//!
//! ## Building with RISC Zero (requires Rust 1.83+)
//!
//! To enable actual ZK proving:
//! 1. Update Rust to 1.83+: `rustup update`
//! 2. Install RISC Zero: `cargo install cargo-risczero`
//! 3. Build with feature: `cargo build --features risc0`
//!
//! The build.rs script uses risc0-build to compile the guest code
//! into the GUEST_ELF binary and compute its GUEST_ID.

// When built with risc0 feature, this includes the generated methods.rs
// which defines ZK_PERP_GUEST_ELF and ZK_PERP_GUEST_ID
#[cfg(feature = "risc0")]
include!(concat!(env!("OUT_DIR"), "/methods.rs"));

// Mock stubs when risc0 feature is not enabled
#[cfg(not(feature = "risc0"))]
pub const ZK_PERP_GUEST_ELF: &[u8] = &[];

#[cfg(not(feature = "risc0"))]
pub const ZK_PERP_GUEST_ID: [u32; 8] = [0; 8];
