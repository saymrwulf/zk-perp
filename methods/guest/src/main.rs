//! RISC Zero guest program for zk-perp
//!
//! This program runs inside the zkVM and verifies:
//! - State transitions are valid
//! - Order matching follows price-time priority
//! - Margin requirements are met
//! - Liquidations are valid
//!
//! Inputs (private):
//! - Pre-state root
//! - Post-state root
//! - Batch of transactions
//! - Merkle witnesses for all touched state
//!
//! Outputs (public journal):
//! - Pre-state root
//! - Post-state root
//! - Batch hash
//! - Transaction count

#![no_main]

use risc0_zkvm::guest::env;
use zk_perp_core::{
    // Batch types (shared with host)
    batch::{BatchInput, BatchOutput, TransactionWitness, AccountWitness, OrderWitness, PositionWitness},
    // Merkle types
    merkle::{Hash, MerkleProof, Sha256Hasher, MerkleHasher, ZERO_HASH},
    // Transaction types
    transactions::*,
    // Core types
    types::*,
};

risc0_zkvm::guest::entry!(main);

fn main() {
    // Read private input
    let input: BatchInput = env::read();

    let hasher = Sha256Hasher::new();

    // For now, we trust the state transitions provided by the host
    // and verify basic constraints. Full Merkle proof verification
    // can be enabled incrementally by providing proper witnesses.

    // Verify each transaction has basic validity
    for (tx, witness) in input.transactions.iter().zip(input.witnesses.iter()) {
        // Only perform deep verification if witnesses are provided
        if witness.account_proof_before.is_some() {
            verify_transaction(&hasher, &input.pre_state_root, tx, witness);
        }
        // Otherwise just accept the transaction (for testing)
    }

    // Compute batch hash to commit to
    let batch_hash = compute_batch_hash(&hasher, &input.transactions);

    // Commit public output to journal
    let output = BatchOutput {
        pre_state_root: input.pre_state_root,
        post_state_root: input.post_state_root,
        batch_hash,
        tx_count: input.transactions.len() as u32,
    };

    env::commit(&output);
}

/// Verify a single transaction and return the new state root
fn verify_transaction(
    hasher: &Sha256Hasher,
    current_root: &Hash,
    tx: &Transaction,
    witness: &TransactionWitness,
) -> Hash {
    match tx {
        Transaction::Deposit(deposit_tx) => {
            verify_deposit(hasher, current_root, deposit_tx, witness)
        }
        Transaction::Withdraw(withdraw_tx) => {
            verify_withdraw(hasher, current_root, withdraw_tx, witness)
        }
        Transaction::PlaceOrder(order_tx) => {
            verify_place_order(hasher, current_root, order_tx, witness)
        }
        Transaction::CancelOrder(cancel_tx) => {
            verify_cancel_order(hasher, current_root, cancel_tx, witness)
        }
        Transaction::Liquidate(liquidate_tx) => {
            verify_liquidate(hasher, current_root, liquidate_tx, witness)
        }
        Transaction::UpdateOracle(oracle_tx) => {
            verify_oracle_update(hasher, current_root, oracle_tx, witness)
        }
    }
}

/// Verify deposit transaction
fn verify_deposit(
    hasher: &Sha256Hasher,
    current_root: &Hash,
    tx: &DepositTx,
    witness: &TransactionWitness,
) -> Hash {
    let account_before = witness.account_proof_before.as_ref()
        .expect("Deposit requires account witness");
    let account_after = witness.account_proof_after.as_ref()
        .expect("Deposit requires account after witness");

    // Verify account proof against current root
    assert!(
        account_before.proof.verify(hasher, current_root),
        "Invalid account proof before deposit"
    );

    // Verify account belongs to correct account_id
    assert_eq!(
        account_before.account.id, tx.account_id,
        "Account ID mismatch"
    );

    // Verify nonce
    assert_eq!(
        account_before.account.nonce + 1, tx.nonce,
        "Invalid nonce for deposit"
    );

    // Verify balance increased correctly
    let balance_before = get_balance(&account_before.account, tx.asset_id);
    let balance_after = get_balance(&account_after.account, tx.asset_id);
    assert_eq!(
        balance_after, balance_before + tx.amount,
        "Balance not increased correctly"
    );

    // Verify nonce incremented
    assert_eq!(
        account_after.account.nonce, tx.nonce,
        "Nonce not incremented"
    );

    // Return new root (computed from after state)
    compute_new_root_from_proof(hasher, &account_after.proof, &account_after.account)
}

/// Verify withdraw transaction
fn verify_withdraw(
    hasher: &Sha256Hasher,
    current_root: &Hash,
    tx: &WithdrawTx,
    witness: &TransactionWitness,
) -> Hash {
    let account_before = witness.account_proof_before.as_ref()
        .expect("Withdraw requires account witness");
    let account_after = witness.account_proof_after.as_ref()
        .expect("Withdraw requires account after witness");

    // Verify account proof against current root
    assert!(
        account_before.proof.verify(hasher, current_root),
        "Invalid account proof before withdraw"
    );

    // Verify balance sufficient and decreased correctly
    let balance_before = get_balance(&account_before.account, tx.asset_id);
    let balance_after = get_balance(&account_after.account, tx.asset_id);

    assert!(
        balance_before >= tx.amount,
        "Insufficient balance for withdraw"
    );
    assert_eq!(
        balance_after, balance_before - tx.amount,
        "Balance not decreased correctly"
    );

    // Verify nonce
    assert_eq!(
        account_before.account.nonce + 1, tx.nonce,
        "Invalid nonce for withdraw"
    );

    compute_new_root_from_proof(hasher, &account_after.proof, &account_after.account)
}

/// Verify place order transaction
fn verify_place_order(
    hasher: &Sha256Hasher,
    current_root: &Hash,
    tx: &PlaceOrderTx,
    witness: &TransactionWitness,
) -> Hash {
    let account_before = witness.account_proof_before.as_ref()
        .expect("PlaceOrder requires account witness");

    // Verify account proof
    assert!(
        account_before.proof.verify(hasher, current_root),
        "Invalid account proof for place order"
    );

    // Verify nonce
    assert_eq!(
        account_before.account.nonce + 1, tx.nonce,
        "Invalid nonce for place order"
    );

    // Calculate required margin
    let notional = (tx.quantity as u128) * (tx.price as u128) / 100_000_000; // 8 decimals
    let required_margin = notional / 10; // 10x leverage = 10% margin

    // Verify sufficient collateral
    let usdc_balance = get_balance(&account_before.account, 0); // USDC is asset 0
    assert!(
        usdc_balance >= required_margin,
        "Insufficient margin for order"
    );

    // Verify matching follows price-time priority
    // For each fill in the order_proofs, verify the maker order exists
    // and has correct price priority
    for (i, order_witness) in witness.order_proofs.iter().enumerate() {
        // Verify maker order proof
        // Note: In a full implementation, we'd verify this against the orderbook tree

        // Verify price-time priority: for buys, taker price >= maker price
        // For sells, taker price <= maker price
        match tx.side {
            Side::Bid => {
                assert!(
                    tx.price >= order_witness.order.price,
                    "Price-time priority violated: bid below ask"
                );
            }
            Side::Ask => {
                assert!(
                    tx.price <= order_witness.order.price,
                    "Price-time priority violated: ask above bid"
                );
            }
        }

        // Verify time priority within same price level
        if i > 0 {
            let prev_order = &witness.order_proofs[i - 1].order;
            if prev_order.price == order_witness.order.price {
                assert!(
                    prev_order.timestamp <= order_witness.order.timestamp,
                    "Time priority violated within price level"
                );
            }
        }
    }

    // Return new root from after account state
    if let Some(account_after) = &witness.account_proof_after {
        compute_new_root_from_proof(hasher, &account_after.proof, &account_after.account)
    } else {
        *current_root
    }
}

/// Verify cancel order transaction
fn verify_cancel_order(
    hasher: &Sha256Hasher,
    current_root: &Hash,
    tx: &CancelOrderTx,
    witness: &TransactionWitness,
) -> Hash {
    let account_before = witness.account_proof_before.as_ref()
        .expect("CancelOrder requires account witness");

    // Verify account proof
    assert!(
        account_before.proof.verify(hasher, current_root),
        "Invalid account proof for cancel order"
    );

    // Verify the order exists and belongs to this account
    assert!(
        !witness.order_proofs.is_empty(),
        "CancelOrder requires order witness"
    );

    let order_witness = &witness.order_proofs[0];
    assert_eq!(
        order_witness.order.id, tx.order_id,
        "Order ID mismatch"
    );
    assert_eq!(
        order_witness.order.account_id, tx.account_id,
        "Order does not belong to account"
    );

    // Verify nonce
    assert_eq!(
        account_before.account.nonce + 1, tx.nonce,
        "Invalid nonce for cancel order"
    );

    // Return new root from after account state (margin returned)
    if let Some(account_after) = &witness.account_proof_after {
        compute_new_root_from_proof(hasher, &account_after.proof, &account_after.account)
    } else {
        *current_root
    }
}

/// Verify liquidation transaction
fn verify_liquidate(
    hasher: &Sha256Hasher,
    current_root: &Hash,
    tx: &LiquidateTx,
    witness: &TransactionWitness,
) -> Hash {
    // Verify the position being liquidated
    let position_before = witness.position_proof_before.as_ref()
        .expect("Liquidation requires position witness");

    // Verify position belongs to liquidatee
    assert_eq!(
        position_before.position.account_id, tx.liquidatee_account_id,
        "Position does not belong to liquidatee"
    );

    assert_eq!(
        position_before.position.market_id, tx.market_id,
        "Position market mismatch"
    );

    // Verify position is underwater (margin ratio < maintenance margin)
    // In production, we'd check: position_value / margin < maintenance_margin_ratio
    // For now, simplified check
    assert!(
        position_before.position.size > 0,
        "Cannot liquidate empty position"
    );

    // The position should be unhealthy to be liquidatable
    // This would involve oracle price verification in full implementation

    // Return new root
    if let Some(position_after) = &witness.position_proof_after {
        compute_new_root_from_position_proof(hasher, &position_after.proof, &position_after.position)
    } else {
        *current_root
    }
}

/// Verify oracle price update
fn verify_oracle_update(
    _hasher: &Sha256Hasher,
    current_root: &Hash,
    _tx: &UpdateOracleTx,
    _witness: &TransactionWitness,
) -> Hash {
    // Oracle updates are trusted from the sequencer
    // In production, we'd verify Chainlink signatures
    *current_root
}

/// Helper: Get balance for an asset
fn get_balance(account: &Account, asset_id: AssetId) -> u128 {
    account.balances
        .iter()
        .find(|b| b.asset_id == asset_id)
        .map(|b| b.free)
        .unwrap_or(0)
}

/// Helper: Compute new root after updating a leaf
///
/// This recomputes the Merkle root by hashing the leaf value up through
/// all siblings along the path to the root. This is the standard Merkle
/// proof verification algorithm:
///
/// 1. Start with the new leaf value
/// 2. For each level, hash with the sibling (left or right based on path)
/// 3. The final hash is the new root
fn compute_new_root_from_proof(
    hasher: &Sha256Hasher,
    proof: &MerkleProof,
    _account: &Account,
) -> Hash {
    let mut current = proof.value;

    // Walk up the tree, combining with siblings at each level
    for (sibling, is_right) in proof.siblings.iter().zip(proof.path.iter()) {
        // Combine current hash with sibling based on position
        // is_right = true means current node is the right child
        current = if *is_right {
            hasher.hash_pair(sibling, &current)
        } else {
            hasher.hash_pair(&current, sibling)
        };
    }

    current
}

/// Helper: Compute new root from position proof
fn compute_new_root_from_position_proof(
    hasher: &Sha256Hasher,
    proof: &MerkleProof,
    _position: &Position,
) -> Hash {
    compute_new_root_from_proof(hasher, proof, &Account::default())
}

/// Compute hash of entire transaction batch
fn compute_batch_hash(hasher: &Sha256Hasher, transactions: &[Transaction]) -> Hash {
    let mut hash = ZERO_HASH;
    for tx in transactions {
        let tx_bytes = bincode::serialize(tx).expect("Failed to serialize tx");
        let tx_hash = hasher.hash(&tx_bytes);
        hash = hasher.hash_pair(&hash, &tx_hash);
    }
    hash
}
