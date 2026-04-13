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
> **Experimental research software.** Mosaik is under active development. APIs and wire protocols may change without notice. Production quality is targeted for `v1.0`.

# Overview

Mosaik provides primitives for automatic peer discovery, typed pub/sub data streams, consensus groups with Raft, and replicated data structures. Nodes deployed on plain VMs self-organize into a functioning topology using only a secret key, a gossip seed, and role tags — **no orchestration, configuration templates, or DevOps glue required.**

All resource identifiers (networks, streams, collections, groups) are **intent-addressed**: derived from human-readable strings via blake3 hashing. Two nodes that independently declare the same name converge on the same identifier without prior coordination — enabling forward references, independent deployment, and coordination-free bootstrapping.

The core claim: when binaries are deployed on arbitrary machines, the network should self-organize, infer its own data-flow graph, and converge to a stable operational topology. This property is foundational for scaling the system, adding new capabilities, and reducing operational complexity.

Mosaik has first-class support for **Trusted Execution Environments (TEEs)** — nodes running inside Intel TDX enclaves can generate hardware-attested identity tickets, and other nodes can require valid attestation before accepting connections. This enables cryptographic proof of what software each peer is running, without out-of-band coordination.

Mosaik initially targets trusted, permissioned networks such as L2 chains controlled by a single organization. All members are assumed honest; the system is not Byzantine fault tolerant. For stronger integrity guarantees, groups can optionally require hardware attestations (e.g. Intel TDX) to cryptographically prove that every member is running the expected software.

> [!TIP]
> To see mosaik in action, browse the integration tests in the [`tests`](tests/) directory or run one of the [examples](examples/):
>
> ```bash
> # p2p group chat — start several instances to chat
> cargo run --example group-chat -- --nickname Alice
>
> # distributed order-matching engine
> cargo run -p orderbook
> ```

# Core Primitives

## Discovery

Gossip-based peer discovery and catalog synchronization. Nodes announce their presence, capabilities (tags), and available streams/groups/stores. The catalog converges across the network through two complementary protocols:

- **Announcements** — real-time broadcast of peer presence and metadata changes via `iroh-gossip`, with signed entries and periodic re-announcements
- **Catalog Sync** — full bidirectional catalog exchange for initial catch-up and on-demand synchronization
- **Automatic DHT bootstrap** - See the [DHT Bootstrap](./book/src/subsystems/discovery/dht-bootstrap.md) sub-chapter for details on the automatic discovery mechanism.

Discovery is largely transparent and ships with sensible defaults. To spin up a node on a given network, just provide a `NetworkId`:

```rust
use mosaik::*;

let network_id = NetworkId::random()
let node = Network::new(network_id).await?;
```

For finer control, use `NetworkBuilder` to customize discovery settings such as tags or bootstrap peers:

```rust
let n0 = Network::builder(network_id)
  .with_discovery(
    discovery::Config::builder()
      .with_tags("tag1")
      .with_tags(["tag2", "tag3"])
      .with_bootstrap([peer_id1, peer_id2])
  ).build().await?;
```

## Streams

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
  .with_stream_id("custom.stream.id")
  .require(|peer| peer.tags().contains("tag1"))
  .online_when(|c| c.minimum_of(2))
  .with_max_consumers(4)
  .build()

let consumer = network.streams()
  .consumer::<Data1>()
  .with_stream_id("custom.stream.id")
  .require(|peer| peer.tags().contains("tag2"))
  .build();
```

Key features:

- **Consumer predicates** — conditions for accepting subscribers (auth, attestation, tags)
- **Producer limits** — cap subscriber count or egress bandwidth
- **Online conditions** — define when a producer/consumer is ready (e.g., "online when ≥2 subscribers with tag X are connected")
- **Per-subscription stats** — datums count, bytes count, uptime tracking
- **Backpressure** — slow consumers are disconnected to prevent head-of-line blocking

## Tickets

Opaque, typed credentials for peer authentication. Tickets are attached to a node's discovery entry and propagated via gossip — no separate auth round-trip is needed. Streams, groups, and collections can all require valid tickets before accepting connections.

Mosaik ships a built-in JWT validator (`Jwt`) and ticket builder (`JwtTicketBuilder`) supporting HMAC (HS256/384/512), ECDSA (ES256, ES256K, ES384, ES512), and EdDSA (Ed25519). For hardware-backed authentication, the `Tdx` validator verifies Intel TDX attestation quotes.

```rust
use mosaik::tickets::{Jwt, Hs256, JwtTicketBuilder, Expiration};

// Validator: verify incoming JWTs with a shared HMAC key
let validator = Jwt::with_key(Hs256::new(secret))
    .allow_issuer("my-app");

let producer = network.streams()
    .producer::<MyDatum>()
    .require_ticket(validator)
    .build()?;

// Issuer: sign and attach a JWT ticket for this peer
let builder = JwtTicketBuilder::new(Hs256::new(secret))
    .issuer("my-app");
let ticket = builder.build(&network.local().id(), Expiration::At(expiry));
network.discovery().add_ticket(ticket);
```

Key features:

- **Built-in JWT support** — HMAC symmetric and ECDSA/EdDSA asymmetric algorithms
- **Expiration-aware** — the runtime proactively disconnects peers when tickets expire
- **Multiple validators** — require several independent credentials (AND-composed)
- **Group identity** — validator configuration is mixed into the group ID derivation
- **TDX attestation** — hardware-backed proof of code integrity via Intel TDX

## Collections

Replicated, eventually consistent data structures built on top of Groups. Each collection is backed by a Raft-replicated state machine. Every collection has a **writer** (can mutate) and a **reader** (read-only replica that tracks the writer's state).

All mutations return a `Version` that can be awaited on readers via `when().reaches(ver)` to confirm convergence.

### `Map<K, V>`

Replicated unordered key-value store.

```rust
let store_id = StoreId::random();

// On the writer node
let map = mosaik::collections::Map::<String, u64>::new(&network, store_id);
map.when().online().await;

map.insert("alice".into(), 100).await?;
map.extend(vec![("bob".into(), 200), ("carol".into(), 300)]).await?;

assert_eq!(map.get(&"alice".into()), Some(100));
assert!(map.contains_key(&"bob".into()));
assert_eq!(map.len(), 3);

map.remove("carol".into()).await?;

// On a reader node
let reader = mosaik::collections::Map::<String, u64>::reader(&network, store_id);
reader.when().online.await;
assert_eq!(reader.get(&"bob".into()), Some(200));

let ver = map.clear().await?;
reader.when().reaches(ver).await;
assert!(reader.is_empty());
```

### `Vec<T>`

Replicated ordered, index-addressable sequence.

```rust
let store_id = StoreId::random();

let vec_writer = mosaik::collections::Vec::<u64>::writer(&network, store_id);
vec_writer.when().online().await;

// Push to front and back
vec.push_back(42).await?;
vec.push_front(10).await?;
vec.extend([7, 13, 21]).await?;

// On a reader node
let vec_reader = mosaik::collections::Vec::<u64>::reader(&network, store_id);
vec_reader.when().online().await;

assert_eq!(vec_reader.get(0), Some(10));
assert_eq!(vec_reader.get(1), Some(42));
assert_eq!(vec_reader.get(2), Some(7));
assert_eq!(vec_reader.get(3), Some(13));

let ver = vec_writer.clear().await?;
vec_reader.when().reaches(ver).await;
assert!(vec_reader.is_empty());

```

### `Set<T>`

Replicated unordered collection of unique values.

```rust
let store_id = StoreId::random();

let set = mosaik::collections::Set::<u64>::new(&network, store_id);
set.when().online().await;

set.insert(42).await?;
set.extend([7, 13, 21]).await?;
set.insert(42).await?;       // duplicate — len stays 4
set.remove(7).await?;

// On a reader node
let reader = mosaik::collections::Set::<u64>::reader(&network, store_id);
reader.when().online().await;
assert!(reader.contains(&13));
assert!(!reader.contains(&7));
assert_eq!(reader.len(), 3);
```

### `Cell<T>`

Replicated single-value cell. Holds at most one value at a time — writing replaces the previous value. This is the distributed equivalent of a `tokio::sync::watch` channel.

```rust
let store_id = StoreId::random();

let reg = mosaik::collections::Cell::<String>::new(&network, store_id);
reg.when().online().await;

reg.write("v1".into()).await?;
assert_eq!(reg.read(), Some("v1".into()));

// Overwrite replaces the previous value
reg.write("v2".into()).await?;
assert_eq!(reg.read(), Some("v2".into()));

// On a reader node
let reader = mosaik::collections::Cell::<String>::reader(&network, store_id);
reader.when().online().await;
assert_eq!(reader.read(), Some("v2".into()));

let ver = reg.clear().await?;
reader.when().reaches(ver).await;
assert!(reader.is_empty());
```

### `Once<T>`

Replicated write-once cell. Holds at most one value — once a value has been set, subsequent writes are silently ignored. This is the distributed equivalent of a `tokio::sync::OnceCell`.

```rust
let store_id = StoreId::random();

let once = mosaik::collections::Once::<String>::new(&network, store_id);
once.when().online().await;

// First write — permanently stored
once.write("genesis".into()).await?;
assert_eq!(once.read(), Some("genesis".into()));

// Second write — silently ignored
once.write("overwrite-attempt".into()).await?;
assert_eq!(once.read(), Some("genesis".into())); // still "genesis"

// On a reader node
let reader = mosaik::collections::Once::<String>::reader(&network, store_id);
reader.when().online().await;
assert_eq!(reader.read(), Some("genesis".into()));
```

### `PriorityQueue<P, K, V>`

Replicated double-ended priority queue. Each entry has a priority, a unique key, and a value. Supports efficient min/max access, key-based lookups, priority updates, and range removals.

```rust
let store_id = StoreId::random();

let pq = mosaik::collections::PriorityQueue::<u64, String, String>::new(&network, store_id);
pq.when().online().await;

// Insert entries with (priority, key, value)
pq.insert(10, "alice".into(), "payload_a".into()).await?;
pq.insert(30, "bob".into(), "payload_b".into()).await?;
pq.insert(20, "carol".into(), "payload_c".into()).await?;

assert_eq!(pq.get_min(), Some((10, "alice".into(), "payload_a".into())));
assert_eq!(pq.get_max(), Some((30, "bob".into(), "payload_b".into())));
assert_eq!(pq.get_priority(&"carol".into()), Some(20));

// Update priority without changing value
pq.update_priority(&"alice".into(), 50).await?;
assert_eq!(pq.max_priority(), Some(50));

// Range removal — all standard range syntaxes work
pq.remove_range(..25).await?;   // remove priorities below 25
pq.remove_range(40..).await?;   // remove priorities >= 40
pq.remove_range(10..=30).await?; // remove priorities in [10, 30]

// On a reader node
let reader = mosaik::collections::PriorityQueue::<u64, String, String>::reader(&network, store_id);
reader.when().online().await;

// Iterate in priority order
for (priority, key, value) in reader.iter_asc() {
    println!("{priority}: {key} => {value}");
}
```

## Groups

Clusters of trusted nodes that coordinate for failover and shared state. Built on a **Raft consensus** optimized for trusted environments:

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

To build your own replicated state machine, implement the `StateMachine` trait. Here's a minimal distributed counter:

```rust
use mosaik::{groups::*, UniqueId};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CounterCommand {
    Increment(u64),
    Decrement(u64),
    Reset,
}

#[derive(Default)]
struct Counter {
    value: u64,
}

impl StateMachine for Counter {
    type Command = CounterCommand;
    type Query = ();
    type QueryResult = u64;
    type StateSync = LogReplaySync<Self>;

    fn signature(&self) -> UniqueId {
        UniqueId::from("my_counter")
    }

    fn apply(&mut self, command: Self::Command) {
        match command {
            CounterCommand::Increment(n) => self.value += n,
            CounterCommand::Decrement(n) => self.value = self.value.saturating_sub(n),
            CounterCommand::Reset => self.value = 0,
        }
    }

    fn query(&self, (): Self::Query) -> Self::QueryResult {
        self.value
    }

    fn state_sync(&self) -> Self::StateSync {
        LogReplaySync::default()
    }
}
```

Then join a group with your state machine and replicate commands across the network:

```rust
let group = network.groups()
    .with_key(group_key)
    .with_state_machine(Counter::default())
    .join();

group.when().online().await;
group.execute(CounterCommand::Increment(5)).await?;
```

Key features:

- **Bonded mesh** — every pair of members maintains a persistent bidirectional connection, authenticated via HMAC-derived proofs of a shared group secret
- **Non-voting followers** — nodes behind the leader's log abstain from votes, preventing stale nodes from disrupting elections
- **Dynamic quorum** — abstaining nodes excluded from the quorum denominator
- **Distributed log catch-up** — lagging followers partition the log range across responders and pull in parallel
- **Replicated state machines** — implement the `StateMachine` trait with `apply(command)` for mutations and `query(query)` for reads
- **Consistency levels** — `Weak` (local, possibly stale) vs `Strong` (forwarded to leader)
- **Reactive conditions** — `when().is_leader()`, `when().is_follower()`, `when().leader_changed()`, `when().is_online()`

# Architecture

Mosaik is built on [iroh](https://github.com/n0-computer/iroh) for QUIC-based peer-to-peer networking with relay support.

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
│  │   Collections   │  │   TEE    │  │Transport      │  │
│  │                 │  │          │  │               │  │
│  │ Map · Vec · Set │  │ TDX      │  │ QUIC · Relay  │  │
│  │   Cell · Once   │  │          │  │ mDNS · pkarr  │  │
│  │  PriorityQueue  │  │          │  │               │  │
│  └─────────────────┘  └──────────┘  └───────────────┘  │
└────────────────────────────────────────────────────────┘
```

# Repository Layout

| Path               | Description                                                                      |
| ------------------ | -------------------------------------------------------------------------------- |
| `src/`             | Core library — all shared primitives, protocols, and APIs                        |
| `src/discovery/`   | Peer discovery, announcement, and catalog synchronization                        |
| `src/streams/`     | Typed pub/sub: producers, consumers, status conditions, criteria                 |
| `src/groups/`      | Consensus groups: bonds, Raft consensus, replicated state machines               |
| `src/collections/` | Replicated data structures: `Map`, `Vec`, `Set`, `Cell`, `Once`, `PriorityQueue` |
| `src/tickets/`     | Auth tickets: `Ticket`, `TicketValidator`, built-in JWT and TDX validators       |
| `src/network/`     | Transport layer, connection management, typed links                              |
| `src/primitives/`  | Identifiers, formatting helpers, async work queues, etc.                         |
| `src/tee/`         | TEE support: Intel TDX attestation, validation, and image building               |
| `tests/`           | Integration tests organized by subsystem                                         |

# Getting Started

## Prerequisites

- Rust toolchain **≥ 1.93**.

## Usage

```bash
cargo add mosaik
```

or in `Cargo.toml`

```toml
[dependencies]
mosaik = "0.2"
```

## Scenario Tests

Read through the scenario tests in the [`tests/`](tests/) directory for practical examples of mosaik capabilities.

```bash
# Run all integration tests
TEST_TRACE=on cargo test --test basic -- --test-threads=1

# Run some integration tests
TEST_TRACE=on cargo test --release --test basic collections::map -- --test-threads=1

# Verbose test output with tracing
TEST_TRACE=on cargo test --test basic groups::leader::is_elected
TEST_TRACE=trace cargo test --test basic groups::leader::is_elected
```

If tests are running on a slow network, the timeouts can be extended by setting the `TIME_FACTOR` env variable that will multiply all timeout durations by the given value, e.g:

```bash
TIME_FACTOR=3 TEST_TRACE=on cargo test --test basic groups::leader::is_elected
```

# Roadmap

## Stage 1: Primitives

Core primitives for building self-organized distributed systems in trusted, permissioned networks.

- [x] **Discovery** — gossip announcements, catalog sync, tags
- [x] **Streams** — producers, consumers, predicates, limits, online conditions, stats
- [x] **Groups** — membership, shared state, failover, load balancing
- [x] **Collections** — Replicated, eventually consistent data structures
- [x] **Tickets** — JWT and TDX-based peer authentication with expiration-aware disconnect
- [x] **TEE** — First-class Intel TDX support for hardware-attested identity and access control
- [ ] **Preferences** — ranking producers by latency, geo-proximity
- [ ] **Diagnostics** — network inspection, automatic metrics, developer debug tools

### Stage 2: Trust & Privacy

- [x] **TEE-based authorization** — gate access to streams, groups, and collections to hardware-attested peers using Intel TDX tickets and measurement validation
- [ ] **Privacy Corridors** — end-to-end data isolation across chains of nodes and services; data that enters a corridor cannot be read or exfiltrated outside of it
- [ ] **Trust zones** — network partitions that enforce integrity and privacy guarantees for all data and computation within them, backed by TEE hardware attestation

### Stage 3: Decentralization & Permissionlessness

Extending the system beyond trusted, single-operator environments.

## Contributing

- **Commits:** concise, imperative subjects referencing the component (e.g., *"Progress on pubsub semantics"*)
- **PRs:** include a summary, linked issues/RFCs, and a checklist confirming `cargo build`, `cargo test`, `cargo clippy`, and `cargo fmt` pass. Attach logs or screenshots for user-visible changes.
- **Tests:** add or extend integration coverage for behavioral changes. Note remaining gaps or follow-up work in the PR body.

## License

MIT — see [LICENSE](LICENSE) for details.
