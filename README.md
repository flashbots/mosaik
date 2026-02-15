<p align="center">
  <h1 align="center">mosaik</h1>
  <p align="center">
    A Rust runtime for building self-organizing, leaderless distributed systems.
  </p>
  <p align="center">
    <a href="https://github.com/flashbots/mosaik/blob/main/LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
    <a href="https://crates.io/crates/mosaik"><img alt="Status" src="https://img.shields.io/crates/v/mosaik.svg?color=blue"></a>
    <a href="https://github.com/flashbots/mosaik"><img alt="Rust" src="https://img.shields.io/badge/rust-1.87%2B-blue.svg"></a>
    <a href="https://github.com/flashbots/mosaik"><img alt="Status" src="https://img.shields.io/badge/status-experimental-orange.svg"></a>
  </p>
</p>

---

> [!WARNING]
> **Experimental research software.** Mosaik is under active development. APIs and wire protocols may change without notice. Production quality is targeted for `v1.0`.

## Overview

Mosaik provides primitives for automatic peer discovery, typed pub/sub data streams, availability groups with Raft consensus, and synchronized data stores. Nodes deployed on plain VMs self-organize into a functioning topology using only a secret key, a gossip seed, and role tags — **no orchestration, configuration templates, or DevOps glue required.**

The core claim: when binaries are deployed on arbitrary machines, the network should self-organize, infer its own data-flow graph, and converge to a stable operational topology. This property is foundational for scaling the system, adding new capabilities, and reducing operational complexity.

Mosaik targets trusted, permissioned networks such as L2 chains controlled by a single organization. All members are assumed honest; the system is not Byzantine fault tolerant.

## Core Primitives

### Discovery

Gossip-based peer discovery and catalog synchronization. Nodes announce their presence, capabilities (tags), and available streams/groups/stores. The catalog converges across the network through two complementary protocols:

- **Announcements** — real-time broadcast of peer presence and metadata changes via `iroh-gossip`, with signed entries and periodic re-announcements
- **Catalog Sync** — full bidirectional catalog exchange for initial catch-up and on-demand synchronization

### Streams

Typed async pub/sub data channels connecting producers and consumers across the network. Any serializable type automatically implements `Datum` and can be streamed.

```rust
let network_id = NetworkId::random();

let n0 = Network::new(network_id).await?;
let n1 = Network::new(network_id).await?;
let n2 = Network::new(network_id).await?;

let mut p0 = n0.streams().produce::<Data1>();
let mut c1 = n1.streams().consume::<Data1>();
let mut c2 = n2.streams().consume::<Data1>();

// await topology formation
p0.when().subscribed().minimum_of(2).await;

// produce item (implements futures::Sink)
p0.send(Data1(42)).await?;

// consume item (implements futures::Stream)
assert_eq!(c1.next().await, Some(Data1(42)));
assert_eq!(c2.next().await, Some(Data1(42)));

```

Producers and consumers can be further configured:

```rust
let producer = network.streams()
  .producer::<Data1>()
  .accept_if(|peer| peer.tags().contains("tag1"))
  .online_when(|c| c.minimum_of(2))
  .with_max_consumers(4)
  .build()

let consumer = network.streams()
  .consumer::<Data1>()
  .subscribe_if(|peer| peer.tags().contains("tag2"))
  .build();
```

Key features:

- **Consumer predicates** — conditions for accepting subscribers (auth, attestation, tags)
- **Producer limits** — cap subscriber count or egress bandwidth
- **Online conditions** — define when a producer/consumer is ready (e.g., "online when ≥2 subscribers with tag X are connected")
- **Per-subscription stats** — datums count, bytes count, uptime tracking
- **Backpressure** — slow consumers are disconnected to prevent head-of-line blocking

### Groups

Availability groups — clusters of trusted nodes that coordinate for failover and shared state. Built on a **modified Raft consensus** optimized for trusted environments:

```rust
let group = network.groups()
    .with_key(group_key)
    .with_state_machine(counter)
    .with_storage(InMemory::default())
    .join()
    .await?;

// Wait for leader election
group.when().leader_elected().await;

// Replicate a command
group.execute(Increment(5), Consistency::Strong).await?;
```

Key features:

- **Bonded mesh** — every pair of members maintains a persistent bidirectional connection, authenticated via HMAC-derived proofs of a shared group secret
- **Non-voting followers** — nodes behind the leader's log abstain from votes, preventing stale nodes from disrupting elections
- **Dynamic quorum** — abstaining nodes excluded from the quorum denominator
- **Distributed log catch-up** — lagging followers partition the log range across responders and pull in parallel
- **Replicated state machines** — implement the `StateMachine` trait with `apply(command)` for mutations and `query(query)` for reads
- **Consistency levels** — `Weak` (local, possibly stale) vs `Strong` (forwarded to leader)
- **Reactive conditions** — `when().is_leader()`, `when().is_follower()`, `when().leader_changed()`, `when().is_online()`

### Store *(work in progress)*

Synchronized data store protocol for replaying stream data to late-joining consumers. Enables state accumulation (folding stream deltas into materialized views) and producer-driven catch-up.

## Architecture

Mosaik is built on [iroh](https://github.com/n0-computer/iroh) for QUIC-based peer-to-peer networking with relay support.

```text
┌─────────────────────────────────────────────┐
│                  Network                    │
│                                             │
│  ┌───────────┐  ┌─────────┐  ┌───────────┐  │
│  │ Discovery │  │ Streams │  │  Groups   │  │
│  │           │  │         │  │           │  │
│  │ Announce  │  │Producer │  │  Bonds    │  │
│  │ Catalog   │  │Consumer │  │  Raft     │  │
│  │ Sync      │  │Status   │  │  RSM      │  │
│  └───────────┘  └─────────┘  └───────────┘  │
│                                             │
│  ┌─────────┐  ┌──────────────────────────┐  │
│  │  Store  │  │     Transport (iroh)     │  │
│  │  (WIP)  │  │  QUIC · Relay · mDNS     │  │
│  └─────────┘  └──────────────────────────┘  │
└─────────────────────────────────────────────┘
```

**Key design patterns:**

- **Worker-loop architecture** — each subsystem spawns long-running tasks that own mutable state, controlled via command channels. Public handles are cheap, cloneable wrappers.
- **Type-safe links** — `Link<P: Protocol>` provides typed, bidirectional, framed transport with compile-time ALPN correctness.
- **Blake3 identifiers** — `NetworkId`, `PeerId`, `StreamId`, `GroupId`, `StoreId`, and `Tag` are all 32-byte blake3 hashes.
- **Cancellation hierarchy** — network → subsystem → per-connection graceful shutdown via `CancellationToken`.
- **Immutable snapshots** — catalog state uses `im::OrdMap` for cheap, thread-safe cloning.

## Repository Layout

| Path              | Description                                                           |
| ----------------- | --------------------------------------------------------------------- |
| `src/`            | Core library — all shared primitives, protocols, and APIs             |
| `src/discovery/`  | Peer discovery, announcement, and catalog synchronization             |
| `src/streams/`    | Typed pub/sub: producers, consumers, status conditions, criteria      |
| `src/groups/`     | Availability groups: bonds, Raft consensus, replicated state machines |
| `src/store/`      | Synchronized data store protocol (WIP)                                |
| `src/network/`    | Transport layer, connection management, typed links                   |
| `src/primitives/` | Identifiers (`Digest`), formatting helpers, async work queues         |
| `src/builtin/`    | Built-in implementations (`NoOp` state machine, `InMemory` storage)   |
| `src/cli/`        | CLI tool for bootstrapping and inspecting nodes                       |
| `tests/`          | Integration tests organized by subsystem                              |
| `docs/`           | mdBook with protocol documentation                                    |

## Getting Started

### Prerequisites

- Rust toolchain **≥ 1.87** — install with `rustup toolchain install 1.87`
- Optional: `cargo install mdbook` for building documentation

### Testing

```bash
# Run all integration tests
cargo test --test basic

# Verbose test output with tracing
TEST_TRACE=on cargo test --test basic groups::leader::is_elected
TEST_TRACE=trace cargo test --test basic groups::leader::is_elected
```

## Roadmap

### Stage 1: Primitives *(current)*

Core primitives for building self-organized distributed systems in trusted, permissioned networks.

- [x] **Discovery** — gossip announcements, catalog sync, tags
- [x] **Streams** — producers, consumers, predicates, limits, online conditions, stats
- [ ] **Groups** — membership, shared state, failover, load balancing
- [ ] **Preferences** — ranking producers by latency, geo-proximity
- [ ] **Diagnostics** — network inspection, automatic metrics, developer debug tools

### Stage 2: Trust & Privacy

Advanced stream subscription authorization, attested sandbox runtimes, trust corridors, etc.

### Stage 3: Decentralization & Permissionlessness

Extending the system beyond trusted, single-operator environments.

## Contributing

- **Commits:** concise, imperative subjects referencing the component (e.g., *"Progress on pubsub semantics"*)
- **PRs:** include a summary, linked issues/RFCs, and a checklist confirming `cargo build`, `cargo test`, `cargo clippy`, and `cargo fmt` pass. Attach logs or screenshots for user-visible changes.
- **Tests:** add or extend integration coverage for behavioral changes. Note remaining gaps or follow-up work in the PR body. Async tests use `#[tokio::test(flavor = "multi_thread")]`.
- **Docs:** update `docs/` when protocol semantics or traces change.

## License

MIT — see [LICENSE](LICENSE) for details.
