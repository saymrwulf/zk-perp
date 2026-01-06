# Local Claude Code installation

- No `~/.code` directory found; Claude Code data appears under `~/.claude/` and `~/.claude.json`.
- `~/.claude.json` shows `installMethod: "native"`, `autoUpdates: false`, `theme: "light"`, and `lastReleaseNotesSeen: "2.0.76"`.
- Claude Code state directories present: `debug/`, `downloads/`, `file-history/`, `history.jsonl`, `ide/`, `plans/`, `plugins/`, `projects/`, `session-env/`, `shell-snapshots/`, `statsig/`, `todos/`.
- Project metadata includes `/Users/oho/GitClone/ClaudeCodeProjects` with trust dialog accepted and empty allowed tools list.

# zk-perp project overview (analogy: Lighter, with RISC Zero instead of Plonky2)

`/Users/oho/GitClone/ClaudeCodeProjects/zk-perp` is a Rust workspace implementing a perp DEX stack with:
- Core types, Merkle/Hypertree state, and transaction definitions.
- An orderbook + matching engine.
- A sequencer with HTTP API, batching, and proof generation.
- A DA (data availability) append-only log.
- A RISC Zero zkVM guest program and host prover.
- A verifier that consumes DA proofs and advances verified state.

Workspace members are declared in `Cargo.toml`:
- `crates/core`, `crates/orderbook`, `crates/state`, `crates/oracle`, `crates/da`
- `methods` (RISC Zero guest build)
- `host` (prover)
- `sequencer` (API + execution)
- `verifier`

# ZK proving flow (RISC Zero)

- `methods/` uses `risc0-build` to embed the guest program (`methods/build.rs`).
- `methods/src/lib.rs` exports `ZK_PERP_GUEST_ELF`/`ZK_PERP_GUEST_ID` when built with the `risc0` feature, otherwise stub values.
- `methods/guest/src/main.rs` reads a `BatchInput`, verifies transactions when witnesses are present, computes a batch hash, and commits a `BatchOutput` to the journal.
- `host/src/lib.rs` exposes a `Prover` that can run in mock mode (no proof) or real mode (RISC Zero `default_prover`).
- `sequencer` defaults to `use_mock_prover: true` in `SequencerConfig`.

Notable caveats in the current guest logic:
- Transactions are only deeply verified when witnesses are present; otherwise they are accepted for testing.

# Core state model

- The state commitment uses a 6-tree Hypertree (`crates/core/src/merkle/hypertree.rs`):
  1) Accounts
  2) Account Orders
  3) Orderbook
  4) Positions
  5) Pools
  6) System (markets/oracles/config)
- Each tree is a Sparse Merkle Tree; the combined root is a hash of the six roots.
- There is a TODO in the sparse Merkle proof verification test (`crates/core/src/merkle/tree.rs`) indicating proof verification logic is not finalized.

# Transactions and signatures

- Transaction types include Deposit, Withdraw, PlaceOrder, CancelOrder, Liquidate, and UpdateOracle (`crates/core/src/transactions/mod.rs`).
- Signatures use Ed25519 via `ed25519-dalek` (`crates/core/src/crypto.rs`).
- Transaction hashes are SHA-256 over bincode-serialized payloads.

# Sequencer, execution, and API

- The sequencer validates signatures (optional), executes transactions against `GlobalState`, batches pending transactions, triggers proving, and writes to the DA log (`sequencer/src/lib.rs`).
- `GlobalState` maintains the hypertree, in-memory caches (accounts/positions), and per-market orderbooks, plus oracle prices (`crates/state/src/lib.rs`).
- The HTTP API (Axum) exposes health, status, account registration, balances, orderbook snapshots, positions, tx submission, and batch triggers (`sequencer/src/api.rs`).

# Orderbook and matching

- Orderbook uses `BTreeMap` price levels with FIFO at each level, giving price-time priority (`crates/orderbook/src/book.rs`).
- Matching engine is used by state transition logic for order execution (`crates/state/src/lib.rs`).

# Oracle

- Oracle module includes `MockOracle`, `SimulatedOracle`, and a `PriceSource` trait for price feeds (`crates/oracle/src/lib.rs`).

# DA layer (append-only log)

- DA stores batches and proofs on disk with an index (`crates/da/src/lib.rs`).
- The layout is `batches/`, `proofs/`, and `index.json` under the configured data directory.

# Verifier

- The verifier pulls batches from DA, verifies proofs via the host prover, and advances its verified root (`verifier/src/lib.rs`).

# Build artifacts

- `target/` contains compiled artifacts, including a RISC Zero guest build under `target/riscv-guest/...`, indicating the guest has been built before.
