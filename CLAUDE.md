# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build the library
cargo build

# Run all integration tests (single-threaded required)
TEST_TRACE=on cargo test --test basic -- --test-threads=1

# Run a specific test or module
TEST_TRACE=on cargo test --test basic collections::map -- --test-threads=1

# Run tests with full tracing output
TEST_TRACE=trace cargo test --test basic groups::leader::is_elected

# Extend timeouts for slow networks
TIME_FACTOR=3 TEST_TRACE=on cargo test --test basic groups::leader::is_elected

# Lint
cargo clippy

# Format (uses unstable nightly features via rustfmt.toml)
cargo +nightly fmt

# Check formatting
cargo +nightly fmt --check
```

Integration tests all live under the single test harness `tests/basic.rs`. Use `--test basic` to select it.

## Architecture

Mosaik is a Rust library (`src/lib.rs`) providing a runtime for self-organizing, leaderless distributed systems. It is built on [iroh](https://github.com/n0-computer/iroh) (QUIC-based p2p transport with relay support).

### Subsystems

- **`src/network/`** — `Network` and `NetworkBuilder` are the primary entrypoints. A `Network` wraps an iroh node, bootstraps the transport, and provides access to all subsystems via `.discovery()`, `.streams()`, `.groups()`. `LocalNode` wraps the iroh node identity and address.

- **`src/discovery/`** — Gossip-based peer discovery and catalog synchronization. Two protocols:
  - `announce.rs` — broadcasts presence/metadata via iroh-gossip with signed entries
  - `sync.rs` / `catalog.rs` — full bidirectional catalog exchange for catch-up
  - `dht.rs` — automatic DHT bootstrap via pkarr
  - `entry.rs` — catalog entry type (PeerId, tags, streams, groups advertised by a peer)

- **`src/streams/`** — Typed async pub/sub channels (implements `futures::Sink` / `futures::Stream`). Key types:
  - `Producer<T>` — publishes data, manages subscriber connections, enforces predicates/limits
  - `Consumer<T>` — subscribes to a remote producer
  - `StreamId` is a `Digest` (blake3 hash), derived by default from the datum type
  - `StreamDef` — declarative stream descriptor primitive
  - `status/` — reactive conditions (`when().subscribed().minimum_of(2)`, etc.)

- **`src/groups/`** — Availability groups using a modified Raft consensus:
  - `bond/` — persistent bidirectional authenticated connections between all group member pairs
  - `raft/` — Raft roles (`leader.rs`, `follower.rs`, `candidate.rs`), protocol, shared state
  - `machine/` — `StateMachine` trait (implement `apply(cmd)` + `query(q)` + `state_sync()`)
  - `replay/` — `LogReplaySync` for state machine snapshot/catch-up
  - `key.rs` — `GroupKey` and `GroupId` (GroupId is derived from key + config + state machine signature)
  - `storage.rs` — pluggable log storage (built-in: `InMemory`)

- **`src/collections/`** — Replicated data structures layered on Groups:
  - `Map<K,V>`, `Vec<T>`, `Set<T>`, `Register<T>`, `PriorityQueue<P,K,V>`
  - Each has a **writer** (mutates via Raft commands) and a **reader** (read-only replica)
  - Mutations return a `Version`; readers support `.when().reaches(ver)` for convergence checks
  - `StoreId` identifies a collection instance on the network

- **`src/primitives/`** — Shared types: `Datum` (any `Serialize + DeserializeOwned + Clone`), `Digest` (blake3 hash used as IDs), `Tag`, `UniqueId`, `AsyncWorkQueue`, serialization helpers (postcard encoding by default)

- **`src/macros/`** — `mosaik-macros` proc-macro crate providing `UniqueId` compile-time derivation

### Key Patterns

- **`when()` conditions** — all major types expose a `.when()` builder returning reactive futures that resolve when topology/consensus state matches criteria (e.g., `producer.when().subscribed().minimum_of(2).await`).
- **`NetworkId`** — a `Digest` derived from a string name; nodes on the same network share the same `NetworkId` and only connect to peers with the same id.
- **Serialization** — postcard is used for wire encoding; `Datum` is auto-implemented for any `Serialize + DeserializeOwned + Clone + Send + Sync + 'static`.
- **`tests/utils/`** — test helpers including `timeout_s()` and `discover_all()` for forcing catalog sync between nodes in tests.

### Code Style

- Rust edition 2024, MSRV 1.89
- Tabs for indentation (tab_spaces = 2, hard_tabs = true per `rustfmt.toml`)
- Max line width 80, imports grouped as `StdExternalCrate`
- Clippy pedantic + nursery warnings enabled; `wildcard_imports` and `future_not_send` are allowed
- Commit messages: concise imperative, prefixed with component (e.g., `"Groups: Support for user-provided encoding"`)
