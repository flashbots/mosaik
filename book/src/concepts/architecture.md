# Architecture

This chapter describes how mosaik's subsystems fit together, how protocols are multiplexed, and how the lifecycle of a node is managed.

## Subsystem Composition

A `Network` is the top-level entry point. When built, it creates and composes four subsystems:

```text
Network::builder(network_id)
    │
    ├── LocalNode          (QUIC endpoint, identity, lifecycle)
    ├── Discovery          (gossip announcements, catalog sync)
    ├── Streams            (typed pub/sub channels)
    └── Groups             (Raft consensus groups)
            └── Collections  (replicated Map, Vec, Set, PriorityQueue)
```

Each subsystem is created during `Network::builder().build()` and installed as a protocol handler on the iroh `Router`. The subsystems are then accessible via accessor methods:

```rust,ignore
let network = Network::builder(network_id).build().await?;

let local     = network.local();       // LocalNode
let discovery = network.discovery();   // Discovery
let streams   = network.streams();     // Streams
let groups    = network.groups();      // Groups
```

All handles are cheap to clone (they wrap `Arc` internally).

## ALPN-Based Protocol Multiplexing

Mosaik multiplexes multiple protocols over a single QUIC endpoint using **ALPN** (Application-Layer Protocol Negotiation). Each subsystem registers its own ALPN identifier:

| Subsystem            | ALPN                   | Purpose                           |
| -------------------- | ---------------------- | --------------------------------- |
| Discovery (announce) | `/mosaik/announce`     | Real-time gossip broadcasts       |
| Discovery (sync)     | `/mosaik/catalog-sync` | Full catalog exchange             |
| Streams              | `/mosaik/streams/1.0`  | Pub/sub data channels             |
| Groups               | `/mosaik/groups/1`     | Raft consensus, bonds, state sync |

When a connection arrives, the iroh router inspects the ALPN and dispatches to the correct subsystem handler. This means all subsystems share the same QUIC endpoint and port.

Subsystems implement the `ProtocolProvider` trait to install their handlers:

```rust,ignore
// Internal trait — subsystems implement this
trait ProtocolProvider {
    fn install(self, router: &mut Router);
}
```

The `Protocol` trait defines the ALPN for typed links:

```rust,ignore
pub trait Protocol {
    const ALPN: &'static [u8];
}
```

## Builder Pattern

`Network` uses a builder pattern for configuration:

```rust,ignore
let network = Network::builder(network_id)
    .with_secret_key(secret_key)           // Stable identity
    .with_relay_mode(RelayMode::Disabled)  // No relay servers
    .with_mdns_discovery(true)             // Local network discovery
    .with_discovery(
        discovery::Config::builder()
            .with_bootstrap(bootstrap_addr)
            .with_tags("my-role")
    )
    .with_streams(
        streams::Config::builder()
            .with_backoff(ExponentialBackoff::default())
    )
    .with_groups(
        groups::Config::builder()
    )
    .build()
    .await?;
```

For quick prototyping, `Network::new(network_id)` uses all defaults.

## Lifecycle & Shutdown

Mosaik uses `tokio_util::sync::CancellationToken` for structured lifecycle management. The token lives in `LocalNode` and propagates shutdown to all subsystems:

```text
Network::drop()
    │
    ├── cancels LocalNode's CancellationToken
    │       │
    │       ├── Discovery workers shut down
    │       ├── Stream producer/consumer workers shut down
    │       ├── Group bond workers + Raft shut down
    │       └── iroh Router shuts down
    │
    └── all resources released
```

When a `Network` is dropped, the cancellation token is triggered, and all background tasks gracefully terminate. Each subsystem's internal tasks are select-looped against the cancellation token, ensuring no orphaned tasks.

## Internal Communication Patterns

Mosaik uses several recurring patterns internally:

### Watch Channels

Status changes are broadcast via `tokio::sync::watch` channels. This enables the `when()` API:

```rust,ignore
// Wait for a stream producer to come online
producer.when().online().await;

// Wait for a Raft group leader to be elected
group.when().leader_elected().await;

// Wait for a collection to reach a specific version
collection.when().reaches(version).await;
```

### Arc\<Inner\> Pattern

All public handles (`Network`, `Discovery`, `Streams`, `Groups`, `Group`, `Producer`, `Consumer`, `Map`, `Vec`, etc.) are cheap to clone. They wrap an `Arc<Inner>` containing the actual state, making them safe to share across tasks.

### Task-Per-Connection

Each consumer subscription and each producer-subscriber pair runs in its own tokio task. This avoids head-of-line blocking — a slow consumer doesn't affect other consumers, and a slow subscriber doesn't block the producer.
