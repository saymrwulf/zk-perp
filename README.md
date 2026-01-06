# zk-perp: Zero-Knowledge Perpetual DEX

A fully functional proof-of-concept for a perpetual futures decentralized exchange (DEX) that uses zero-knowledge proofs for state transition verification. Built with RISC Zero zkVM.

## Table of Contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [How It Works](#how-it-works)
4. [Project Structure](#project-structure)
5. [Core Components](#core-components)
6. [Running the System](#running-the-system)
7. [API Reference](#api-reference)
8. [Technical Deep Dive](#technical-deep-dive)

---

## Overview

### What is a Perpetual DEX?

A perpetual futures exchange allows traders to speculate on asset prices with leverage without expiration dates. Unlike traditional futures that expire quarterly, perpetuals use a funding rate mechanism to keep prices aligned with the underlying spot market.

### Why Zero-Knowledge Proofs?

Traditional exchanges (even decentralized ones) require users to trust that:
- Order matching is fair
- Balances are computed correctly
- Liquidations happen at the right time

With ZK proofs, **anyone can verify** that every state transition follows the rules—without needing to trust anyone. The proofs are cryptographic guarantees that:

1. Orders matched at the correct prices
2. Balances updated correctly
3. All transactions were properly authorized
4. No funds appeared or disappeared

### This Implementation

This PoC implements a **sovereign rollup** architecture:
- **Sequencer**: Processes transactions and generates ZK proofs
- **DA Layer**: Stores all transaction data for reproducibility
- **Verifier**: Independently verifies proofs

It replicates the architecture of [Lighter](https://lighter.xyz) but uses RISC Zero's zkVM instead of custom Plonky2 circuits.

---

## Architecture

```
                                ┌─────────────────────────────────────┐
                                │           User Interface            │
                                │     (CLI, SDK, or Web Frontend)     │
                                └──────────────┬──────────────────────┘
                                               │
                                               ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│                              SEQUENCER (HTTP API)                            │
│  ┌────────────────┐  ┌────────────────┐  ┌────────────────┐                  │
│  │ Transaction    │  │    Order       │  │    State       │                  │
│  │  Validation    │──▶│   Matching     │──▶│   Updates      │                  │
│  └────────────────┘  └────────────────┘  └────────────────┘                  │
│         │                    │                    │                          │
│         └────────────────────┴────────────────────┘                          │
│                              │                                               │
│                              ▼                                               │
│                     ┌────────────────┐                                       │
│                     │  Batch Builder │                                       │
│                     │ (100 txs or 5s)│                                       │
│                     └───────┬────────┘                                       │
│                             │                                                │
└─────────────────────────────┼────────────────────────────────────────────────┘
                              │
              ┌───────────────┴───────────────┐
              │                               │
              ▼                               ▼
┌─────────────────────────┐      ┌─────────────────────────┐
│     DA Layer            │      │      ZK Prover          │
│                         │      │                         │
│  ┌───────────────────┐  │      │  ┌───────────────────┐  │
│  │  batches/         │  │      │  │   RISC Zero       │  │
│  │  ├── 000001.batch │  │      │  │   zkVM Guest      │  │
│  │  ├── 000002.batch │  │      │  │                   │  │
│  │  └── ...          │  │      │  │   Verifies:       │  │
│  ├───────────────────┤  │      │  │   - Signatures    │  │
│  │  proofs/          │  │      │  │   - State roots   │  │
│  │  ├── 000001.proof │  │      │  │   - Matching      │  │
│  │  └── ...          │  │      │  │   - Margins       │  │
│  ├───────────────────┤  │      │  └───────────────────┘  │
│  │  index.json       │  │      │                         │
│  └───────────────────┘  │      │   Output: STARK Proof   │
│                         │      │                         │
└─────────────────────────┘      └───────────┬─────────────┘
                                             │
                                             ▼
                               ┌─────────────────────────┐
                               │       VERIFIER          │
                               │                         │
                               │  - Reads from DA        │
                               │  - Verifies proofs      │
                               │  - Tracks verified      │
                               │    state root           │
                               │                         │
                               │  Anyone can run this!   │
                               └─────────────────────────┘
```

---

## How It Works

### The Transaction Lifecycle

```
1. USER SUBMITS TRANSACTION
   │
   │  POST /tx { signed_transaction: { tx: {...}, signature: [...] } }
   │
   ▼
2. SEQUENCER RECEIVES TX
   │
   ├─► Verify ed25519 signature
   │   └─► Public key must match account owner
   │
   ├─► Validate transaction
   │   ├─► Account exists?
   │   ├─► Nonce correct?
   │   ├─► Sufficient balance?
   │   └─► Valid market/order params?
   │
   ├─► Execute against state
   │   ├─► Update account balances
   │   ├─► Match orders (if applicable)
   │   └─► Update positions
   │
   └─► Add to pending batch
   │
   ▼
3. BATCH PROCESSING (every 100 txs or 5 seconds)
   │
   ├─► Collect pending transactions
   │
   ├─► Compute state roots
   │   ├─► Pre-state root (before batch)
   │   └─► Post-state root (after batch)
   │
   ├─► Store batch in DA layer
   │
   └─► Generate ZK proof
   │
   ▼
4. ZK PROOF GENERATION (RISC Zero)
   │
   │  Input (private):
   │  ├─► Pre-state root
   │  ├─► Post-state root
   │  ├─► All transactions
   │  └─► Merkle witnesses
   │
   │  Guest program verifies:
   │  ├─► Each signature is valid
   │  ├─► Nonces are correct
   │  ├─► Balances update correctly
   │  ├─► Order matching is fair
   │  └─► State transitions are valid
   │
   │  Output (public journal):
   │  ├─► Pre-state root
   │  ├─► Post-state root
   │  ├─► Batch hash
   │  └─► Transaction count
   │
   ▼
5. PROOF STORED IN DA
   │
   │  Anyone can now verify the batch!
   │
   ▼
6. VERIFIER CHECKS PROOF
   │
   ├─► Load batch from DA
   ├─► Load proof from DA
   ├─► Verify proof against guest image ID
   ├─► Check state continuity (roots chain)
   └─► Update verified state
```

### State Model: The Hypertree

The system maintains state across 6 Merkle trees, combined into a single root:

```
                    ┌─────────────────┐
                    │   Global Root   │
                    └────────┬────────┘
                             │
            ┌────────────────┴────────────────┐
            │                                 │
     ┌──────┴──────┐                  ┌───────┴───────┐
     │             │                  │               │
  ┌──┴──┐      ┌───┴───┐          ┌───┴───┐      ┌───┴───┐
  │     │      │       │          │       │      │       │
 T1    T2     T3      T4         T5      T6
```

| Tree | Name | Contents | Key Format |
|------|------|----------|------------|
| T1 | Account | Balances, nonces, metadata | `hash(account_id)` |
| T2 | Account Orders | Orders per account | `hash(account_id \|\| market_id \|\| idx)` |
| T3 | Order Book | All active orders | `price_bits(64) \|\| nonce(8)` |
| T4 | Position | Open positions | `hash(account_id \|\| market_id)` |
| T5 | Pool | LP pools (future) | `hash(pool_id)` |
| T6 | System | Markets, oracle, config | `hash(key)` |

### Order Matching: Price-Time Priority

The order book uses a price-time priority matching algorithm:

```
Example: New BUY order arrives at $50,000

ASKS (Sell orders, sorted low→high):
┌─────────────────────────────────────────────────────────┐
│  $50,100  │ 1.0 BTC │ timestamp: 100  │ ← Not filled   │
│  $50,050  │ 0.5 BTC │ timestamp: 90   │ ← Not filled   │
│  $50,000  │ 0.3 BTC │ timestamp: 80   │ ← Filled 2nd   │
│  $50,000  │ 0.2 BTC │ timestamp: 70   │ ← Filled 1st   │ (earlier)
│  $49,950  │ 0.1 BTC │ timestamp: 60   │ ← Filled 0th   │ (best price)
└─────────────────────────────────────────────────────────┘

Matching order:
1. Best price first ($49,950 before $50,000)
2. At same price, earliest order first (timestamp 70 before 80)
```

This is exactly what the ZK proof verifies—that matching followed these rules.

---

## Project Structure

```
zk-perp/
├── Cargo.toml              # Workspace configuration
├── README.md               # This file
│
├── crates/
│   ├── core/               # Shared types and cryptographic primitives
│   │   └── src/
│   │       ├── types/      # Order, Account, Position, Market
│   │       ├── merkle/     # Sparse Merkle Tree, SHA-256 hashing
│   │       ├── transactions/ # Transaction types (Deposit, PlaceOrder, etc.)
│   │       ├── batch.rs    # Batch input/output for ZK prover
│   │       └── crypto.rs   # ed25519 signatures
│   │
│   ├── orderbook/          # Order matching engine
│   │   └── src/
│   │       ├── book.rs     # BTreeMap-based order book
│   │       └── matching.rs # Price-time priority matching
│   │
│   ├── state/              # Global state management
│   │   └── src/
│   │       └── lib.rs      # GlobalState with all 6 trees
│   │
│   ├── oracle/             # Price feeds
│   │   └── src/
│   │       └── lib.rs      # MockOracle, SimulatedOracle
│   │
│   └── da/                 # Data Availability layer
│       └── src/
│           └── lib.rs      # AppendLog (file-based)
│
├── methods/                # RISC Zero guest program
│   ├── guest/
│   │   └── src/
│   │       └── main.rs     # ZK verification logic
│   └── build.rs
│
├── host/                   # Prover (generates ZK proofs)
│   └── src/
│       └── lib.rs          # Prover struct, proof generation
│
├── sequencer/              # Main application server
│   └── src/
│       ├── lib.rs          # Sequencer logic
│       └── api.rs          # HTTP API (Axum)
│
└── verifier/               # Standalone verification node
    └── src/
        ├── lib.rs          # Verification logic
        └── main.rs         # CLI and HTTP API
```

---

## Core Components

### 1. Cryptographic Primitives (`crates/core/src/crypto.rs`)

**Ed25519 Digital Signatures** provide authentication:

```rust
// Generate a keypair
let keypair = Keypair::generate();

// Sign a transaction
let signed_tx = keypair.sign_transaction(&tx)?;

// Verify a signature
let valid = verify_transaction(&signed_tx, &public_key)?;
```

Ed25519 is chosen for:
- Fast signing and verification (critical for trading)
- 128-bit security level
- Deterministic signatures (same input = same signature)
- Compact signatures (64 bytes)

### 2. Merkle Trees (`crates/core/src/merkle/`)

**Sparse Merkle Trees** provide O(log n) proofs of state:

```rust
// A Merkle proof proves a value exists at a key
pub struct MerkleProof {
    pub key: Hash,           // The key being proved
    pub value: Hash,         // Hash of the value
    pub siblings: Vec<Hash>, // Sibling hashes along the path
    pub path: Vec<bool>,     // Direction at each level (left/right)
}

// Verification: rehash from leaf to root
fn verify(&self, root: &Hash) -> bool {
    let mut current = self.value;
    for (sibling, is_right) in self.siblings.iter().zip(self.path.iter()) {
        current = if *is_right {
            hash_pair(sibling, &current)
        } else {
            hash_pair(&current, sibling)
        };
    }
    current == *root
}
```

The tree structure:
```
Level 3 (root):     [H(H01, H23)]
                    /            \
Level 2:        [H01]            [H23]
               /     \          /     \
Level 1:    [H0]    [H1]     [H2]    [H3]
           /   \   /   \    /   \   /   \
Level 0:  V0   V1 V2   V3  V4   V5 V6   V7   (leaf values)

Proof for V2:
- value = V2
- siblings = [H3, H01]
- path = [false, true]  (left at level 1, right at level 2)
```

### 3. Order Book (`crates/orderbook/`)

The order book uses `BTreeMap` for efficient price-level management:

```rust
pub struct OrderBook {
    /// Bids sorted by price (highest first)
    bids: BTreeMap<Reverse<Price>, VecDeque<Order>>,
    /// Asks sorted by price (lowest first)
    asks: BTreeMap<Price, VecDeque<Order>>,
    /// Quick lookup of order locations
    order_locations: HashMap<OrderId, OrderLocation>,
}
```

Matching algorithm:
```rust
fn match_order(&mut self, taker: Order) -> Vec<Fill> {
    let fills = vec![];

    // For a BUY, iterate asks from lowest price
    // For a SELL, iterate bids from highest price
    let levels = match taker.side {
        Side::Bid => &mut self.asks,
        Side::Ask => &mut self.bids,
    };

    for (price, orders) in levels {
        // Check if price crosses (BUY price >= ASK price)
        if !price_crosses(taker.side, taker.price, *price) {
            break;
        }

        // Match against orders at this level (FIFO)
        while let Some(maker) = orders.front_mut() {
            let fill_qty = min(taker.remaining, maker.remaining);
            fills.push(Fill { maker, taker, qty: fill_qty, price: *price });

            // Update remaining quantities...
        }
    }

    fills
}
```

### 4. ZK Guest Program (`methods/guest/src/main.rs`)

The guest program runs inside RISC Zero's zkVM and verifies:

```rust
fn main() {
    // Read private inputs
    let input: BatchInput = env::read();

    // Verify each transaction
    for (tx, witness) in input.transactions.iter().zip(input.witnesses.iter()) {
        verify_transaction(&tx, &witness);
    }

    // Compute batch hash
    let batch_hash = compute_batch_hash(&input.transactions);

    // Commit public outputs to journal
    env::commit(&BatchOutput {
        pre_state_root: input.pre_state_root,
        post_state_root: input.post_state_root,
        batch_hash,
        tx_count: input.transactions.len() as u32,
    });
}
```

Transaction verification includes:
- **Deposits**: Account exists, nonce correct, balance increased
- **Withdrawals**: Sufficient balance, balance decreased correctly
- **Place Order**: Margin requirements met, matching follows price-time priority
- **Cancel Order**: Order exists, belongs to account
- **Liquidations**: Position is underwater at oracle price

### 5. Data Availability (`crates/da/`)

The DA layer ensures all data needed to verify or reconstruct state is available:

```rust
pub struct AppendLog {
    base_path: PathBuf,
    batch_cache: HashMap<u64, Batch>,
    index: DaIndex,
}

impl AppendLog {
    /// Append a batch with state continuity validation
    pub fn append_batch(
        &mut self,
        transactions: Vec<Transaction>,
        pre_root: Hash,
        post_root: Hash,
        batch_hash: Hash,
    ) -> Result<u64, DaError> {
        // Verify chain: pre_root must match previous post_root
        if self.index.last_batch_id > 0 {
            if pre_root != self.index.current_state_root {
                return Err(DaError::StateRootMismatch { ... });
            }
        }

        // Save batch to disk
        let batch = Batch { id, transactions, pre_root, post_root, ... };
        self.save_batch(&batch)?;

        // Update index
        self.index.current_state_root = post_root;
        Ok(batch_id)
    }
}
```

File structure:
```
data/
├── batches/
│   ├── 000001.batch   # Binary serialized batch
│   ├── 000002.batch
│   └── ...
├── proofs/
│   ├── 000001.proof   # Serialized RISC Zero receipt
│   └── ...
└── index.json         # Metadata (last batch, current root, etc.)
```

---

## Running the System

### Prerequisites

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install RISC Zero toolchain (for real ZK proving)
curl -L https://risczero.com/install | bash
rzup install
```

### Build

```bash
# Clone the repository
cd zk-perp

# Build all crates
cargo build --release

# Run tests
cargo test
```

### Start the Sequencer

```bash
# With mock proving (fast, for development)
cargo run --bin sequencer -- --port 8080 --data-dir ./data

# With real ZK proving (slow, ~16 seconds per batch)
cargo run --bin sequencer --features risc0 -- --port 8080 --real-proving
```

### Start the Verifier

```bash
# In another terminal
cargo run --bin verifier -- --port 8081 --data-dir ./data

# With auto-verify on startup
cargo run --bin verifier -- --auto-verify
```

### Example Workflow

```bash
# 1. Register an account
curl -X POST http://localhost:8080/accounts \
  -H "Content-Type: application/json" \
  -d '{"public_key": "0x1234..."}'

# 2. Check account
curl http://localhost:8080/accounts/0

# 3. Submit a deposit transaction
curl -X POST http://localhost:8080/tx \
  -H "Content-Type: application/json" \
  -d '{
    "transaction": {
      "tx": { "Deposit": { "account_id": 0, "asset_id": 0, "amount": "1000000000000000000", "nonce": 1 }},
      "signature": "0x..."
    }
  }'

# 4. Check orderbook
curl http://localhost:8080/markets/0/orderbook

# 5. Check verification status
curl http://localhost:8081/stats
```

---

## API Reference

### Sequencer API (Port 8080)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check with block number |
| `/status` | GET | System stats, DA stats, market info |
| `/accounts` | POST | Register new account |
| `/accounts/:id` | GET | Get account details |
| `/accounts/:id/positions` | GET | Get positions for account |
| `/tx` | POST | Submit signed transaction |
| `/markets/:id/orderbook` | GET | Get order book snapshot |
| `/markets/:id/price` | GET | Get current price |
| `/admin/batch` | POST | Trigger batch processing |

### Verifier API (Port 8081)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/stats` | GET | Verification statistics |
| `/batch/:id` | GET | Get batch verification status |
| `/batch/:id/verified` | GET | Check if batch is verified |
| `/verify` | POST | Verify next pending batch |
| `/verify/all` | POST | Verify all pending batches |

---

## Technical Deep Dive

### Zero-Knowledge Proofs: STARK vs SNARK

This system uses **STARKs** (via RISC Zero) rather than SNARKs:

| Property | STARK | SNARK |
|----------|-------|-------|
| **Trusted setup** | No | Usually yes |
| **Post-quantum secure** | Yes | No |
| **Proof size** | ~100 KB | ~200 bytes |
| **Proving time** | Slower | Faster |
| **Verification time** | Fast | Very fast |

RISC Zero enables writing verification logic in Rust rather than specialized circuit languages, making development much faster.

### How the ZK Proof Works

1. **Execution**: The sequencer executes transactions and records state changes
2. **Witness Generation**: For each state access, a Merkle proof (witness) is recorded
3. **Guest Execution**: The guest program re-executes inside the zkVM, verifying:
   - Every Merkle proof against the claimed roots
   - Business logic (margins, matching, balances)
4. **Proof Generation**: RISC Zero generates a STARK proof of correct execution
5. **Verification**: Anyone can verify the proof in milliseconds

The key insight: **re-execution inside the zkVM** proves the state transition is valid without revealing private data.

### Signature Scheme: Ed25519

Ed25519 uses the Edwards curve:
```
y² = x³ + ax² + x  (where a = 486662)

Base point: G (standard generator)
Private key: k (256-bit scalar)
Public key: K = kG (point on curve)

Signing (message m):
  r = hash(private_key || m)  // deterministic nonce
  R = rG
  s = r + hash(R || K || m) * k
  signature = (R, s)

Verification:
  Check: sG == R + hash(R || K || m) * K
```

### Merkle Tree Optimization: Zero Hashes

For sparse trees, most leaves are empty. Instead of storing all zeros, we precompute "zero hashes":

```rust
// zero_hash[0] = hash of empty leaf
// zero_hash[i] = hash(zero_hash[i-1], zero_hash[i-1])

static ZERO_HASHES: Lazy<[Hash; 256]> = Lazy::new(|| {
    let mut hashes = [[0u8; 32]; 256];
    // hashes[0] = ZERO_HASH (all zeros)
    for i in 1..256 {
        hashes[i] = hash_pair(&hashes[i-1], &hashes[i-1]);
    }
    hashes
});
```

This allows constant-time proofs for non-existent keys.

### State Continuity

Every batch's `pre_state_root` must equal the previous batch's `post_state_root`. This creates an unbroken chain:

```
Genesis  →  Batch 1  →  Batch 2  →  Batch 3
Root₀    →  Root₁    →  Root₂    →  Root₃

Batch 1: pre_root = Root₀, post_root = Root₁
Batch 2: pre_root = Root₁, post_root = Root₂  ✓ (chains correctly)
Batch 3: pre_root = Root₂, post_root = Root₃  ✓
```

Both the DA layer and verifier enforce this continuity.

---

## Security Considerations

### What the ZK Proof Guarantees

- All signatures are valid
- Balances cannot go negative
- Order matching follows price-time priority
- State transitions are deterministic

### What Requires Additional Trust

- **Sequencer Liveness**: The sequencer must be online to process transactions
- **DA Availability**: Batch data must be available for verification
- **Oracle Prices**: Currently uses mock/simulated prices

### Production Hardening (Future Work)

1. **Decentralized Sequencer**: Multiple sequencers with leader election
2. **External DA**: Use Celestia, EigenDA, or similar
3. **Chainlink Integration**: Real price feeds with signature verification
4. **Liquidation Bot**: Automated position monitoring
5. **Bridge Contracts**: Connect to L1 for deposits/withdrawals

---

## License

MIT License - See LICENSE file for details.

---

## Contributing

This is a proof-of-concept implementation. Contributions welcome!

1. Fork the repository
2. Create a feature branch
3. Write tests for new functionality
4. Submit a pull request

---

## Acknowledgments

- [RISC Zero](https://risczero.com) for the zkVM
- [Lighter](https://lighter.xyz) for the architectural inspiration
- The Rust community for excellent cryptographic libraries
