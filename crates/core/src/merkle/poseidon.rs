//! Poseidon hash function wrapper
//!
//! Poseidon is a ZK-friendly hash function that is significantly more efficient
//! in zero-knowledge circuits compared to SHA-256 or Keccak.
//!
//! This implementation wraps the light-poseidon crate for use with our Merkle trees.

use light_poseidon::{Poseidon, PoseidonBytesHasher};
use thiserror::Error;

/// Hash output: 32 bytes
pub type Hash = [u8; 32];

/// Zero hash (hash of empty data)
pub const ZERO_HASH: Hash = [0u8; 32];

#[derive(Error, Debug)]
pub enum PoseidonError {
    #[error("Invalid input length")]
    InvalidInputLength,
    #[error("Hash computation failed: {0}")]
    HashFailed(String),
}

/// Poseidon hasher for Merkle tree operations
#[derive(Clone)]
pub struct PoseidonHasher {
    // We use the BN254 curve parameters which are common in ZK applications
}

impl Default for PoseidonHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl PoseidonHasher {
    /// Create a new Poseidon hasher
    pub fn new() -> Self {
        Self {}
    }

    /// Hash a single input (32 bytes)
    pub fn hash(&self, input: &[u8]) -> Result<Hash, PoseidonError> {
        // Pad or truncate input to 32 bytes
        let mut padded = [0u8; 32];
        let len = input.len().min(32);
        padded[..len].copy_from_slice(&input[..len]);

        // Use light-poseidon to hash
        let mut hasher = Poseidon::<ark_bn254::Fr>::new_circom(2)
            .map_err(|e| PoseidonError::HashFailed(e.to_string()))?;

        // Convert bytes to field elements and hash
        let result = hasher.hash_bytes_be(&[&padded, &[0u8; 32]])
            .map_err(|e| PoseidonError::HashFailed(e.to_string()))?;

        Ok(result)
    }

    /// Hash two 32-byte inputs (for Merkle tree internal nodes)
    pub fn hash_pair(&self, left: &Hash, right: &Hash) -> Result<Hash, PoseidonError> {
        let mut hasher = Poseidon::<ark_bn254::Fr>::new_circom(2)
            .map_err(|e| PoseidonError::HashFailed(e.to_string()))?;

        let result = hasher.hash_bytes_be(&[left.as_slice(), right.as_slice()])
            .map_err(|e| PoseidonError::HashFailed(e.to_string()))?;

        Ok(result)
    }

    /// Hash multiple inputs (variable arity)
    pub fn hash_many(&self, inputs: &[&[u8]]) -> Result<Hash, PoseidonError> {
        if inputs.is_empty() {
            return Ok(ZERO_HASH);
        }

        if inputs.len() == 1 {
            return self.hash(inputs[0]);
        }

        // For multiple inputs, hash pairs recursively
        let mut current: Vec<Hash> = inputs
            .iter()
            .map(|inp| {
                let mut h = [0u8; 32];
                let len = inp.len().min(32);
                h[..len].copy_from_slice(&inp[..len]);
                h
            })
            .collect();

        while current.len() > 1 {
            let mut next = Vec::new();
            for chunk in current.chunks(2) {
                if chunk.len() == 2 {
                    next.push(self.hash_pair(&chunk[0], &chunk[1])?);
                } else {
                    next.push(chunk[0]);
                }
            }
            current = next;
        }

        Ok(current[0])
    }

    /// Compute hash of serializable data using bincode
    pub fn hash_serializable<T: serde::Serialize>(&self, data: &T) -> Result<Hash, PoseidonError> {
        let bytes = bincode::serialize(data)
            .map_err(|e| PoseidonError::HashFailed(e.to_string()))?;
        self.hash(&bytes)
    }
}

/// Simple SHA256 hasher as fallback (for testing without full Poseidon setup)
#[derive(Clone, Default)]
pub struct Sha256Hasher;

impl Sha256Hasher {
    pub fn new() -> Self {
        Self
    }

    pub fn hash(&self, input: &[u8]) -> Hash {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(input);
        let result = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        hash
    }

    pub fn hash_pair(&self, left: &Hash, right: &Hash) -> Hash {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(left);
        hasher.update(right);
        let result = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        hash
    }

    pub fn hash_serializable<T: serde::Serialize>(&self, data: &T) -> Hash {
        let bytes = bincode::serialize(data).unwrap_or_default();
        self.hash(&bytes)
    }
}

/// Trait for hash functions usable in Merkle trees
pub trait MerkleHasher {
    fn hash(&self, input: &[u8]) -> Hash;
    fn hash_pair(&self, left: &Hash, right: &Hash) -> Hash;
}

impl MerkleHasher for Sha256Hasher {
    fn hash(&self, input: &[u8]) -> Hash {
        Sha256Hasher::hash(self, input)
    }

    fn hash_pair(&self, left: &Hash, right: &Hash) -> Hash {
        Sha256Hasher::hash_pair(self, left, right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_hash() {
        let hasher = Sha256Hasher::new();
        let hash1 = hasher.hash(b"hello");
        let hash2 = hasher.hash(b"hello");
        let hash3 = hasher.hash(b"world");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_sha256_hash_pair() {
        let hasher = Sha256Hasher::new();
        let left = hasher.hash(b"left");
        let right = hasher.hash(b"right");

        let combined1 = hasher.hash_pair(&left, &right);
        let combined2 = hasher.hash_pair(&left, &right);
        let combined3 = hasher.hash_pair(&right, &left);

        assert_eq!(combined1, combined2);
        assert_ne!(combined1, combined3); // Order matters
    }

    #[test]
    fn test_sha256_serializable() {
        let hasher = Sha256Hasher::new();

        #[derive(serde::Serialize)]
        struct TestData {
            value: u64,
        }

        let data1 = TestData { value: 42 };
        let data2 = TestData { value: 42 };
        let data3 = TestData { value: 43 };

        let hash1 = hasher.hash_serializable(&data1);
        let hash2 = hasher.hash_serializable(&data2);
        let hash3 = hasher.hash_serializable(&data3);

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }
}
