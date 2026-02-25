# Summary

[Introduction](./introduction.md)

---

# Getting Started

- [Overview](./getting-started/overview.md)
- [Installation](./getting-started/installation.md)
- [Quick Start](./getting-started/quickstart.md)

# Core Concepts

- [Architecture](./concepts/architecture.md)
- [Identity & Networking](./concepts/identity.md)
- [Self-Organization](./concepts/self-organization.md)

# Tutorials

- [Building a Bootstrap Node](./tutorials/bootstrap.md)
- [Building a Distributed Orderbook](./tutorials/orderbook.md)

# Subsystems

- [Network](./subsystems/network.md)
- [Discovery](./subsystems/discovery.md)
  - [DHT Bootstrap](./subsystems/discovery/dht-bootstrap.md)
  - [Catalog](./subsystems/discovery/catalog.md)
  - [Events](./subsystems/discovery/events.md)
- [Streams](./subsystems/streams.md)
  - [Producers](./subsystems/streams/producers.md)
  - [Consumers](./subsystems/streams/consumers.md)
  - [Status & Conditions](./subsystems/streams/status.md)
- [Groups](./subsystems/groups.md)
  - [Creating & Joining](./subsystems/groups/joining.md)
  - [State Machines](./subsystems/groups/state-machines.md)
  - [Commands & Queries](./subsystems/groups/commands.md)
  - [Status & Observation](./subsystems/groups/status.md)
- [Collections](./subsystems/collections.md)
  - [Map](./subsystems/collections/map.md)
  - [Vec](./subsystems/collections/vec.md)
  - [Set](./subsystems/collections/set.md)
  - [Register](./subsystems/collections/register.md)
  - [PriorityQueue](./subsystems/collections/priority-queue.md)
  - [Writer/Reader Pattern](./subsystems/collections/writer-reader.md)

# Architecture Deep Dives

- [Raft Consensus](./internals/raft.md)
- [Bonds & Peer Connections](./internals/bonds.md)
- [State Sync & Replay](./internals/state-sync.md)
- [Protocol & Framing](./internals/protocol.md)
- [Determinism & Hashing](./internals/determinism.md)

# Reference

- [Configuration](./reference/configuration.md)
- [Error Handling](./reference/errors.md)
- [Primitives & Types](./reference/primitives.md)
- [Testing Patterns](./reference/testing.md)
