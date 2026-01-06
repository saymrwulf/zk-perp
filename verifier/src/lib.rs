//! Verifier node for zk-perp
//!
//! The verifier is a standalone component that:
//! - Reads batches and proofs from the DA layer
//! - Verifies ZK proofs against the guest image ID
//! - Maintains verified state (tracks which batches have been verified)
//! - Provides an HTTP API for verification status queries
//!
//! In a production system, anyone can run a verifier to independently
//! verify the integrity of the entire state history.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use zk_perp_core::merkle::Hash;
use zk_perp_da::{AppendLog, Batch, StoredProof, DaError};
use zk_perp_host::{Prover, ProofReceipt, ProverError};

/// Verifier errors
#[derive(Error, Debug)]
pub enum VerifierError {
    #[error("DA error: {0}")]
    DaError(#[from] DaError),
    #[error("Prover error: {0}")]
    ProverError(#[from] ProverError),
    #[error("Batch not found: {0}")]
    BatchNotFound(u64),
    #[error("Proof not found for batch: {0}")]
    ProofNotFound(u64),
    #[error("State root mismatch at batch {batch_id}: expected {expected:?}, got {got:?}")]
    StateRootMismatch {
        batch_id: u64,
        expected: Hash,
        got: Hash,
    },
    #[error("Proof verification failed for batch {0}")]
    ProofVerificationFailed(u64),
    #[error("Receipt data mismatch for batch {batch_id}: {details}")]
    ReceiptMismatch {
        batch_id: u64,
        details: String,
    },
    #[error("Serialization error: {0}")]
    SerializationError(String),
}

/// Verification status for a single batch
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BatchVerification {
    pub batch_id: u64,
    pub verified: bool,
    pub pre_state_root: Hash,
    pub post_state_root: Hash,
    pub tx_count: u32,
    pub verified_at: Option<u64>,
    pub error: Option<String>,
}

/// Statistics about verification progress
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerifierStats {
    /// Total batches in DA
    pub total_batches: u64,
    /// Batches that have been verified
    pub verified_batches: u64,
    /// Current verified state root
    pub verified_state_root: Hash,
    /// Last verified batch ID
    pub last_verified_batch: u64,
    /// Batches with proof but not yet verified
    pub pending_verification: u64,
    /// Verification mode (mock or real)
    pub use_mock_verifier: bool,
}

/// Configuration for the verifier
#[derive(Clone, Debug)]
pub struct VerifierConfig {
    /// Data directory where DA stores batches/proofs
    pub data_dir: String,
    /// Use mock verification (for development)
    pub use_mock_verifier: bool,
    /// HTTP port for verifier API
    pub port: u16,
}

impl Default for VerifierConfig {
    fn default() -> Self {
        Self {
            data_dir: "./data".to_string(),
            use_mock_verifier: true,
            port: 8081,
        }
    }
}

/// The verifier state
pub struct Verifier {
    /// Configuration
    pub config: VerifierConfig,
    /// Current verified state root
    verified_root: Hash,
    /// Last verified batch ID
    last_verified_batch: u64,
    /// Set of verified batch IDs
    verified_batches: HashSet<u64>,
    /// Prover (for verification)
    prover: Prover,
    /// DA layer connection
    da: Option<AppendLog>,
}

impl Verifier {
    /// Create a new verifier
    pub fn new(config: VerifierConfig) -> Self {
        let prover = if config.use_mock_verifier {
            Prover::mock()
        } else {
            Prover::new()
        };

        Self {
            config,
            verified_root: [0u8; 32],
            last_verified_batch: 0,
            verified_batches: HashSet::new(),
            prover,
            da: None,
        }
    }

    /// Create a verifier with mock mode for testing
    pub fn mock() -> Self {
        Self::new(VerifierConfig {
            use_mock_verifier: true,
            ..Default::default()
        })
    }

    /// Connect to the DA layer
    pub fn connect_da<P: AsRef<Path>>(&mut self, path: P) -> Result<(), VerifierError> {
        let da = AppendLog::new(path)?;
        self.da = Some(da);
        Ok(())
    }

    /// Set DA layer directly (for testing)
    pub fn set_da(&mut self, da: AppendLog) {
        self.da = Some(da);
    }

    /// Get the current verified state root
    pub fn verified_root(&self) -> Hash {
        self.verified_root
    }

    /// Get the last verified batch ID
    pub fn last_verified_batch(&self) -> u64 {
        self.last_verified_batch
    }

    /// Check if a batch has been verified
    pub fn is_verified(&self, batch_id: u64) -> bool {
        self.verified_batches.contains(&batch_id)
    }

    /// Verify a single batch given the batch data and proof
    pub fn verify_batch_data(
        &mut self,
        batch: &Batch,
        proof: &StoredProof,
    ) -> Result<BatchVerification, VerifierError> {
        let batch_id = batch.id;

        // Check state continuity
        if self.last_verified_batch > 0 && batch.pre_state_root != self.verified_root {
            return Err(VerifierError::StateRootMismatch {
                batch_id,
                expected: self.verified_root,
                got: batch.pre_state_root,
            });
        }

        // Deserialize the receipt
        let receipt: ProofReceipt = bincode::deserialize(&proof.receipt_bytes)
            .map_err(|e| VerifierError::SerializationError(e.to_string()))?;

        // Verify the receipt's claimed roots match the batch
        if receipt.output.pre_state_root != batch.pre_state_root {
            return Err(VerifierError::ReceiptMismatch {
                batch_id,
                details: format!(
                    "Pre-state root: receipt {:?} != batch {:?}",
                    receipt.output.pre_state_root,
                    batch.pre_state_root
                ),
            });
        }

        if receipt.output.post_state_root != batch.post_state_root {
            return Err(VerifierError::ReceiptMismatch {
                batch_id,
                details: format!(
                    "Post-state root: receipt {:?} != batch {:?}",
                    receipt.output.post_state_root,
                    batch.post_state_root
                ),
            });
        }

        // Verify tx count matches
        if receipt.output.tx_count != batch.tx_count {
            return Err(VerifierError::ReceiptMismatch {
                batch_id,
                details: format!(
                    "TX count: receipt {} != batch {}",
                    receipt.output.tx_count,
                    batch.tx_count
                ),
            });
        }

        // Verify the ZK proof itself
        self.prover.verify(&receipt)
            .map_err(|_| VerifierError::ProofVerificationFailed(batch_id))?;

        // Update verified state
        self.verified_root = batch.post_state_root;
        self.last_verified_batch = batch_id;
        self.verified_batches.insert(batch_id);

        Ok(BatchVerification {
            batch_id,
            verified: true,
            pre_state_root: batch.pre_state_root,
            post_state_root: batch.post_state_root,
            tx_count: batch.tx_count,
            verified_at: Some(current_timestamp()),
            error: None,
        })
    }

    /// Verify the next unverified batch from DA
    pub fn verify_next(&mut self) -> Result<Option<BatchVerification>, VerifierError> {
        let da = self.da.as_mut()
            .ok_or(VerifierError::DaError(DaError::BatchNotFound(0)))?;

        let next_batch_id = self.last_verified_batch + 1;

        // Check if the batch exists
        if next_batch_id > da.last_batch_id() {
            return Ok(None);
        }

        // Get batch and proof
        let batch = da.get_batch(next_batch_id)?;

        // Check if proof exists
        if !da.is_proven(next_batch_id) {
            return Err(VerifierError::ProofNotFound(next_batch_id));
        }

        let proof = da.get_proof(next_batch_id)?;

        self.verify_batch_data(&batch, &proof).map(Some)
    }

    /// Verify all pending batches from DA
    pub fn verify_all_pending(&mut self) -> Result<Vec<BatchVerification>, VerifierError> {
        let mut results = Vec::new();

        loop {
            match self.verify_next() {
                Ok(Some(result)) => results.push(result),
                Ok(None) => break,
                Err(VerifierError::ProofNotFound(_)) => break, // Stop at unproven batch
                Err(e) => return Err(e),
            }
        }

        Ok(results)
    }

    /// Get verification statistics
    pub fn stats(&self) -> VerifierStats {
        let total_batches = self.da.as_ref()
            .map(|da| da.last_batch_id())
            .unwrap_or(0);

        let proven_batches = self.da.as_ref()
            .map(|da| da.stats().proven_batches)
            .unwrap_or(0);

        VerifierStats {
            total_batches,
            verified_batches: self.verified_batches.len() as u64,
            verified_state_root: self.verified_root,
            last_verified_batch: self.last_verified_batch,
            pending_verification: proven_batches.saturating_sub(self.verified_batches.len() as u64),
            use_mock_verifier: self.config.use_mock_verifier,
        }
    }

    /// Get verification status for a specific batch
    pub fn get_batch_status(&mut self, batch_id: u64) -> Result<BatchVerification, VerifierError> {
        let da = self.da.as_mut()
            .ok_or(VerifierError::DaError(DaError::BatchNotFound(batch_id)))?;

        let batch = da.get_batch(batch_id)?;

        Ok(BatchVerification {
            batch_id,
            verified: self.verified_batches.contains(&batch_id),
            pre_state_root: batch.pre_state_root,
            post_state_root: batch.post_state_root,
            tx_count: batch.tx_count,
            verified_at: if self.verified_batches.contains(&batch_id) {
                Some(batch.timestamp)
            } else {
                None
            },
            error: None,
        })
    }
}

/// Async wrapper for the verifier (for HTTP API)
pub struct AsyncVerifier {
    inner: Arc<RwLock<Verifier>>,
}

impl AsyncVerifier {
    pub fn new(verifier: Verifier) -> Self {
        Self {
            inner: Arc::new(RwLock::new(verifier)),
        }
    }

    pub async fn verify_next(&self) -> Result<Option<BatchVerification>, VerifierError> {
        self.inner.write().await.verify_next()
    }

    pub async fn verify_all_pending(&self) -> Result<Vec<BatchVerification>, VerifierError> {
        self.inner.write().await.verify_all_pending()
    }

    pub async fn stats(&self) -> VerifierStats {
        self.inner.read().await.stats()
    }

    pub async fn get_batch_status(&self, batch_id: u64) -> Result<BatchVerification, VerifierError> {
        self.inner.write().await.get_batch_status(batch_id)
    }

    pub async fn is_verified(&self, batch_id: u64) -> bool {
        self.inner.read().await.is_verified(batch_id)
    }

    pub async fn verified_root(&self) -> Hash {
        self.inner.read().await.verified_root()
    }

    pub async fn last_verified_batch(&self) -> u64 {
        self.inner.read().await.last_verified_batch()
    }

    pub fn inner(&self) -> Arc<RwLock<Verifier>> {
        self.inner.clone()
    }
}

/// Get current Unix timestamp in milliseconds
fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use zk_perp_core::transactions::{DepositTx, Transaction};
    use zk_perp_host::BatchInput;

    fn test_transaction() -> Transaction {
        Transaction::Deposit(DepositTx {
            account_id: 1,
            asset_id: 0,
            amount: 1_000_000_000_000_000_000,
            nonce: 1,
        })
    }

    #[test]
    fn test_verifier_creation() {
        let verifier = Verifier::mock();
        assert_eq!(verifier.last_verified_batch(), 0);
        assert_eq!(verifier.verified_root(), [0u8; 32]);
    }

    #[test]
    fn test_verify_batch_data() {
        let mut verifier = Verifier::mock();
        let prover = Prover::mock();

        // Create a batch
        let pre_root = [0u8; 32];
        let post_root = [1u8; 32];
        let batch = Batch {
            id: 1,
            transactions: vec![test_transaction()],
            pre_state_root: pre_root,
            post_state_root: post_root,
            timestamp: current_timestamp(),
            batch_hash: [2u8; 32],
            tx_count: 1,
        };

        // Create proof
        let input = BatchInput {
            pre_state_root: pre_root,
            post_state_root: post_root,
            transactions: vec![test_transaction()],
            witnesses: vec![],
        };
        let receipt = prover.prove(input).expect("Mock proving should succeed");
        let receipt_bytes = bincode::serialize(&receipt).expect("Should serialize");

        let stored_proof = StoredProof {
            batch_id: 1,
            receipt_bytes,
            image_id: [0u32; 8],
            proof_timestamp: current_timestamp(),
        };

        // Verify
        let result = verifier.verify_batch_data(&batch, &stored_proof)
            .expect("Verification should succeed");

        assert!(result.verified);
        assert_eq!(result.batch_id, 1);
        assert_eq!(verifier.verified_root(), post_root);
        assert_eq!(verifier.last_verified_batch(), 1);
    }

    #[test]
    fn test_state_continuity_check() {
        let mut verifier = Verifier::mock();
        let prover = Prover::mock();

        // First batch (succeeds)
        let batch1 = Batch {
            id: 1,
            transactions: vec![test_transaction()],
            pre_state_root: [0u8; 32],
            post_state_root: [1u8; 32],
            timestamp: current_timestamp(),
            batch_hash: [2u8; 32],
            tx_count: 1,
        };

        let input1 = BatchInput {
            pre_state_root: [0u8; 32],
            post_state_root: [1u8; 32],
            transactions: vec![test_transaction()],
            witnesses: vec![],
        };
        let receipt1 = prover.prove(input1).expect("Mock proving should succeed");
        let proof1 = StoredProof {
            batch_id: 1,
            receipt_bytes: bincode::serialize(&receipt1).unwrap(),
            image_id: [0u32; 8],
            proof_timestamp: current_timestamp(),
        };

        verifier.verify_batch_data(&batch1, &proof1).expect("Should verify batch 1");

        // Second batch with wrong pre_root (fails)
        let batch2 = Batch {
            id: 2,
            transactions: vec![test_transaction()],
            pre_state_root: [99u8; 32], // Wrong! Should be [1u8; 32]
            post_state_root: [2u8; 32],
            timestamp: current_timestamp(),
            batch_hash: [3u8; 32],
            tx_count: 1,
        };

        let input2 = BatchInput {
            pre_state_root: [99u8; 32],
            post_state_root: [2u8; 32],
            transactions: vec![test_transaction()],
            witnesses: vec![],
        };
        let receipt2 = prover.prove(input2).expect("Mock proving should succeed");
        let proof2 = StoredProof {
            batch_id: 2,
            receipt_bytes: bincode::serialize(&receipt2).unwrap(),
            image_id: [0u32; 8],
            proof_timestamp: current_timestamp(),
        };

        let result = verifier.verify_batch_data(&batch2, &proof2);
        assert!(matches!(result, Err(VerifierError::StateRootMismatch { .. })));
    }

    #[test]
    fn test_stats() {
        let verifier = Verifier::mock();
        let stats = verifier.stats();

        assert_eq!(stats.total_batches, 0);
        assert_eq!(stats.verified_batches, 0);
        assert!(stats.use_mock_verifier);
    }
}
