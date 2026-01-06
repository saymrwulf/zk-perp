# Quick Start Guide

Get zk-perp running in under 5 minutes.

## Prerequisites

- Rust 1.75+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- Git

## Build & Run

```bash
# Clone and build
git clone https://github.com/your-repo/zk-perp.git
cd zk-perp
cargo build --release

# Start sequencer (mock proving, instant)
cargo run --bin sequencer -- --port 8080

# In another terminal, start verifier
cargo run --bin verifier -- --port 8081
```

## Test the API

### 1. Check Health

```bash
curl http://localhost:8080/health | jq
```

Response:
```json
{
  "success": true,
  "data": {
    "status": "healthy",
    "version": "0.1.0",
    "block_number": 0,
    "pending_transactions": 0
  }
}
```

### 2. Register an Account

```bash
# Generate a keypair (in a real app)
# For testing, use a dummy public key:
curl -X POST http://localhost:8080/accounts \
  -H "Content-Type: application/json" \
  -d '{
    "public_key": "0000000000000000000000000000000000000000000000000000000000000001"
  }' | jq
```

Response:
```json
{
  "success": true,
  "data": {
    "account_id": 0,
    "public_key": "0000000000000000000000000000000000000000000000000000000000000001"
  }
}
```

### 3. Check Account

```bash
curl http://localhost:8080/accounts/0 | jq
```

### 4. Get Order Book

```bash
# BTC-USDC market (market_id = 0)
curl "http://localhost:8080/markets/0/orderbook?depth=10" | jq
```

### 5. Get Market Price

```bash
curl http://localhost:8080/markets/0/price | jq
```

Response:
```json
{
  "success": true,
  "data": {
    "id": 0,
    "name": "BTC-USDC",
    "base_asset": "BTC",
    "quote_asset": "USDC",
    "price": 5000000000000
  }
}
```

### 6. Check Verifier Status

```bash
curl http://localhost:8081/stats | jq
```

## Using Real ZK Proofs

For real zero-knowledge proofs (slower, ~16 seconds per batch):

```bash
# Install RISC Zero toolchain
curl -L https://risczero.com/install | bash
rzup install

# Run with real proving
cargo run --bin sequencer --features risc0 -- --real-proving
```

## Run Tests

```bash
# Run all tests
cargo test

# Run specific crate tests
cargo test -p zk-perp-core
cargo test -p zk-perp-orderbook
cargo test -p zk-perp-verifier

# Run with real RISC Zero proving (slow)
cargo test -p zk-perp-host --test integration -- --ignored
```

## Project Structure Overview

```
zk-perp/
├── crates/
│   ├── core/       # Types, crypto, Merkle trees
│   ├── orderbook/  # Order matching engine
│   ├── state/      # Global state management
│   ├── oracle/     # Price feeds
│   └── da/         # Data availability layer
├── methods/        # RISC Zero guest (ZK circuit)
├── host/           # Proof generation
├── sequencer/      # Main server
└── verifier/       # Verification node
```

## Next Steps

1. Read the [README](../README.md) for a complete overview
2. Explore the [Architecture](ARCHITECTURE.md) for technical details
3. Check the API reference in the README
4. Try writing a client that submits transactions
