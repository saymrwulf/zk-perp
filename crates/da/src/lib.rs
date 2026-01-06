//! Data Availability layer for zk-perp
//!
//! This module provides an append-only log for storing transaction batches and their ZK proofs.
//! The DA layer is the source of truth for all state transitions and enables:
//! - State reconstruction from any point in history
//! - Proof verification for any batch
//! - Complete audit trail of all transactions
//!
//! ## Storage Format
//!
//! Data is stored in a directory structure:
//! ```text
//! da/
//! ├── batches/
//! │   ├── 000000.batch    # First batch
//! │   ├── 000001.batch    # Second batch
//! │   └── ...
//! ├── proofs/
//! │   ├── 000000.proof    # Proof for first batch
//! │   └── ...
//! └── index.json          # Metadata and batch index
//! ```

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use zk_perp_core::{transactions::Transaction, merkle::Hash};
use serde::{Serialize, Deserialize};
use thiserror::Error;

/// DA layer errors
#[derive(Error, Debug)]
pub enum DaError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    SerializationError(String),
    #[error("Batch not found: {0}")]
    BatchNotFound(u64),
    #[error("Proof not found for batch: {0}")]
    ProofNotFound(u64),
    #[error("Invalid batch sequence: expected {expected}, got {got}")]
    InvalidBatchSequence { expected: u64, got: u64 },
    #[error("State root mismatch: expected {expected:?}, got {got:?}")]
    StateRootMismatch { expected: Hash, got: Hash },
}

/// A batch of transactions with metadata
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Batch {
    /// Unique batch identifier
    pub id: u64,
    /// Transactions in this batch
    pub transactions: Vec<Transaction>,
    /// State root before batch execution
    pub pre_state_root: Hash,
    /// State root after batch execution
    pub post_state_root: Hash,
    /// Unix timestamp when batch was created
    pub timestamp: u64,
    /// Hash of all transactions in the batch
    pub batch_hash: Hash,
    /// Number of transactions
    pub tx_count: u32,
}

/// Proof receipt stored in DA
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredProof {
    /// The batch this proof is for
    pub batch_id: u64,
    /// Serialized RISC Zero receipt
    pub receipt_bytes: Vec<u8>,
    /// Image ID of the guest program used
    pub image_id: [u32; 8],
    /// Timestamp when proof was generated
    pub proof_timestamp: u64,
}

/// Index file structure
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DaIndex {
    /// Last batch ID
    pub last_batch_id: u64,
    /// Current state root (from last batch)
    pub current_state_root: Hash,
    /// Total number of transactions processed
    pub total_transactions: u64,
    /// Creation timestamp
    pub created_at: u64,
    /// Last update timestamp
    pub updated_at: u64,
    /// Batch IDs that have proofs
    pub proven_batches: Vec<u64>,
}

impl Default for DaIndex {
    fn default() -> Self {
        Self {
            last_batch_id: 0,
            current_state_root: [0u8; 32],
            total_transactions: 0,
            created_at: current_timestamp(),
            updated_at: current_timestamp(),
            proven_batches: Vec::new(),
        }
    }
}

/// File-based append-only log for data availability
pub struct AppendLog {
    /// Base directory for DA storage
    base_path: PathBuf,
    /// In-memory cache of batches (LRU-style, most recent)
    batch_cache: HashMap<u64, Batch>,
    /// Index metadata
    index: DaIndex,
    /// Maximum batches to cache in memory
    cache_size: usize,
}

impl AppendLog {
    /// Create a new append log at the given path
    pub fn new<P: AsRef<Path>>(base_path: P) -> Result<Self, DaError> {
        let base_path = base_path.as_ref().to_path_buf();

        // Create directory structure
        fs::create_dir_all(base_path.join("batches"))?;
        fs::create_dir_all(base_path.join("proofs"))?;

        // Load or create index
        let index_path = base_path.join("index.json");
        let index = if index_path.exists() {
            let file = File::open(&index_path)?;
            serde_json::from_reader(BufReader::new(file))
                .map_err(|e| DaError::SerializationError(e.to_string()))?
        } else {
            let index = DaIndex::default();
            Self::save_index_to_path(&index_path, &index)?;
            index
        };

        Ok(Self {
            base_path,
            batch_cache: HashMap::new(),
            index,
            cache_size: 100,
        })
    }

    /// Create an in-memory only append log (for testing)
    pub fn in_memory() -> Self {
        Self {
            base_path: PathBuf::from("/dev/null"),
            batch_cache: HashMap::new(),
            index: DaIndex::default(),
            cache_size: 1000,
        }
    }

    /// Get the current state root
    pub fn current_state_root(&self) -> Hash {
        self.index.current_state_root
    }

    /// Get the last batch ID
    pub fn last_batch_id(&self) -> u64 {
        self.index.last_batch_id
    }

    /// Get total number of transactions
    pub fn total_transactions(&self) -> u64 {
        self.index.total_transactions
    }

    /// Append a batch of transactions
    pub fn append_batch(
        &mut self,
        transactions: Vec<Transaction>,
        pre_root: Hash,
        post_root: Hash,
        batch_hash: Hash,
    ) -> Result<u64, DaError> {
        // Validate state continuity (except for first batch)
        if self.index.last_batch_id > 0 {
            if pre_root != self.index.current_state_root {
                return Err(DaError::StateRootMismatch {
                    expected: self.index.current_state_root,
                    got: pre_root,
                });
            }
        }

        let batch_id = if self.index.last_batch_id == 0 && self.index.total_transactions == 0 {
            1 // First batch starts at 1
        } else {
            self.index.last_batch_id + 1
        };

        let batch = Batch {
            id: batch_id,
            tx_count: transactions.len() as u32,
            transactions,
            pre_state_root: pre_root,
            post_state_root: post_root,
            timestamp: current_timestamp(),
            batch_hash,
        };

        // Save to disk (if not in-memory mode)
        if self.base_path.to_str() != Some("/dev/null") {
            self.save_batch(&batch)?;
        }

        // Update cache
        self.batch_cache.insert(batch_id, batch.clone());
        self.trim_cache();

        // Update index
        self.index.last_batch_id = batch_id;
        self.index.current_state_root = post_root;
        self.index.total_transactions += batch.tx_count as u64;
        self.index.updated_at = current_timestamp();
        self.save_index()?;

        Ok(batch_id)
    }

    /// Append proof for a batch
    pub fn append_proof(
        &mut self,
        batch_id: u64,
        receipt_bytes: Vec<u8>,
        image_id: [u32; 8],
    ) -> Result<(), DaError> {
        // Verify batch exists
        if batch_id > self.index.last_batch_id {
            return Err(DaError::BatchNotFound(batch_id));
        }

        let proof = StoredProof {
            batch_id,
            receipt_bytes,
            image_id,
            proof_timestamp: current_timestamp(),
        };

        // Save to disk (if not in-memory mode)
        if self.base_path.to_str() != Some("/dev/null") {
            self.save_proof(&proof)?;
        }

        // Update index
        if !self.index.proven_batches.contains(&batch_id) {
            self.index.proven_batches.push(batch_id);
            self.index.proven_batches.sort();
            self.index.updated_at = current_timestamp();
            self.save_index()?;
        }

        Ok(())
    }

    /// Get batch by ID
    pub fn get_batch(&mut self, id: u64) -> Result<Batch, DaError> {
        // Check cache first
        if let Some(batch) = self.batch_cache.get(&id) {
            return Ok(batch.clone());
        }

        // Load from disk
        let batch = self.load_batch(id)?;

        // Add to cache
        self.batch_cache.insert(id, batch.clone());
        self.trim_cache();

        Ok(batch)
    }

    /// Get proof by batch ID
    pub fn get_proof(&self, batch_id: u64) -> Result<StoredProof, DaError> {
        self.load_proof(batch_id)
    }

    /// Check if a batch has been proven
    pub fn is_proven(&self, batch_id: u64) -> bool {
        self.index.proven_batches.contains(&batch_id)
    }

    /// Get all batch IDs in range
    pub fn get_batch_ids(&self, start: u64, end: u64) -> Vec<u64> {
        let start = start.max(1);
        let end = end.min(self.index.last_batch_id);
        (start..=end).collect()
    }

    /// Reconstruct state by replaying batches
    /// Returns the transactions from all batches in order
    pub fn get_all_transactions(&mut self, up_to_batch: Option<u64>) -> Result<Vec<Transaction>, DaError> {
        let end_batch = up_to_batch.unwrap_or(self.index.last_batch_id);
        let mut all_txs = Vec::new();

        for batch_id in 1..=end_batch {
            let batch = self.get_batch(batch_id)?;
            all_txs.extend(batch.transactions);
        }

        Ok(all_txs)
    }

    /// Get batch statistics
    pub fn stats(&self) -> DaStats {
        DaStats {
            total_batches: self.index.last_batch_id,
            total_transactions: self.index.total_transactions,
            proven_batches: self.index.proven_batches.len() as u64,
            current_state_root: self.index.current_state_root,
            created_at: self.index.created_at,
            updated_at: self.index.updated_at,
        }
    }

    // --- Private helpers ---

    fn batch_path(&self, id: u64) -> PathBuf {
        self.base_path.join("batches").join(format!("{:06}.batch", id))
    }

    fn proof_path(&self, id: u64) -> PathBuf {
        self.base_path.join("proofs").join(format!("{:06}.proof", id))
    }

    fn index_path(&self) -> PathBuf {
        self.base_path.join("index.json")
    }

    fn save_batch(&self, batch: &Batch) -> Result<(), DaError> {
        let path = self.batch_path(batch.id);
        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        bincode::serialize_into(writer, batch)
            .map_err(|e| DaError::SerializationError(e.to_string()))?;
        Ok(())
    }

    fn load_batch(&self, id: u64) -> Result<Batch, DaError> {
        let path = self.batch_path(id);
        if !path.exists() {
            return Err(DaError::BatchNotFound(id));
        }
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        bincode::deserialize_from(reader)
            .map_err(|e| DaError::SerializationError(e.to_string()))
    }

    fn save_proof(&self, proof: &StoredProof) -> Result<(), DaError> {
        let path = self.proof_path(proof.batch_id);
        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        bincode::serialize_into(writer, proof)
            .map_err(|e| DaError::SerializationError(e.to_string()))?;
        Ok(())
    }

    fn load_proof(&self, id: u64) -> Result<StoredProof, DaError> {
        let path = self.proof_path(id);
        if !path.exists() {
            return Err(DaError::ProofNotFound(id));
        }
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        bincode::deserialize_from(reader)
            .map_err(|e| DaError::SerializationError(e.to_string()))
    }

    fn save_index(&self) -> Result<(), DaError> {
        if self.base_path.to_str() == Some("/dev/null") {
            return Ok(());
        }
        Self::save_index_to_path(&self.index_path(), &self.index)
    }

    fn save_index_to_path(path: &Path, index: &DaIndex) -> Result<(), DaError> {
        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, index)
            .map_err(|e| DaError::SerializationError(e.to_string()))?;
        Ok(())
    }

    fn trim_cache(&mut self) {
        while self.batch_cache.len() > self.cache_size {
            // Remove oldest batch (lowest ID) from cache
            if let Some(&oldest_id) = self.batch_cache.keys().min() {
                self.batch_cache.remove(&oldest_id);
            }
        }
    }
}

/// Statistics about the DA layer
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DaStats {
    pub total_batches: u64,
    pub total_transactions: u64,
    pub proven_batches: u64,
    pub current_state_root: Hash,
    pub created_at: u64,
    pub updated_at: u64,
}

/// Get current Unix timestamp in milliseconds
fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// Import serde_json for index file
use serde_json;

#[cfg(test)]
mod tests {
    use super::*;
    use zk_perp_core::transactions::DepositTx;

    fn test_transaction() -> Transaction {
        Transaction::Deposit(DepositTx {
            account_id: 1,
            asset_id: 0,
            amount: 1_000_000_000_000_000_000,
            nonce: 1,
        })
    }

    #[test]
    fn test_in_memory_append_log() {
        let mut log = AppendLog::in_memory();

        let pre_root = [0u8; 32];
        let post_root = [1u8; 32];
        let batch_hash = [2u8; 32];

        let batch_id = log.append_batch(
            vec![test_transaction()],
            pre_root,
            post_root,
            batch_hash,
        ).expect("Should append batch");

        assert_eq!(batch_id, 1);
        assert_eq!(log.last_batch_id(), 1);
        assert_eq!(log.current_state_root(), post_root);

        let batch = log.get_batch(1).expect("Should get batch");
        assert_eq!(batch.id, 1);
        assert_eq!(batch.tx_count, 1);
        assert_eq!(batch.pre_state_root, pre_root);
        assert_eq!(batch.post_state_root, post_root);
    }

    #[test]
    fn test_state_continuity() {
        let mut log = AppendLog::in_memory();

        let root1 = [1u8; 32];
        let root2 = [2u8; 32];
        let root3 = [3u8; 32];
        let batch_hash = [0u8; 32];

        // First batch
        log.append_batch(vec![test_transaction()], [0u8; 32], root1, batch_hash)
            .expect("Should append first batch");

        // Second batch must continue from first's post_root
        log.append_batch(vec![test_transaction()], root1, root2, batch_hash)
            .expect("Should append second batch");

        // Invalid batch (wrong pre_root)
        let result = log.append_batch(vec![test_transaction()], [99u8; 32], root3, batch_hash);
        assert!(matches!(result, Err(DaError::StateRootMismatch { .. })));

        // Valid third batch
        log.append_batch(vec![test_transaction()], root2, root3, batch_hash)
            .expect("Should append third batch");

        assert_eq!(log.last_batch_id(), 3);
        assert_eq!(log.current_state_root(), root3);
    }

    #[test]
    fn test_proof_storage() {
        let mut log = AppendLog::in_memory();

        let batch_id = log.append_batch(
            vec![test_transaction()],
            [0u8; 32],
            [1u8; 32],
            [2u8; 32],
        ).expect("Should append batch");

        assert!(!log.is_proven(batch_id));

        let proof_bytes = vec![1, 2, 3, 4, 5];
        let image_id = [0u32; 8];

        log.append_proof(batch_id, proof_bytes.clone(), image_id)
            .expect("Should append proof");

        assert!(log.is_proven(batch_id));
    }

    #[test]
    fn test_get_all_transactions() {
        let mut log = AppendLog::in_memory();
        let batch_hash = [0u8; 32];

        // Create 3 batches with 2 transactions each
        for i in 0..3 {
            let pre = [i as u8; 32];
            let post = [(i + 1) as u8; 32];
            log.append_batch(
                vec![test_transaction(), test_transaction()],
                pre,
                post,
                batch_hash,
            ).expect("Should append batch");
        }

        let all_txs = log.get_all_transactions(None).expect("Should get all transactions");
        assert_eq!(all_txs.len(), 6);

        let first_two_batches = log.get_all_transactions(Some(2)).expect("Should get transactions");
        assert_eq!(first_two_batches.len(), 4);
    }

    #[test]
    fn test_stats() {
        let mut log = AppendLog::in_memory();
        let batch_hash = [0u8; 32];

        log.append_batch(vec![test_transaction(), test_transaction()], [0u8; 32], [1u8; 32], batch_hash)
            .expect("Should append batch");

        let stats = log.stats();
        assert_eq!(stats.total_batches, 1);
        assert_eq!(stats.total_transactions, 2);
        assert_eq!(stats.proven_batches, 0);
    }
}
