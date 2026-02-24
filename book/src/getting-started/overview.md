# Overview

Mosaik is a Rust runtime for building self-organizing, leaderless distributed systems. It targets **trusted, permissioned networks** — environments where all participating nodes are controlled by the same organization and assumed to be honest.

## Key Features

| Feature                    | Description                                                                               |
| -------------------------- | ----------------------------------------------------------------------------------------- |
| **Self-organizing**        | Nodes discover each other via gossip and form the correct topology automatically          |
| **Typed pub/sub**          | Stream any serializable Rust type between nodes with backpressure and filtering           |
| **Raft consensus**         | Form availability groups with leader election and replicated state machines               |
| **Replicated collections** | Distributed `Map`, `Vec`, `Set`, and `PriorityQueue` with strong consistency              |
| **QUIC transport**         | Built on [iroh](https://github.com/n0-computer/iroh) for modern, encrypted P2P networking |
| **Relay support**          | Nodes behind NAT can communicate via relay servers with automatic hole-punching           |

## Module Map

Mosaik is organized into five composable subsystems:

```text
┌──────────────────────────────────────────────────────┐
│                     Network                          │
│  Entry point. QUIC endpoint, identity, routing.      │
├─────────────┬─────────────┬─────────────┬────────────┤
│  Discovery  │   Streams   │   Groups    │Collections │
│  Gossip &   │  Typed      │  Raft       │ Replicated │
│  peer       │  pub/sub    │  consensus  │ Map, Vec,  │
│  catalog    │  channels   │  groups     │ Set, DEPQ  │
└─────────────┴─────────────┴─────────────┴────────────┘
```

### Network

The entry point to the SDK. Creates the QUIC endpoint, manages node identity (derived from a secret key), and composes all subsystems via ALPN-based protocol multiplexing.

### Discovery

Gossip-based peer discovery. Maintains a **catalog** — a synchronized view of all known peers, their capabilities (tags), and their available streams and groups. Uses two complementary protocols: real-time gossip announcements and full catalog synchronization for catch-up.

### Streams

Typed, async pub/sub data channels. Any Rust type implementing `Serialize + DeserializeOwned + Send + 'static` can be streamed. Producers publish data; consumers subscribe. Discovery automatically connects matching producers and consumers across the network.

### Groups

Availability groups coordinated by a modified Raft consensus protocol. Nodes sharing a group key form a cluster, elect a leader, and replicate commands through a shared log. Custom state machines define application logic that runs deterministically on all group members.

### Collections

Higher-level replicated data structures built on Groups. Each collection (`Map`, `Vec`, `Set`, `PriorityQueue`) creates its own Raft group with a specialized state machine.

## Design Decisions

### Trusted Network Assumption

Mosaik is **not** Byzantine fault tolerant. All nodes are assumed to be honest and correctly implementing the protocol. This assumption enables:

- Simpler consensus (no need for 2/3 supermajority)
- Higher throughput (fewer message rounds)
- Simplified state sync (no fraud proofs needed)

This makes mosaik ideal for infrastructure controlled by a single organization, such as L2 chains, internal microservices, or distributed compute clusters.

### Built on iroh

Mosaik uses [iroh](https://github.com/n0-computer/iroh) for its networking layer, which provides:

- **QUIC transport** — multiplexed, encrypted connections
- **Relay servers** — NAT traversal for nodes behind firewalls
- **mDNS** — optional local network discovery
- **Endpoint identity** — public keys as node identifiers
