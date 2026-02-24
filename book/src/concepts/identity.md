# Identity & Networking

Mosaik's identity system is built on cryptographic keys and content-addressed hashing. Every identifier in the system — networks, peers, streams, groups, collections — is a 32-byte `Digest` (blake3 hash).

## UniqueId: The Universal Identifier

At the core of mosaik's identity system is `Digest` — a 32-byte blake3 hash that serves as the universal identifier type:

```rust,ignore
use mosaik::Digest;

// From a string (hashes the string if not valid hex)
let id: UniqueId = "my-network".into();

// From raw bytes
let id = UniqueId::from_bytes([0u8; 32]);

// Random
let id = UniqueId::random();

// Compile-time constant from hex
use mosaik::unique_id;
let id = unique_id!("a1b2c3d4e5f6...");  // 64 hex chars

// Deterministic derivation
let derived = id.derive("sub-identifier");
```

All the following types are aliases for `Digest`:

| Type        | Alias For  | Identifies                                    |
| ----------- | ---------- | --------------------------------------------- |
| `UniqueId`  | `Digest`   | General-purpose unique identifier             |
| `NetworkId` | `UniqueId` | A mosaik network (derived from name)          |
| `Tag`       | `UniqueId` | A capability or role label                    |
| `StreamId`  | `UniqueId` | A data stream (derived from type name)        |
| `GroupId`   | `UniqueId` | A consensus group (derived from key + config) |
| `StoreId`   | `UniqueId` | A replicated collection instance              |

## PeerId: Node Identity

A `PeerId` is the node's public key, derived from its secret key. It's globally unique across all mosaik networks.

```rust,ignore
use mosaik::{Network, PeerId};
use iroh::SecretKey;

// Random identity (default when using Network::new)
let network = Network::new(network_id).await?;
let my_id: &PeerId = &network.local().id();

// Stable identity via explicit secret key
let secret = SecretKey::generate(&mut rand::rng());
let network = Network::builder(network_id)
    .with_secret_key(secret)
    .build()
    .await?;
```

For bootstrap nodes and other long-lived infrastructure, you should use a fixed secret key so the node's `PeerId` (and therefore its address) remains stable across restarts.

## NetworkId: Network Isolation

A `NetworkId` is a `Digest` derived from a name string. Nodes can only connect to peers sharing the same `NetworkId`:

```rust,ignore
use mosaik::NetworkId;

// These produce the same NetworkId
let id1: NetworkId = "my-app".into();
let id2: NetworkId = "my-app".into();
assert_eq!(id1, id2);

// Different name → different network → can't communicate
let other: NetworkId = "other-app".into();
assert_ne!(id1, other);
```

The `NetworkId` also drives **automatic peer discovery**: nodes sharing the same `NetworkId` find each other through the [Mainline DHT](../subsystems/discovery/dht-bootstrap.md) without requiring any hardcoded bootstrap peers. Simply using the same network name is enough for nodes to connect.

## Tags: Capability Labels

Tags are `Digest` values used to describe a node's role or capabilities:

```rust,ignore
use mosaik::Tag;

let tag: Tag = "matcher".into();
let another: Tag = "validator".into();
```

Tags are advertised through the discovery system and can be used to filter which peers a producer accepts or which producers a consumer subscribes to:

```rust,ignore
// Only accept consumers that have the "authorized" tag
let producer = network.streams().producer::<Order>()
    .accept_if(|peer| peer.tags.contains(&"authorized".into()))
    .build()?;
```

## StreamId: Stream Identity

By default, a `StreamId` is derived from the Rust type name:

```rust,ignore
use mosaik::StreamId;

// Automatically derived from the type name
let producer = network.streams().produce::<SensorReading>();

// Or set explicitly
let producer = network.streams().producer::<SensorReading>()
    .with_stream_id("custom-stream-name")
    .build()?;
```

## GroupId: Group Identity

A `GroupId` is deterministically derived from multiple inputs:

```text
GroupId = hash(
    GroupKey,
    ConsensusConfig,
    StateMachine::signature(),
    StateSync::signature()
)
```

This ensures that nodes with different configurations, different state machines, or different group secrets **cannot** accidentally join the same group.

## Endpoint Addresses

Nodes are addressed using `EndpointAddr` from iroh, which encodes the public key and optional relay URL. This is what you pass to bootstrap peers:

```rust,ignore
let addr = network.local().addr();
// addr contains: PeerId + relay URL + direct addresses
```
