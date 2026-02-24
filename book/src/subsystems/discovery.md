# Discovery

The Discovery subsystem handles gossip-based peer discovery and catalog synchronization. It maintains a network-wide view of all peers, their capabilities, and their available streams and groups.

## Overview

Discovery uses two complementary protocols:

| Protocol     | ALPN                   | Purpose                                          |
| ------------ | ---------------------- | ------------------------------------------------ |
| Announce     | `/mosaik/announce`     | Real-time gossip broadcasts via iroh-gossip      |
| Catalog Sync | `/mosaik/catalog-sync` | Full bidirectional catalog exchange for catch-up |

Together, they ensure that every node eventually learns about every other node in the network.

## Configuration

Configure discovery through the `Network` builder:

```rust,ignore
use mosaik::{Network, discovery};
use std::time::Duration;

let network = Network::builder(network_id)
    .with_discovery(
        discovery::Config::builder()
            .with_bootstrap(bootstrap_addr)
            .with_tags("matcher")
            .with_tags(["validator", "signer"])  // additive
            .with_purge_after(Duration::from_secs(600))
            .with_announce_interval(Duration::from_secs(10))
            .with_announce_jitter(0.3)
            .with_max_time_drift(Duration::from_secs(5))
            .with_events_backlog(200)
    )
    .build()
    .await?;
```

### Configuration Options

| Field               | Default | Description                                     |
| ------------------- | ------- | ----------------------------------------------- |
| `bootstrap_peers`   | `[]`    | Initial peers to connect to on startup          |
| `tags`              | `[]`    | Tags to advertise about this node               |
| `announce_interval` | 15s     | How often to re-announce via gossip             |
| `announce_jitter`   | 0.5     | Max jitter factor (0.0–1.0) for announce timing |
| `purge_after`       | 300s    | Duration after which stale entries are purged   |
| `max_time_drift`    | 10s     | Maximum acceptable clock drift between peers    |
| `events_backlog`    | 100     | Past events retained in event broadcast channel |

Both `with_bootstrap()` and `with_tags()` are **additive** — calling them multiple times adds to the list.

## Accessing Discovery

```rust,ignore
let discovery = network.discovery();
```

The `Discovery` handle is cheap to clone.

## Core API

### Catalog Access

```rust,ignore
// Get current snapshot of all known peers
let catalog = discovery.catalog();
for (peer_id, entry) in catalog.iter() {
    println!("{}: tags={:?}", peer_id, entry.tags);
}

// Watch for catalog changes
let mut watch = discovery.catalog_watch();
loop {
    watch.changed().await?;
    let catalog = watch.borrow();
    println!("Catalog updated: {} peers", catalog.len());
}
```

### Dialing Peers

```rust,ignore
// Connect to bootstrap peers manually
discovery.dial(bootstrap_addr);

// Dial multiple peers
discovery.dial([addr1, addr2, addr3]);
```

### Manual Sync

```rust,ignore
// Trigger a full catalog sync with a specific peer
discovery.sync_with(peer_addr).await?;
```

### Managing Tags

```rust,ignore
// Add tags at runtime
discovery.add_tags("new-role");
discovery.add_tags(["role-a", "role-b"]);

// Remove tags
discovery.remove_tags("old-role");
```

Changing tags triggers an immediate re-announcement to the network.

### Local Entry

```rust,ignore
// Get this node's signed entry (as others see it)
let my_entry = discovery.me();
println!("I am: {:?}", my_entry);
```

### Unsigned Entries

For testing or manual feeds, you can insert entries that aren't cryptographically signed:

```rust,ignore
use mosaik::discovery::PeerEntry;

// Insert an unsigned entry (local-only, not gossiped)
discovery.insert(PeerEntry { /* ... */ });

// Remove a specific peer
discovery.remove(peer_id);

// Clear all unsigned entries
discovery.clear_unsigned();
```

### Injecting Signed Entries

```rust,ignore
// Feed a signed entry from an external source
let success = discovery.feed(signed_peer_entry);
```

## Events

Subscribe to discovery lifecycle events:

```rust,ignore
let mut events = discovery.events();
while let Ok(event) = events.recv().await {
    match event {
        Event::PeerDiscovered(entry) => {
            println!("New peer: {}", entry.peer_id());
        }
        Event::PeerUpdated(entry) => {
            println!("Peer updated: {}", entry.peer_id());
        }
        Event::PeerDeparted(peer_id) => {
            println!("Peer left: {}", peer_id);
        }
    }
}
```

See the [Events](./discovery/events.md) sub-chapter for details.

## How It Works

### Announce Protocol

Every node periodically broadcasts its `SignedPeerEntry` via iroh-gossip. The announce includes:
- Node identity (PeerId)
- Network ID
- Tags
- Available streams and groups
- Version (start + update timestamps)

The jitter on the announce interval prevents all nodes from announcing simultaneously. When a node changes its metadata (adds a tag, creates a stream, joins a group), it re-announces immediately.

### Catalog Sync Protocol

When a new node connects to a peer, they exchange their full catalogs bidirectionally. This ensures a new node quickly learns about all existing peers without waiting for gossip cycles.

### Signed vs Unsigned Entries

| Property     | Signed                        | Unsigned              |
| ------------ | ----------------------------- | --------------------- |
| Source       | Created by the peer itself    | Injected locally      |
| Verification | Cryptographic signature       | None                  |
| Gossip       | Yes — propagated network-wide | No — local only       |
| Use case     | Normal operation              | Testing, manual feeds |

### Staleness Detection

Each entry has a two-part version: `(start_timestamp, update_timestamp)`. If `update_timestamp` falls behind the current time by more than `purge_after`, the entry is hidden from the public catalog API and eventually removed.

See the [Catalog](./discovery/catalog.md) sub-chapter for the full catalog API.
