# Catalog

The `Catalog` is an immutable snapshot of all discovered peers in the network. It's thread-safe, cheap to clone, and updated via `watch` channels.

## Getting a Catalog

```rust,ignore
let catalog = network.discovery().catalog();
```

Each call returns a snapshot — it won't change after you've obtained it. To observe changes, use the watch channel.

## Catalog API

### Iterating Peers

```rust,ignore
let catalog = discovery.catalog();

// Iterate all peers
for (peer_id, entry) in catalog.iter() {
    println!("Peer {}: {:?}", peer_id, entry.tags);
}

// Count peers
println!("Known peers: {}", catalog.len());
```

### Watching for Changes

```rust,ignore
let mut watch = discovery.catalog_watch();

loop {
    // Wait for catalog to change
    watch.changed().await?;

    // Borrow the latest snapshot
    let catalog = watch.borrow();
    println!("Catalog updated, {} peers", catalog.len());
}
```

The watch receiver always has the latest value. Multiple calls to `changed()` may skip intermediate updates if the catalog changes faster than you observe it.

## PeerEntry

Each peer in the catalog is represented by a `PeerEntry`:

```rust,ignore
pub struct PeerEntry {
    pub network_id: NetworkId,
    pub peer_id: PeerId,
    pub addr: EndpointAddr,
    pub tags: BTreeSet<Tag>,
    pub streams: BTreeSet<StreamId>,
    pub groups: BTreeSet<GroupId>,
    pub version: PeerEntryVersion,
}
```

### Fields

| Field        | Type                 | Description                                            |
| ------------ | -------------------- | ------------------------------------------------------ |
| `network_id` | `NetworkId`          | Which network this peer belongs to                     |
| `peer_id`    | `PeerId`             | The peer's public key                                  |
| `addr`       | `EndpointAddr`       | Connection address (public key + relay + direct addrs) |
| `tags`       | `BTreeSet<Tag>`      | Capability labels (e.g., `"matcher"`, `"validator"`)   |
| `streams`    | `BTreeSet<StreamId>` | Streams this peer produces                             |
| `groups`     | `BTreeSet<GroupId>`  | Groups this peer belongs to                            |
| `version`    | `PeerEntryVersion`   | Two-part version for staleness detection               |

### Signed Entries

In practice, most catalog entries are `SignedPeerEntry` — a `PeerEntry` with a cryptographic signature proving the owner created it:

```rust,ignore
let my_entry: SignedPeerEntry = discovery.me();
let peer_id = my_entry.peer_id();
```

## Version & Staleness

Each entry carries a two-part version:

```rust,ignore
pub struct PeerEntryVersion {
    pub start: Timestamp,   // When the peer first came online
    pub update: Timestamp,  // When the entry was last updated
}
```

Entries where `update` is older than `purge_after` (default: 300s) are considered stale. Stale entries are:
1. **Hidden** from the public catalog API
2. **Eventually removed** if not refreshed

The periodic gossip re-announce (every ~15s by default) keeps entries fresh. When a node departs gracefully, it broadcasts a departure message; ungraceful departures are detected by staleness.

## Using Catalog for Filtering

The catalog is commonly used to filter peers in Streams:

```rust,ignore
// Producer: only accept consumers with specific tags
let producer = network.streams().producer::<Order>()
    .accept_if(|peer| peer.tags.contains(&"authorized".into()))
    .build()?;

// Consumer: only subscribe to specific producers
let consumer = network.streams().consumer::<Order>()
    .subscribe_if(|peer| peer.tags.contains(&"primary".into()))
    .build();
```

## Internal Structure

The catalog uses `im::OrdMap` — an immutable, persistent ordered map — for its internal storage. This provides:

- **O(1) cloning** — snapshots are effectively free
- **Consistent iteration order** — deterministic across nodes
- **Thread safety** — immutable snapshots can be shared freely

Updates create new snapshots atomically via `tokio::sync::watch::Sender<Catalog>`.
