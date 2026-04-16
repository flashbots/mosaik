<p align="center">
  <h1 align="center">mosaik</h1>
  <p align="center">
    A Rust runtime for building self-organizing, leaderless distributed systems.
  </p>
  <p align="center">
    <a href="https://github.com/flashbots/mosaik/blob/main/LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
    <a href="https://github.com/flashbots/mosaik"><img alt="Status" src="https://img.shields.io/badge/status-experimental-orange.svg"></a>
    <a href="https://crates.io/crates/mosaik"><img alt="crates.io" src="https://img.shields.io/crates/v/mosaik.svg?color=blue"></a>
    <a href="https://docs.rs/mosaik"><img alt="docs.rs" src="https://docs.rs/mosaik/badge.svg"></a>
    <a href="http://docs.mosaik.world"><img alt="Documentation" src="https://img.shields.io/badge/documentation-8A2BE2"></a>
  </p>
</p>

---

> [!WARNING]
> **Experimental research software.** APIs and wire protocols may change without notice. Production quality is targeted for `v1.0`.

## What is mosaik?

Mosaik provides primitives for **peer discovery**, **typed pub/sub streams**, **Raft consensus groups**, and **replicated data structures**. Nodes deployed on plain VMs self-organize into a functioning topology using only a secret key, a gossip seed, and role tags — no orchestration, configuration templates, or DevOps glue required.

All resource identifiers are **intent-addressed**: derived from human-readable strings via blake3 hashing. Two nodes that independently declare the same name converge on the same identifier without prior coordination.

Mosaik has first-class support for **Trusted Execution Environments** — nodes inside Intel TDX enclaves can generate hardware-attested identity tickets, and peers can require valid attestation before accepting connections.

```text
┌────────────────────────────────────────────────────────┐
│                        Network                         │
│                                                        │
│  ┌────────────┐  ┌───────────┐  ┌───────────┐          │
│  │ Discovery  │  │ Streams   │  │  Groups   │          │
│  │            │  │           │  │           │          │
│  │ Announce   │  │ Producer  │  │  Bonds    │          │
│  │ Catalog    │  │ Consumer  │  │  Raft     │          │
│  │ Sync       │  │ Status    │  │  RSM      │          │
│  └────────────┘  └───────────┘  └───────────┘          │
│                                                        │
│  ┌─────────────────┐  ┌──────────┐  ┌───────────────┐  │
│  │   Collections   │  │ Tickets  │  │Transport      │  │
│  │                 │  │          │  │               │  │
│  │ Map · Vec · Set │  │ JWT      │  │ QUIC · Relay  │  │
│  │   Cell · Once   │  │ TDX      │  │ mDNS · pkarr  │  │
│  │  PriorityQueue  │  │          │  │               │  │
│  └─────────────────┘  └──────────┘  └───────────────┘  │
└────────────────────────────────────────────────────────┘
```

## Quick taste

```rust
use mosaik::*;

let network_id = NetworkId::from("my-app");

// Node 0: produce a typed stream
let n0 = Network::new(network_id).await?;
let mut producer = n0.streams().produce::<SensorReading>();
producer.when().subscribed().minimum_of(1).await;
producer.send(SensorReading { temp: 21.5 }).await?;

// Node 1: consume the same stream — discovered automatically
let n1 = Network::new(network_id).await?;
let mut consumer = n1.streams().consume::<SensorReading>();
let reading = consumer.next().await.unwrap();
```

```rust
// Replicated collections — same ergonomics as std types
let map = mosaik::collections::Map::<String, u64>::new(&network, store_id);
map.when().online().await;
map.insert("alice".into(), 100).await?;

// Read-only replica on another node
let reader = mosaik::collections::Map::<String, u64>::reader(&network, store_id);
reader.when().online().await;
assert_eq!(reader.get(&"alice".into()), Some(100));
```

See the [documentation](http://docs.mosaik.world) for the full API guide covering streams, groups, collections, tickets, and TEE support.

## Getting started

**Prerequisites:** Rust toolchain **>= 1.93**

```bash
cargo add mosaik
```

### Examples

```bash
# P2P group chat — start several instances to chat
cargo run --example group-chat -- --nickname Alice

# Distributed order-matching engine
cargo run -p orderbook

# DHT bootstrap discovery
cargo run --example bootstrap

# Distributed rate limiter
cargo run --example rate-limiter
```

### Running tests

```bash
# All integration tests (single-threaded required)
TEST_TRACE=on cargo test --test all -- --test-threads=1

# A specific module
TEST_TRACE=on cargo test --test all collections::map -- --test-threads=1

# Verbose tracing
TEST_TRACE=trace cargo test --test all groups::leader::is_elected

# Extend timeouts for slow networks
TIME_FACTOR=3 TEST_TRACE=on cargo test --test all groups::leader::is_elected
```

## Observability

Every subsystem emits metrics via the [`metrics`](https://docs.rs/metrics) facade. Enable the built-in Prometheus exporter:

```rust
let network = Network::builder("my-app")
    .with_prometheus_addr("0.0.0.0:9000".parse().unwrap())
    .build()
    .await?;
```

See the [Metrics reference](./book/src/reference/metrics.md) for the full catalog.

## Roadmap

### Stage 1: Primitives

- [x] **Discovery** — gossip announcements, catalog sync, DHT bootstrap, tags
- [x] **Streams** — producers, consumers, predicates, limits, online conditions, stats
- [x] **Groups** — Raft consensus, membership, shared state, failover
- [x] **Collections** — Map, Vec, Set, Cell, Once, PriorityQueue
- [x] **Tickets** — JWT and TDX-based peer authentication with expiration-aware disconnect
- [x] **TEE** — Intel TDX support for hardware-attested identity and access control
- [x] **Metrics** — built-in observability via `metrics` with optional Prometheus exporter
- [ ] **Preferences** — ranking producers by latency, geo-proximity
- [ ] **Diagnostics** — network inspection, developer debug tools

### Stage 2: Trust & Privacy

- [x] **TEE-based authorization** — gate access to streams, groups, and collections to hardware-attested peers
- [ ] **Privacy Corridors** — end-to-end data isolation across chains of nodes and services
- [ ] **Trust zones** — network partitions enforcing integrity and privacy guarantees, backed by TEE attestation

### Stage 3: Decentralization & Permissionlessness

## Contributing

- **Commits:** concise, imperative subjects referencing the component (e.g., *"Groups: Support for user-provided encoding"*)
- **PRs:** include a summary, linked issues/RFCs, and a checklist confirming `cargo build`, `cargo test`, `cargo clippy`, and `cargo +nightly fmt` pass.
- **Tests:** add or extend integration coverage for behavioral changes.

## License

MIT — see [LICENSE](LICENSE) for details.
