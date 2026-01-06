//! Host/Prover for zk-perp
//!
//! This crate handles proof generation using RISC Zero zkVM.
//! It takes a batch of transactions with their witnesses and generates
//! a STARK proof that the state transitions are valid.

use serde::{Serialize, Deserialize};
use thiserror::Error;

// Re-export batch types from core (shared with guest)
pub use zk_perp_core::batch::{
    BatchInput, BatchOutput, TransactionWitness,
    AccountWitness, OrderWitness, PositionWitness,
};
use zk_perp_core::merkle::Hash;

#[cfg(feature = "risc0")]
use risc0_zkvm::{default_prover, ExecutorEnv};

/// Errors from the prover
#[derive(Error, Debug)]
pub enum ProverError {
    #[error("Failed to serialize input: {0}")]
    SerializationError(String),
    #[error("Failed to execute guest: {0}")]
    ExecutionError(String),
    #[error("Failed to generate proof: {0}")]
    ProvingError(String),
    #[error("Invalid output from guest: {0}")]
    InvalidOutput(String),
}

/// RISC Zero receipt wrapper
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProofReceipt {
    /// The journal (public output)
    pub journal: Vec<u8>,
    /// The seal (proof data)
    pub seal: Vec<u8>,
    /// Parsed output for convenience
    pub output: BatchOutput,
}

/// The prover for generating ZK proofs
pub struct Prover {
    /// Whether to use mock proving (for development)
    use_mock: bool,
}

impl Prover {
    /// Create a new prover
    pub fn new() -> Self {
        Self {
            use_mock: cfg!(feature = "mock"),
        }
    }

    /// Create a mock prover (for development/testing)
    pub fn mock() -> Self {
        Self { use_mock: true }
    }

    /// Generate a ZK proof for a batch of transactions
    pub fn prove(&self, input: BatchInput) -> Result<ProofReceipt, ProverError> {
        if self.use_mock {
            return self.prove_mock(input);
        }

        #[cfg(feature = "risc0")]
        {
            self.prove_real(input)
        }

        #[cfg(not(feature = "risc0"))]
        {
            self.prove_mock(input)
        }
    }

    /// Mock proving (no actual ZK proof)
    fn prove_mock(&self, input: BatchInput) -> Result<ProofReceipt, ProverError> {
        use zk_perp_core::merkle::{Sha256Hasher, ZERO_HASH};

        // Compute batch hash manually
        let hasher = Sha256Hasher::new();
        let mut batch_hash = ZERO_HASH;
        for tx in &input.transactions {
            let tx_bytes = bincode::serialize(tx)
                .map_err(|e| ProverError::SerializationError(e.to_string()))?;
            let tx_hash = hasher.hash(&tx_bytes);
            batch_hash = hasher.hash_pair(&batch_hash, &tx_hash);
        }

        let output = BatchOutput {
            pre_state_root: input.pre_state_root,
            post_state_root: input.post_state_root,
            batch_hash,
            tx_count: input.transactions.len() as u32,
        };

        let journal = bincode::serialize(&output)
            .map_err(|e| ProverError::SerializationError(e.to_string()))?;

        Ok(ProofReceipt {
            journal,
            seal: vec![], // Empty seal for mock
            output,
        })
    }

    /// Real RISC Zero proving
    #[cfg(feature = "risc0")]
    fn prove_real(&self, input: BatchInput) -> Result<ProofReceipt, ProverError> {
        use zk_perp_methods::ZK_PERP_GUEST_ELF;

        // Create executor environment with input (uses RISC Zero's built-in serialization)
        let env = ExecutorEnv::builder()
            .write(&input)
            .map_err(|e| ProverError::SerializationError(e.to_string()))?
            .build()
            .map_err(|e| ProverError::ExecutionError(e.to_string()))?;

        // Get the default prover
        let prover = default_prover();

        // Generate the proof
        let receipt = prover
            .prove(env, ZK_PERP_GUEST_ELF)
            .map_err(|e| ProverError::ProvingError(e.to_string()))?
            .receipt;

        // Decode the journal output using RISC Zero's serialization
        let output: BatchOutput = receipt.journal.decode()
            .map_err(|e| ProverError::InvalidOutput(e.to_string()))?;

        Ok(ProofReceipt {
            journal: receipt.journal.bytes.to_vec(),
            seal: bincode::serialize(&receipt)
                .map_err(|e| ProverError::SerializationError(e.to_string()))?,
            output,
        })
    }

    /// Verify a proof receipt
    pub fn verify(&self, receipt: &ProofReceipt) -> Result<bool, ProverError> {
        if self.use_mock {
            // Mock verification always passes
            return Ok(true);
        }

        #[cfg(feature = "risc0")]
        {
            use zk_perp_methods::ZK_PERP_GUEST_ID;

            // Deserialize the full receipt
            let risc0_receipt: risc0_zkvm::Receipt = bincode::deserialize(&receipt.seal)
                .map_err(|e| ProverError::InvalidOutput(e.to_string()))?;

            // Verify against the guest image ID
            risc0_receipt
                .verify(ZK_PERP_GUEST_ID)
                .map_err(|e| ProverError::ProvingError(e.to_string()))?;

            Ok(true)
        }

        #[cfg(not(feature = "risc0"))]
        Ok(true)
    }
}

impl Default for Prover {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zk_perp_core::transactions::{DepositTx, Transaction};

    #[test]
    fn test_mock_prover() {
        let prover = Prover::mock();

        let input = BatchInput {
            pre_state_root: [0u8; 32],
            post_state_root: [1u8; 32],
            transactions: vec![Transaction::Deposit(DepositTx {
                account_id: 1,
                asset_id: 0,
                amount: 1000,
                nonce: 1,
            })],
            witnesses: vec![TransactionWitness::default()],
        };

        let receipt = prover.prove(input).expect("Mock proving should succeed");
        assert_eq!(receipt.output.tx_count, 1);
        assert!(prover.verify(&receipt).expect("Mock verification should succeed"));
    }
}
