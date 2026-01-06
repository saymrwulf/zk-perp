# zk-perp Architecture Deep Dive

This document provides an in-depth technical explanation of the zk-perp system architecture, data structures, and algorithms.

## Table of Contents

1. [System Overview](#system-overview)
2. [Data Structures](#data-structures)
3. [Transaction Processing](#transaction-processing)
4. [Order Matching Engine](#order-matching-engine)
5. [Merkle Tree Implementation](#merkle-tree-implementation)
6. [ZK Circuit Design](#zk-circuit-design)
7. [Proof Generation Pipeline](#proof-generation-pipeline)
8. [State Reconstruction](#state-reconstruction)

---

## System Overview

### Design Principles

1. **Trustless Verification**: Anyone can verify the entire state history using only public data and ZK proofs
2. **Minimal Latency**: Transaction execution is immediate; proof generation is asynchronous
3. **State Commitment**: Every batch commits to a cryptographic state root
4. **Data Availability**: All transaction data is persisted before proof generation

### Component Interaction

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                                SEQUENCER                                    │
│                                                                             │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐  │
│  │   HTTP      │    │   Tx        │    │  Order      │    │   State     │  │
│  │   API       │───▶│ Validator   │───▶│  Matcher    │───▶│  Updater    │  │
│  │  (Axum)     │    │             │    │             │    │             │  │
│  └─────────────┘    └─────────────┘    └─────────────┘    └──────┬──────┘  │
│                                                                   │         │
│                           ┌───────────────────────────────────────┘         │
│                           │                                                 │
│                           ▼                                                 │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐                     │
│  │   Batch     │    │   Witness   │    │   Proof     │                     │
│  │   Builder   │───▶│ Generator   │───▶│  Generator  │                     │
│  │             │    │             │    │  (RISC Zero)│                     │
│  └──────┬──────┘    └─────────────┘    └──────┬──────┘                     │
│         │                                      │                            │
└─────────┼──────────────────────────────────────┼────────────────────────────┘
          │                                      │
          ▼                                      ▼
    ┌───────────┐                          ┌───────────┐
    │    DA     │◀─────────────────────────│   Proof   │
    │   Layer   │                          │  Storage  │
    └───────────┘                          └───────────┘
```

---

## Data Structures

### Account

Represents a trader's account with balances across multiple assets:

```rust
pub struct Account {
    /// Unique identifier (auto-incremented)
    pub id: AccountId,        // u64

    /// Owner's public key (ed25519)
    pub owner: PublicKey,     // [u8; 32]

    /// Transaction nonce for replay protection
    pub nonce: u64,

    /// Asset balances
    pub balances: Vec<Balance>,
}

pub struct Balance {
    pub asset_id: AssetId,    // u32
    pub free: u128,           // Available for trading
    pub locked: u128,         // Held in open orders
}
```

**Key invariants:**
- `nonce` must increment with each transaction
- `free + locked` must equal total balance
- All amounts use 18 decimal places (1 token = 10^18 units)

### Order

Represents a limit order in the order book:

```rust
pub struct Order {
    pub id: OrderId,          // u64, globally unique
    pub account_id: AccountId,
    pub market_id: MarketId,  // u32
    pub side: Side,           // Bid or Ask
    pub price: Price,         // u64, 8 decimal places
    pub quantity: u128,       // 18 decimal places
    pub remaining_qty: u128,  // Unfilled quantity
    pub timestamp: u64,       // Creation time (ms)
    pub nonce: u64,           // Order nonce (unique within account)
}

pub enum Side {
    Bid,  // Buy order
    Ask,  // Sell order
}
```

**Price encoding:**
- Price uses 8 decimal places
- $50,000.00 = `50_000_00000000` (50000 * 10^8)
- This allows prices from $0.00000001 to ~$184 billion

### Position

Represents an open perpetual position:

```rust
pub struct Position {
    pub account_id: AccountId,
    pub market_id: MarketId,
    pub side: PositionSide,   // Long or Short
    pub size: u128,           // 18 decimals
    pub entry_price: Price,   // Average entry
    pub margin: u128,         // Collateral
    pub leverage: u8,         // 1-100x
}

pub enum PositionSide {
    Long,   // Profit when price goes up
    Short,  // Profit when price goes down
}
```

**Margin calculation:**
```
notional_value = size * current_price
margin_ratio = margin / notional_value
maintenance_margin = 2.5% (configurable)

If margin_ratio < maintenance_margin:
    Position is liquidatable
```

### Transaction Types

```rust
pub enum Transaction {
    /// Deposit funds into account
    Deposit(DepositTx),

    /// Withdraw funds from account
    Withdraw(WithdrawTx),

    /// Place a limit order
    PlaceOrder(PlaceOrderTx),

    /// Cancel an existing order
    CancelOrder(CancelOrderTx),

    /// Liquidate an underwater position
    Liquidate(LiquidateTx),

    /// Update oracle price (sequencer only)
    UpdateOracle(UpdateOracleTx),
}
```

Each transaction type has specific fields:

```rust
pub struct DepositTx {
    pub account_id: AccountId,
    pub asset_id: AssetId,
    pub amount: u128,
    pub nonce: u64,
}

pub struct PlaceOrderTx {
    pub account_id: AccountId,
    pub market_id: MarketId,
    pub side: Side,
    pub price: Price,
    pub quantity: u128,
    pub nonce: u64,
}
```

---

## Transaction Processing

### Validation Pipeline

Every transaction passes through multiple validation stages:

```
┌────────────────────────────────────────────────────────────────┐
│                    VALIDATION PIPELINE                         │
│                                                                │
│  ┌─────────────┐                                               │
│  │  Signature  │  Verify ed25519 signature against public key  │
│  │  Check      │  from account owner                          │
│  └──────┬──────┘                                               │
│         │ Pass                                                 │
│         ▼                                                      │
│  ┌─────────────┐                                               │
│  │   Nonce     │  nonce == account.nonce + 1                  │
│  │   Check     │  (prevents replay attacks)                   │
│  └──────┬──────┘                                               │
│         │ Pass                                                 │
│         ▼                                                      │
│  ┌─────────────┐                                               │
│  │  Balance    │  Sufficient funds for operation              │
│  │  Check      │  (deposit: N/A, withdraw: free >= amount)    │
│  └──────┬──────┘                                               │
│         │ Pass                                                 │
│         ▼                                                      │
│  ┌─────────────┐                                               │
│  │  Business   │  Market exists, valid price range,           │
│  │  Rules      │  position limits, etc.                       │
│  └──────┬──────┘                                               │
│         │ Pass                                                 │
│         ▼                                                      │
│       EXECUTE                                                  │
└────────────────────────────────────────────────────────────────┘
```

### State Transition

Each transaction produces deterministic state changes:

```rust
impl GlobalState {
    pub fn apply_deposit(&mut self, tx: &DepositTx) -> Result<Receipt> {
        // 1. Get account (mutable)
        let account = self.get_account_mut(tx.account_id)?;

        // 2. Verify nonce
        ensure!(account.nonce + 1 == tx.nonce, "Invalid nonce");

        // 3. Update balance
        let balance = account.get_balance_mut(tx.asset_id);
        balance.free += tx.amount;

        // 4. Increment nonce
        account.nonce = tx.nonce;

        // 5. Update Merkle tree
        self.accounts.update(account.id, account)?;

        // 6. Return receipt
        Ok(Receipt::success(tx.hash()))
    }
}
```

---

## Order Matching Engine

### Data Structure: Price-Time Priority

The order book uses a two-level structure:

```
BTreeMap<Price, VecDeque<Order>>

Level 1: Prices sorted by BTreeMap
         - Bids: Highest first (Reverse<Price>)
         - Asks: Lowest first (Price)

Level 2: Orders at each price level in FIFO queue
         - First order placed = first to be filled

┌─────────────────────────────────────────────────────────────┐
│                      ASKS (Sell Orders)                     │
│   Price     │  Orders (FIFO)                                │
│  ───────────┼──────────────────────────────────────────     │
│   $50,200   │  [Order{qty:1.0, t:300}]                      │
│   $50,100   │  [Order{qty:0.5, t:200}, Order{qty:0.3, t:250}]│
│   $50,000   │  [Order{qty:2.0, t:100}]  ← Best Ask          │
├─────────────┴──────────────────────────────────────────────┤
│                         SPREAD                              │
├─────────────┬──────────────────────────────────────────────┤
│   $49,900   │  [Order{qty:1.5, t:150}]  ← Best Bid         │
│   $49,800   │  [Order{qty:0.8, t:120}]                      │
│   $49,700   │  [Order{qty:3.0, t:80}]                       │
│                      BIDS (Buy Orders)                      │
└─────────────────────────────────────────────────────────────┘
```

### Matching Algorithm

```rust
pub fn match_order(&mut self, taker: &mut Order) -> Vec<Fill> {
    let mut fills = Vec::new();

    // Get opposing side's book
    let book = match taker.side {
        Side::Bid => &mut self.asks,  // Buyer matches against sellers
        Side::Ask => &mut self.bids,  // Seller matches against buyers
    };

    // Iterate price levels in order
    let mut empty_levels = Vec::new();

    for (price, orders) in book.iter_mut() {
        // Check if price crosses
        let crosses = match taker.side {
            Side::Bid => taker.price >= *price,  // Buy if ask <= my bid
            Side::Ask => taker.price <= *price,  // Sell if bid >= my ask
        };

        if !crosses {
            break;  // No more matches possible
        }

        // Match against orders at this level (FIFO)
        while let Some(maker) = orders.front_mut() {
            // Calculate fill quantity
            let fill_qty = std::cmp::min(taker.remaining_qty, maker.remaining_qty);

            // Record fill
            fills.push(Fill {
                maker_order_id: maker.id,
                taker_order_id: taker.id,
                price: *price,  // Maker's price
                quantity: fill_qty,
                side: taker.side,
            });

            // Update quantities
            maker.remaining_qty -= fill_qty;
            taker.remaining_qty -= fill_qty;

            // Remove fully filled maker
            if maker.remaining_qty == 0 {
                orders.pop_front();
            }

            // Check if taker is fully filled
            if taker.remaining_qty == 0 {
                break;
            }
        }

        // Mark empty price levels for removal
        if orders.is_empty() {
            empty_levels.push(*price);
        }

        if taker.remaining_qty == 0 {
            break;
        }
    }

    // Clean up empty levels
    for price in empty_levels {
        book.remove(&price);
    }

    fills
}
```

### Fill Processing

Each fill updates multiple state components:

```
Fill Event: Buyer purchases 0.5 BTC at $50,000

┌─────────────────────────────────────────────────────────────┐
│ 1. UPDATE BUYER ACCOUNT                                     │
│    - USDC balance: free -= 25,000 (0.5 * 50000)            │
│    - Position: size += 0.5 BTC (or create new)             │
│    - Account tree: update leaf                              │
│                                                             │
│ 2. UPDATE SELLER ACCOUNT                                    │
│    - USDC balance: free += 25,000                          │
│    - Position: size -= 0.5 BTC                             │
│    - Account tree: update leaf                              │
│                                                             │
│ 3. UPDATE ORDER BOOK TREE                                   │
│    - Remove/update maker order                              │
│    - Order book tree: update leaf                           │
│                                                             │
│ 4. GENERATE WITNESSES                                       │
│    - Record Merkle paths for all touched leaves            │
│    - Include before/after states                           │
└─────────────────────────────────────────────────────────────┘
```

---

## Merkle Tree Implementation

### Sparse Merkle Tree (SMT)

A Sparse Merkle Tree allows proving both existence and non-existence of keys:

```
Height: 256 levels (for 256-bit keys)
Leaves: 2^256 possible positions

Most leaves are empty ("default" values).
Only non-empty leaves and their paths are stored.

           Root
          /    \
        H01    H23
       /  \   /  \
      H0  H1 H2  H3
     / \ / \ ...
    Empty or Value

For a tree of height H:
- Proof size: H * 32 bytes = 8KB for 256 levels
- Lookup time: O(H)
- Update time: O(H)
```

### Hash Function: SHA-256

The tree uses SHA-256 for internal hashing:

```rust
pub struct Sha256Hasher;

impl MerkleHasher for Sha256Hasher {
    /// Hash a single value
    fn hash(&self, data: &[u8]) -> Hash {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        hash
    }

    /// Hash two children to create parent
    fn hash_pair(&self, left: &Hash, right: &Hash) -> Hash {
        let mut hasher = Sha256::new();
        hasher.update(left);
        hasher.update(right);
        let result = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        hash
    }
}
```

### Proof Structure

```rust
pub struct MerkleProof {
    /// The key (position) being proved
    pub key: Hash,

    /// Hash of the leaf value
    pub value: Hash,

    /// Sibling hashes from leaf to root
    pub siblings: Vec<Hash>,

    /// Path direction at each level
    /// true = current node is right child
    /// false = current node is left child
    pub path: Vec<bool>,
}

impl MerkleProof {
    /// Verify the proof against a root hash
    pub fn verify(&self, hasher: &impl MerkleHasher, root: &Hash) -> bool {
        let mut current = self.value;

        for (sibling, is_right) in self.siblings.iter().zip(self.path.iter()) {
            current = if *is_right {
                // Current is right child: hash(sibling, current)
                hasher.hash_pair(sibling, &current)
            } else {
                // Current is left child: hash(current, sibling)
                hasher.hash_pair(&current, sibling)
            };
        }

        current == *root
    }
}
```

### Hypertree: 6-Tree Composition

The global state root is computed from 6 individual tree roots:

```rust
impl Hypertree {
    pub fn root(&self, hasher: &impl MerkleHasher) -> Hash {
        // Level 1: Pair up trees
        let h01 = hasher.hash_pair(&self.accounts.root(), &self.account_orders.root());
        let h23 = hasher.hash_pair(&self.orderbook.root(), &self.positions.root());
        let h45 = hasher.hash_pair(&self.pools.root(), &self.system.root());

        // Level 2: Combine pairs
        let h0123 = hasher.hash_pair(&h01, &h23);

        // Level 3: Final root
        hasher.hash_pair(&h0123, &h45)
    }
}
```

```
                   Global Root
                      │
           ┌──────────┴──────────┐
           │                     │
        H(01,23)              H(45)
           │                     │
     ┌─────┴─────┐         ┌─────┴─────┐
     │           │         │           │
   H(0,1)     H(2,3)     H(4)       H(5)
     │           │         │           │
  ┌──┴──┐    ┌──┴──┐   ┌──┴──┐    ┌──┴──┐
  │     │    │     │   │     │    │     │
 T1    T2   T3    T4  T5    T6   (T5)  (T6)
 Acc  AOrd  OB   Pos Pool  Sys

T1: Account Tree     - User balances and nonces
T2: Account Orders   - Orders per user
T3: Order Book       - Active orders by price
T4: Positions        - Open perpetual positions
T5: Pools            - LP pools (future)
T6: System           - Config, oracle prices
```

---

## ZK Circuit Design

### Guest Program Structure

The RISC Zero guest program verifies state transitions:

```rust
// Entry point
risc0_zkvm::guest::entry!(main);

fn main() {
    // 1. READ PRIVATE INPUTS
    let input: BatchInput = env::read();

    // 2. VERIFY EACH TRANSACTION
    let mut current_root = input.pre_state_root;

    for (tx, witness) in input.transactions.iter().zip(input.witnesses.iter()) {
        current_root = verify_transaction(&current_root, tx, witness);
    }

    // 3. VERIFY FINAL STATE
    assert_eq!(current_root, input.post_state_root, "State root mismatch");

    // 4. COMMIT PUBLIC OUTPUTS
    let output = BatchOutput {
        pre_state_root: input.pre_state_root,
        post_state_root: input.post_state_root,
        batch_hash: compute_batch_hash(&input.transactions),
        tx_count: input.transactions.len() as u32,
    };

    env::commit(&output);
}
```

### Witness Structure

Each transaction requires witnesses to verify state access:

```rust
pub struct TransactionWitness {
    /// Account state before transaction
    pub account_proof_before: Option<AccountWitness>,

    /// Account state after transaction
    pub account_proof_after: Option<AccountWitness>,

    /// Order proofs (for matching)
    pub order_proofs: Vec<OrderWitness>,

    /// Position state before (for liquidations)
    pub position_proof_before: Option<PositionWitness>,

    /// Position state after
    pub position_proof_after: Option<PositionWitness>,
}

pub struct AccountWitness {
    pub account: Account,
    pub proof: MerkleProof,
}
```

### Verification Logic

For each transaction type, specific constraints are verified:

```rust
fn verify_deposit(tx: &DepositTx, witness: &TransactionWitness) -> Hash {
    let before = &witness.account_proof_before.unwrap();
    let after = &witness.account_proof_after.unwrap();

    // 1. Verify Merkle proof (account exists in current state)
    assert!(before.proof.verify(&hasher, &current_root));

    // 2. Verify account ID matches
    assert_eq!(before.account.id, tx.account_id);

    // 3. Verify nonce increments correctly
    assert_eq!(after.account.nonce, before.account.nonce + 1);
    assert_eq!(after.account.nonce, tx.nonce);

    // 4. Verify balance increased by exact amount
    let bal_before = before.account.get_balance(tx.asset_id);
    let bal_after = after.account.get_balance(tx.asset_id);
    assert_eq!(bal_after, bal_before + tx.amount);

    // 5. Compute and return new root
    compute_new_root(&after.proof, &after.account)
}

fn verify_place_order(tx: &PlaceOrderTx, witness: &TransactionWitness) -> Hash {
    // ... verify account proof, nonce, margin

    // CRITICAL: Verify price-time priority for each fill
    for (i, order_witness) in witness.order_proofs.iter().enumerate() {
        // Verify maker order exists
        assert!(order_witness.proof.verify(&hasher, &current_root));

        // Verify price crosses
        match tx.side {
            Side::Bid => assert!(tx.price >= order_witness.order.price),
            Side::Ask => assert!(tx.price <= order_witness.order.price),
        }

        // Verify time priority (within same price level)
        if i > 0 && witness.order_proofs[i-1].order.price == order_witness.order.price {
            assert!(witness.order_proofs[i-1].order.timestamp <= order_witness.order.timestamp);
        }
    }

    // ... compute new root
}
```

---

## Proof Generation Pipeline

### Batch Builder

The sequencer collects transactions into batches:

```rust
impl BatchBuilder {
    pub async fn maybe_trigger_batch(&self) -> bool {
        let pending = self.pending_txs.read().await;

        // Trigger conditions:
        // 1. Batch size threshold reached
        // 2. Timeout elapsed since first pending tx
        let should_trigger =
            pending.len() >= self.config.max_batch_size ||
            (pending.len() > 0 && pending[0].received_at.elapsed() >= self.config.batch_timeout);

        if should_trigger {
            drop(pending);
            self.process_batch().await;
            true
        } else {
            false
        }
    }

    pub async fn process_batch(&self) {
        // 1. Take all pending transactions
        let transactions: Vec<_> = self.pending_txs.write().await.drain(..).collect();

        // 2. Get state roots
        let state = self.state.read().await;
        let pre_root = self.da.read().await.current_state_root();
        let post_root = state.root();

        // 3. Generate witnesses for ZK proof
        let witnesses = self.generate_witnesses(&transactions, &state);

        // 4. Store batch in DA (before proof, for availability)
        let batch_hash = compute_batch_hash(&transactions);
        let batch_id = self.da.write().await.append_batch(
            transactions.clone(),
            pre_root,
            post_root,
            batch_hash,
        );

        // 5. Generate ZK proof (async, can be slow)
        let input = BatchInput {
            pre_state_root: pre_root,
            post_state_root: post_root,
            transactions,
            witnesses,
        };

        let receipt = self.prover.prove(input).await;

        // 6. Store proof in DA
        self.da.write().await.append_proof(batch_id, receipt);
    }
}
```

### RISC Zero Proving

```rust
impl Prover {
    pub fn prove(&self, input: BatchInput) -> Result<ProofReceipt, ProverError> {
        // 1. Serialize input for guest
        let input_bytes = bincode::serialize(&input)?;

        // 2. Create executor environment
        let env = ExecutorEnv::builder()
            .write(&input)?
            .build()?;

        // 3. Execute guest and generate proof
        let receipt = default_prover()
            .prove(env, ZK_PERP_GUEST_ELF)?
            .receipt;

        // 4. Extract public output from journal
        let output: BatchOutput = receipt.journal.decode()?;

        Ok(ProofReceipt {
            journal: receipt.journal.bytes.to_vec(),
            seal: bincode::serialize(&receipt)?,
            output,
        })
    }

    pub fn verify(&self, receipt: &ProofReceipt) -> Result<bool, ProverError> {
        // Deserialize receipt
        let risc0_receipt: Receipt = bincode::deserialize(&receipt.seal)?;

        // Verify against guest image ID
        risc0_receipt.verify(ZK_PERP_GUEST_ID)?;

        Ok(true)
    }
}
```

---

## State Reconstruction

### From DA to State

The entire state can be reconstructed from DA data:

```rust
impl AppendLog {
    /// Reconstruct state by replaying all transactions
    pub fn reconstruct_state(&mut self) -> Result<GlobalState, DaError> {
        let mut state = GlobalState::new();

        // Replay all batches in order
        for batch_id in 1..=self.last_batch_id() {
            let batch = self.get_batch(batch_id)?;

            // Verify continuity
            if batch_id > 1 {
                let prev = self.get_batch(batch_id - 1)?;
                assert_eq!(batch.pre_state_root, prev.post_state_root);
            }

            // Apply each transaction
            for tx in &batch.transactions {
                state.apply_transaction(tx)?;
            }

            // Verify resulting root matches batch
            assert_eq!(state.root(), batch.post_state_root);
        }

        Ok(state)
    }
}
```

### Verification Without Trust

Anyone can verify the entire chain:

```rust
impl Verifier {
    /// Verify entire state history
    pub fn verify_all(&mut self) -> Result<(), VerifierError> {
        let mut verified_root = [0u8; 32];  // Genesis

        for batch_id in 1..=self.da.last_batch_id() {
            // Load batch and proof
            let batch = self.da.get_batch(batch_id)?;
            let proof = self.da.get_proof(batch_id)?;

            // Check state continuity
            if batch_id > 1 {
                assert_eq!(batch.pre_state_root, verified_root);
            }

            // Deserialize and verify ZK proof
            let receipt: ProofReceipt = bincode::deserialize(&proof.receipt_bytes)?;
            self.prover.verify(&receipt)?;

            // Check receipt matches batch
            assert_eq!(receipt.output.pre_state_root, batch.pre_state_root);
            assert_eq!(receipt.output.post_state_root, batch.post_state_root);

            // Update verified state
            verified_root = batch.post_state_root;
        }

        self.verified_root = verified_root;
        Ok(())
    }
}
```

---

## Performance Considerations

### Proof Generation Time

| Batch Size | Mock Proof | Real Proof (RISC Zero) |
|------------|------------|------------------------|
| 1 tx       | <1ms       | ~16 seconds            |
| 10 txs     | <1ms       | ~20 seconds            |
| 100 txs    | <1ms       | ~45 seconds            |

Real proof times can be improved with:
- GPU acceleration (CUDA)
- Bonsai proving service
- Proof parallelization

### Merkle Tree Operations

| Operation | Time Complexity | Tree Height 256 |
|-----------|-----------------|-----------------|
| Get       | O(H)            | 256 hashes      |
| Insert    | O(H)            | 256 hashes      |
| Update    | O(H)            | 256 hashes      |
| Proof     | O(H)            | 8 KB            |

### Order Matching

| Operation | Time Complexity |
|-----------|-----------------|
| Add order | O(log P)        |
| Cancel    | O(1) with map   |
| Match     | O(F * log P)    |

Where P = number of price levels, F = number of fills.

---

## Security Analysis

### What Cannot Be Forged

1. **Signatures**: Ed25519 provides 128-bit security
2. **State roots**: SHA-256 collision resistance
3. **Proofs**: STARK soundness guarantees

### Trust Assumptions

1. **Sequencer Ordering**: Sequencer chooses transaction order
2. **DA Availability**: Batch data must be published
3. **Proof Generation**: Proofs must be generated correctly

### Mitigations

1. **Fair Ordering**: Time-based ordering in order book
2. **Forced Inclusion**: Timeout-based batch triggers
3. **Proof Verification**: Anyone can verify

---

## Future Improvements

1. **Decentralized Sequencing**: Leader election among validators
2. **External DA**: Celestia or EigenDA integration
3. **L1 Settlement**: Ethereum smart contract bridge
4. **Real Oracles**: Chainlink price feed integration
5. **Groth16 Proofs**: Smaller proofs for on-chain verification
