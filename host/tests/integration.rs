//! Integration tests for zk-perp
//!
//! Tests the full flow: transaction → state transition → proof generation

use zk_perp_core::{
    transactions::*,
    types::*,
};
use zk_perp_state::GlobalState;
use zk_perp_host::{Prover, BatchInput, TransactionWitness};

/// Test a complete deposit → proof cycle
#[test]
fn test_deposit_proof_cycle() {
    // 1. Create state and deposit transaction
    let mut state = GlobalState::with_default_markets();

    // Create account first - returns the account ID (starts from 0)
    let owner = [1u8; 32];
    let account_id = state.create_account(owner);

    let pre_root = state.root();

    // Process deposit
    let deposit_tx = DepositTx {
        account_id,
        asset_id: 0, // USDC
        amount: 10_000_000_000_000_000_000, // 10 USDC (18 decimals)
        nonce: 1,
    };

    let result = state.process_deposit(&deposit_tx)
        .expect("Deposit should succeed");

    let post_root = state.root();

    assert!(result.receipt.success);
    assert_ne!(pre_root, post_root, "Root should change after deposit");

    // 2. Generate proof using mock prover
    let prover = Prover::mock();

    let batch_input = BatchInput {
        pre_state_root: pre_root,
        post_state_root: post_root,
        transactions: vec![Transaction::Deposit(deposit_tx)],
        witnesses: vec![TransactionWitness::default()],
    };

    let receipt = prover.prove(batch_input)
        .expect("Mock proving should succeed");

    // 3. Verify the proof
    assert!(prover.verify(&receipt).expect("Verification should succeed"));
    assert_eq!(receipt.output.tx_count, 1);
    assert_eq!(receipt.output.pre_state_root, pre_root);
    assert_eq!(receipt.output.post_state_root, post_root);
}

/// Test deposit → place order → proof cycle
#[test]
fn test_order_placement_proof_cycle() {
    let mut state = GlobalState::with_default_markets();

    // Setup: create account with balance
    let owner = [2u8; 32];
    let account_id = state.create_account(owner);

    let pre_deposit_root = state.root();

    // Deposit USDC for margin
    let deposit_tx = DepositTx {
        account_id,
        asset_id: 0,
        amount: 100_000_000_000_000_000_000, // 100 USDC
        nonce: 1,
    };

    state.process_deposit(&deposit_tx)
        .expect("Deposit should succeed");

    // Place a limit buy order
    let place_order_tx = PlaceOrderTx {
        account_id,
        market_id: 0, // BTC-USDC
        side: Side::Bid,
        order_type: OrderType::Limit,
        price: 50_000_00000000, // $50,000
        quantity: 1_000_000_000_000_000_000, // 1 BTC
        post_only: false,
        reduce_only: false,
        nonce: 2,
    };

    let order_result = state.process_place_order(&place_order_tx)
        .expect("Place order should succeed");

    let post_order_root = state.root();

    assert!(order_result.receipt.success);
    assert!(order_result.receipt.order_id.is_some());

    // Generate proof for the entire batch
    let prover = Prover::mock();

    let batch_input = BatchInput {
        pre_state_root: pre_deposit_root,
        post_state_root: post_order_root,
        transactions: vec![
            Transaction::Deposit(deposit_tx),
            Transaction::PlaceOrder(place_order_tx),
        ],
        witnesses: vec![
            TransactionWitness::default(),
            TransactionWitness::default(),
        ],
    };

    let receipt = prover.prove(batch_input)
        .expect("Proving should succeed");

    // Verify
    assert!(prover.verify(&receipt).expect("Verification should succeed"));
    assert_eq!(receipt.output.tx_count, 2);
}

/// Test order matching (buy meets sell) → proof cycle
#[test]
fn test_matching_proof_cycle() {
    let mut state = GlobalState::with_default_markets();

    // Create two accounts
    let owner1 = [1u8; 32];
    let owner2 = [2u8; 32];
    let account1 = state.create_account(owner1);
    let account2 = state.create_account(owner2);

    let pre_root = state.root();

    // Both accounts deposit USDC
    let deposit1 = DepositTx {
        account_id: account1,
        asset_id: 0,
        amount: 100_000_000_000_000_000_000, // 100 USDC
        nonce: 1,
    };
    let deposit2 = DepositTx {
        account_id: account2,
        asset_id: 0,
        amount: 100_000_000_000_000_000_000, // 100 USDC
        nonce: 1,
    };

    state.process_deposit(&deposit1).expect("Deposit 1 should succeed");
    state.process_deposit(&deposit2).expect("Deposit 2 should succeed");

    // Account 1 places sell order
    let sell_order = PlaceOrderTx {
        account_id: account1,
        market_id: 0,
        side: Side::Ask,
        order_type: OrderType::Limit,
        price: 50_000_00000000, // $50,000
        quantity: 100_000_000_000_000_000, // 0.1 BTC
        post_only: true,
        reduce_only: false,
        nonce: 2,
    };

    let sell_result = state.process_place_order(&sell_order)
        .expect("Sell order should succeed");
    assert!(sell_result.receipt.success);
    assert!(sell_result.receipt.fills.is_empty(), "Post-only order should not fill");

    // Account 2 places buy order that matches
    let buy_order = PlaceOrderTx {
        account_id: account2,
        market_id: 0,
        side: Side::Bid,
        order_type: OrderType::Limit,
        price: 50_000_00000000, // $50,000 - same price, should match
        quantity: 100_000_000_000_000_000, // 0.1 BTC
        post_only: false,
        reduce_only: false,
        nonce: 2,
    };

    let buy_result = state.process_place_order(&buy_order)
        .expect("Buy order should succeed");

    let post_root = state.root();

    // Check fills occurred
    assert!(buy_result.receipt.success);
    assert!(!buy_result.receipt.fills.is_empty(), "Orders should match and produce fills");

    // Verify the fill details
    let fill = &buy_result.receipt.fills[0];
    assert_eq!(fill.price, 50_000_00000000);
    assert_eq!(fill.quantity, 100_000_000_000_000_000);

    // Generate and verify proof
    let prover = Prover::mock();

    let transactions = vec![
        Transaction::Deposit(deposit1),
        Transaction::Deposit(deposit2),
        Transaction::PlaceOrder(sell_order),
        Transaction::PlaceOrder(buy_order),
    ];

    let batch_input = BatchInput {
        pre_state_root: pre_root,
        post_state_root: post_root,
        transactions: transactions.clone(),
        witnesses: vec![TransactionWitness::default(); transactions.len()],
    };

    let receipt = prover.prove(batch_input).expect("Proving should succeed");

    assert!(prover.verify(&receipt).expect("Verification should succeed"));
    assert_eq!(receipt.output.tx_count, 4);
}

/// Test withdrawal proof cycle
#[test]
fn test_withdrawal_proof_cycle() {
    let mut state = GlobalState::with_default_markets();

    // Create account and deposit
    let owner = [3u8; 32];
    let account_id = state.create_account(owner);

    let pre_root = state.root();

    let deposit = DepositTx {
        account_id,
        asset_id: 0,
        amount: 50_000_000_000_000_000_000, // 50 USDC
        nonce: 1,
    };

    state.process_deposit(&deposit).expect("Deposit should succeed");

    // Withdraw half
    let withdraw = WithdrawTx {
        account_id,
        asset_id: 0,
        amount: 25_000_000_000_000_000_000, // 25 USDC
        nonce: 2,
    };

    let withdraw_result = state.process_withdraw(&withdraw)
        .expect("Withdraw should succeed");

    let post_root = state.root();

    assert!(withdraw_result.receipt.success);

    // Verify balance
    let account = state.get_account(account_id).expect("Account should exist");
    assert_eq!(account.free_balance(0), 25_000_000_000_000_000_000); // 25 USDC remaining

    // Generate proof
    let prover = Prover::mock();

    let batch_input = BatchInput {
        pre_state_root: pre_root,
        post_state_root: post_root,
        transactions: vec![
            Transaction::Deposit(deposit),
            Transaction::Withdraw(withdraw),
        ],
        witnesses: vec![
            TransactionWitness::default(),
            TransactionWitness::default(),
        ],
    };

    let receipt = prover.prove(batch_input).expect("Proving should succeed");
    assert!(prover.verify(&receipt).expect("Verification should succeed"));
    assert_eq!(receipt.output.tx_count, 2);
}

/// Test cancel order proof cycle
#[test]
fn test_cancel_order_proof_cycle() {
    let mut state = GlobalState::with_default_markets();

    let owner = [4u8; 32];
    let account_id = state.create_account(owner);

    let pre_root = state.root();

    // Deposit
    let deposit = DepositTx {
        account_id,
        asset_id: 0,
        amount: 100_000_000_000_000_000_000,
        nonce: 1,
    };
    state.process_deposit(&deposit).expect("Deposit should succeed");

    // Check balance before order
    let balance_before = state.get_account(account_id).unwrap().free_balance(0);

    // Place order
    let place_order = PlaceOrderTx {
        account_id,
        market_id: 0,
        side: Side::Bid,
        order_type: OrderType::Limit,
        price: 50_000_00000000,
        quantity: 1_000_000_000_000_000_000,
        post_only: true,
        reduce_only: false,
        nonce: 2,
    };

    let order_result = state.process_place_order(&place_order)
        .expect("Place order should succeed");

    let order_id = order_result.receipt.order_id.expect("Should have order ID");

    // Balance should be reduced by locked margin
    let balance_after_order = state.get_account(account_id).unwrap().free_balance(0);
    assert!(balance_after_order < balance_before, "Balance should decrease after placing order");

    // Cancel order
    let cancel = CancelOrderTx {
        account_id,
        order_id,
        market_id: 0,
        nonce: 3,
    };

    let cancel_result = state.process_cancel_order(&cancel)
        .expect("Cancel should succeed");

    let post_root = state.root();

    assert!(cancel_result.receipt.success);

    // Balance should be restored
    let balance_after_cancel = state.get_account(account_id).unwrap().free_balance(0);
    assert_eq!(balance_after_cancel, balance_before, "Balance should be restored after cancel");

    // Generate proof
    let prover = Prover::mock();

    let batch_input = BatchInput {
        pre_state_root: pre_root,
        post_state_root: post_root,
        transactions: vec![
            Transaction::Deposit(deposit),
            Transaction::PlaceOrder(place_order),
            Transaction::CancelOrder(cancel),
        ],
        witnesses: vec![
            TransactionWitness::default(),
            TransactionWitness::default(),
            TransactionWitness::default(),
        ],
    };

    let receipt = prover.prove(batch_input).expect("Proving should succeed");
    assert!(prover.verify(&receipt).expect("Verification should succeed"));
    assert_eq!(receipt.output.tx_count, 3);
}

/// Test with REAL RISC Zero proving (slow, use --ignored to run)
#[test]
#[ignore]
fn test_real_risc0_proving() {
    // This test uses real RISC Zero ZK proving, which takes several minutes
    // Run with: cargo test test_real_risc0_proving -- --ignored

    let mut state = GlobalState::with_default_markets();

    // Create account
    let owner = [99u8; 32];
    let account_id = state.create_account(owner);

    let pre_root = state.root();

    // Single deposit transaction (smallest possible batch)
    let deposit = DepositTx {
        account_id,
        asset_id: 0,
        amount: 1_000_000_000_000_000_000, // 1 USDC
        nonce: 1,
    };

    state.process_deposit(&deposit).expect("Deposit should succeed");
    let post_root = state.root();

    // Use REAL prover
    let prover = Prover::new();

    let batch_input = BatchInput {
        pre_state_root: pre_root,
        post_state_root: post_root,
        transactions: vec![Transaction::Deposit(deposit)],
        witnesses: vec![TransactionWitness::default()],
    };

    println!("Starting RISC Zero proof generation...");
    let start = std::time::Instant::now();

    let receipt = prover.prove(batch_input)
        .expect("Real RISC Zero proving should succeed");

    println!("Proof generated in {:?}", start.elapsed());

    // Verify the proof
    assert!(prover.verify(&receipt).expect("Real verification should succeed"));
    assert_eq!(receipt.output.tx_count, 1);
    assert_eq!(receipt.output.pre_state_root, pre_root);
    assert_eq!(receipt.output.post_state_root, post_root);

    println!("Real RISC Zero proof verified successfully!");
}

/// Test batch proof with multiple transaction types
#[test]
fn test_mixed_batch_proof() {
    let mut state = GlobalState::with_default_markets();

    // Create multiple accounts
    let mut account_ids = Vec::new();
    for i in 1..=3 {
        let mut owner = [0u8; 32];
        owner[0] = i;
        account_ids.push(state.create_account(owner));
    }

    let pre_root = state.root();

    let mut transactions = Vec::new();

    // Multiple deposits
    for &account_id in &account_ids {
        let deposit = DepositTx {
            account_id,
            asset_id: 0,
            amount: 100_000_000_000_000_000_000,
            nonce: 1,
        };
        state.process_deposit(&deposit).expect("Deposit should succeed");
        transactions.push(Transaction::Deposit(deposit));
    }

    // Multiple orders from first two accounts
    for (i, &account_id) in account_ids.iter().take(2).enumerate() {
        let side = if i == 0 { Side::Ask } else { Side::Bid };
        let order = PlaceOrderTx {
            account_id,
            market_id: 0,
            side,
            order_type: OrderType::Limit,
            price: 50_000_00000000,
            quantity: 50_000_000_000_000_000, // 0.05 BTC
            post_only: i == 0, // First is post-only
            reduce_only: false,
            nonce: 2,
        };
        state.process_place_order(&order).expect("Order should succeed");
        transactions.push(Transaction::PlaceOrder(order));
    }

    let post_root = state.root();

    // Generate proof for entire batch
    let prover = Prover::mock();

    let witnesses = vec![TransactionWitness::default(); transactions.len()];

    let batch_input = BatchInput {
        pre_state_root: pre_root,
        post_state_root: post_root,
        transactions,
        witnesses,
    };

    let receipt = prover.prove(batch_input).expect("Proving should succeed");
    assert!(prover.verify(&receipt).expect("Verification should succeed"));
    assert_eq!(receipt.output.tx_count, 5);
}
