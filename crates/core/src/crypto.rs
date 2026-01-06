//! Cryptographic primitives for zk-perp
//!
//! This module provides Ed25519 digital signatures for transaction authentication.
//! Ed25519 is a high-speed, high-security signature scheme using elliptic curve cryptography.

use ed25519_dalek::{
    Signature as DalekSignature,
    Signer,
    SigningKey,
    Verifier,
    VerifyingKey,
    PUBLIC_KEY_LENGTH,
    SECRET_KEY_LENGTH,
    SIGNATURE_LENGTH,
};
use rand::RngCore;
use thiserror::Error;

use crate::types::PublicKey;
use crate::transactions::{Transaction, SignedTransaction};
use crate::merkle::Hash;

/// Cryptographic errors
#[derive(Error, Debug)]
pub enum CryptoError {
    #[error("Invalid signature length: expected {SIGNATURE_LENGTH}, got {0}")]
    InvalidSignatureLength(usize),
    #[error("Invalid public key length: expected {PUBLIC_KEY_LENGTH}, got {0}")]
    InvalidPublicKeyLength(usize),
    #[error("Invalid secret key length: expected {SECRET_KEY_LENGTH}, got {0}")]
    InvalidSecretKeyLength(usize),
    #[error("Signature verification failed")]
    VerificationFailed,
    #[error("Failed to parse signature: {0}")]
    SignatureParseError(String),
    #[error("Failed to parse public key: {0}")]
    PublicKeyParseError(String),
    #[error("Serialization error: {0}")]
    SerializationError(String),
}

/// A keypair for signing transactions
#[derive(Clone)]
pub struct Keypair {
    signing_key: SigningKey,
}

impl Keypair {
    /// Generate a new random keypair
    pub fn generate() -> Self {
        let mut secret_bytes = [0u8; SECRET_KEY_LENGTH];
        rand::thread_rng().fill_bytes(&mut secret_bytes);
        let signing_key = SigningKey::from_bytes(&secret_bytes);
        Self { signing_key }
    }

    /// Create a keypair from a 32-byte secret key
    pub fn from_secret_key(secret: &[u8; SECRET_KEY_LENGTH]) -> Self {
        let signing_key = SigningKey::from_bytes(secret);
        Self { signing_key }
    }

    /// Create a keypair from a hex-encoded secret key
    pub fn from_hex(hex: &str) -> Result<Self, CryptoError> {
        let bytes = hex::decode(hex.trim_start_matches("0x"))
            .map_err(|e| CryptoError::SerializationError(e.to_string()))?;

        if bytes.len() != SECRET_KEY_LENGTH {
            return Err(CryptoError::InvalidSecretKeyLength(bytes.len()));
        }

        let mut secret = [0u8; SECRET_KEY_LENGTH];
        secret.copy_from_slice(&bytes);
        Ok(Self::from_secret_key(&secret))
    }

    /// Get the public key
    pub fn public_key(&self) -> PublicKey {
        self.signing_key.verifying_key().to_bytes()
    }

    /// Get the secret key bytes
    pub fn secret_key(&self) -> [u8; SECRET_KEY_LENGTH] {
        self.signing_key.to_bytes()
    }

    /// Sign a message
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        let signature = self.signing_key.sign(message);
        signature.to_bytes().to_vec()
    }

    /// Sign a transaction
    pub fn sign_transaction(&self, tx: &Transaction) -> Result<SignedTransaction, CryptoError> {
        let tx_bytes = bincode::serialize(tx)
            .map_err(|e| CryptoError::SerializationError(e.to_string()))?;

        let signature = self.sign(&tx_bytes);

        Ok(SignedTransaction {
            tx: tx.clone(),
            signature,
        })
    }
}

impl std::fmt::Debug for Keypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keypair")
            .field("public_key", &hex::encode(&self.public_key()))
            .finish()
    }
}

/// Verify a signature against a message and public key
pub fn verify_signature(
    public_key: &PublicKey,
    message: &[u8],
    signature: &[u8],
) -> Result<bool, CryptoError> {
    if signature.len() != SIGNATURE_LENGTH {
        return Err(CryptoError::InvalidSignatureLength(signature.len()));
    }

    let verifying_key = VerifyingKey::from_bytes(public_key)
        .map_err(|e| CryptoError::PublicKeyParseError(e.to_string()))?;

    let mut sig_bytes = [0u8; SIGNATURE_LENGTH];
    sig_bytes.copy_from_slice(signature);
    let sig = DalekSignature::from_bytes(&sig_bytes);

    match verifying_key.verify(message, &sig) {
        Ok(()) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Verify a signed transaction
pub fn verify_transaction(
    signed_tx: &SignedTransaction,
    public_key: &PublicKey,
) -> Result<bool, CryptoError> {
    let tx_bytes = bincode::serialize(&signed_tx.tx)
        .map_err(|e| CryptoError::SerializationError(e.to_string()))?;

    verify_signature(public_key, &tx_bytes, &signed_tx.signature)
}

/// Hash a transaction to get its unique identifier
pub fn hash_transaction(tx: &Transaction) -> Hash {
    use sha2::{Sha256, Digest};

    let tx_bytes = bincode::serialize(tx).expect("Transaction serialization should not fail");
    let mut hasher = Sha256::new();
    hasher.update(&tx_bytes);
    let result = hasher.finalize();

    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    hash
}

/// Utility: Convert bytes to hex string
pub fn to_hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/// Utility: Convert hex string to bytes
pub fn from_hex(hex: &str) -> Result<Vec<u8>, CryptoError> {
    hex::decode(hex.trim_start_matches("0x"))
        .map_err(|e| CryptoError::SerializationError(e.to_string()))
}

// Re-export hex crate for convenience
mod hex {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

    pub fn encode(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            s.push(HEX_CHARS[(b >> 4) as usize] as char);
            s.push(HEX_CHARS[(b & 0xf) as usize] as char);
        }
        s
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, String> {
        if s.len() % 2 != 0 {
            return Err("Hex string must have even length".to_string());
        }

        let mut bytes = Vec::with_capacity(s.len() / 2);
        let chars: Vec<char> = s.chars().collect();

        for i in (0..s.len()).step_by(2) {
            let high = hex_char_to_nibble(chars[i])?;
            let low = hex_char_to_nibble(chars[i + 1])?;
            bytes.push((high << 4) | low);
        }

        Ok(bytes)
    }

    fn hex_char_to_nibble(c: char) -> Result<u8, String> {
        match c {
            '0'..='9' => Ok(c as u8 - b'0'),
            'a'..='f' => Ok(c as u8 - b'a' + 10),
            'A'..='F' => Ok(c as u8 - b'A' + 10),
            _ => Err(format!("Invalid hex character: {}", c)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transactions::DepositTx;

    #[test]
    fn test_keypair_generation() {
        let keypair = Keypair::generate();
        let public_key = keypair.public_key();

        assert_eq!(public_key.len(), 32);
        assert_ne!(public_key, [0u8; 32]); // Should not be all zeros
    }

    #[test]
    fn test_sign_and_verify() {
        let keypair = Keypair::generate();
        let message = b"Hello, world!";

        let signature = keypair.sign(message);

        assert_eq!(signature.len(), SIGNATURE_LENGTH);

        let is_valid = verify_signature(&keypair.public_key(), message, &signature)
            .expect("Verification should not error");

        assert!(is_valid);
    }

    #[test]
    fn test_verify_wrong_key() {
        let keypair1 = Keypair::generate();
        let keypair2 = Keypair::generate();
        let message = b"Hello, world!";

        let signature = keypair1.sign(message);

        // Verify with wrong public key should fail
        let is_valid = verify_signature(&keypair2.public_key(), message, &signature)
            .expect("Verification should not error");

        assert!(!is_valid);
    }

    #[test]
    fn test_verify_wrong_message() {
        let keypair = Keypair::generate();
        let message1 = b"Hello, world!";
        let message2 = b"Goodbye, world!";

        let signature = keypair.sign(message1);

        // Verify with wrong message should fail
        let is_valid = verify_signature(&keypair.public_key(), message2, &signature)
            .expect("Verification should not error");

        assert!(!is_valid);
    }

    #[test]
    fn test_sign_transaction() {
        let keypair = Keypair::generate();

        let tx = Transaction::Deposit(DepositTx {
            account_id: 1,
            asset_id: 0,
            amount: 1_000_000_000_000_000_000,
            nonce: 1,
        });

        let signed_tx = keypair.sign_transaction(&tx)
            .expect("Signing should succeed");

        let is_valid = verify_transaction(&signed_tx, &keypair.public_key())
            .expect("Verification should not error");

        assert!(is_valid);
    }

    #[test]
    fn test_transaction_hash() {
        let tx1 = Transaction::Deposit(DepositTx {
            account_id: 1,
            asset_id: 0,
            amount: 1000,
            nonce: 1,
        });

        let tx2 = Transaction::Deposit(DepositTx {
            account_id: 1,
            asset_id: 0,
            amount: 1000,
            nonce: 2, // Different nonce
        });

        let hash1 = hash_transaction(&tx1);
        let hash2 = hash_transaction(&tx2);

        assert_ne!(hash1, hash2);

        // Same transaction should have same hash
        let hash1_again = hash_transaction(&tx1);
        assert_eq!(hash1, hash1_again);
    }

    #[test]
    fn test_hex_roundtrip() {
        let bytes = [0xde, 0xad, 0xbe, 0xef];
        let hex_str = to_hex(&bytes);
        assert_eq!(hex_str, "deadbeef");

        let decoded = from_hex(&hex_str).expect("Should decode");
        assert_eq!(decoded, bytes);
    }

    #[test]
    fn test_keypair_from_hex() {
        let keypair1 = Keypair::generate();
        let secret_hex = to_hex(&keypair1.secret_key());

        let keypair2 = Keypair::from_hex(&secret_hex)
            .expect("Should parse hex secret key");

        assert_eq!(keypair1.public_key(), keypair2.public_key());
    }
}
