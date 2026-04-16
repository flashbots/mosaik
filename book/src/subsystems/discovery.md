# Discovery

The Discovery subsystem handles gossip-based peer discovery and catalog synchronization. It maintains a network-wide view of all peers, their capabilities, and their available streams and groups.

## Overview

Discovery uses three complementary mechanisms:

| Mechanism     | Transport                          | Purpose                                          |
| ------------- | ---------------------------------- | ------------------------------------------------ |
| DHT Bootstrap | Mainline DHT (pkarr)               | Automatic peer discovery via shared `NetworkId`  |
| Announce      | `/mosaik/discovery/announce/1.0`   | Real-time gossip broadcasts via iroh-gossip      |
| Catalog Sync  | `/mosaik/discovery/sync/1.0`       | Full bidirectional catalog exchange for catch-up |

Nodes sharing the same `NetworkId` automatically discover each other through the DHT — no hardcoded bootstrap peers are required. Once an initial connection is established, the Announce and Catalog Sync protocols take over for real-time updates.

See the [DHT Bootstrap](./discovery/dht-bootstrap.md) sub-chapter for details on the automatic discovery mechanism.

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
            .with_max_rtt(Duration::from_millis(500))
            .with_rtt_probe_interval(Duration::from_secs(15))
    )
    .build()
    .await?;
```

### Configuration Options

| Field                | Default | Description                                     |
| -------------------- | ------- | ----------------------------------------------- |
| `bootstrap_peers`    | `[]`    | Initial peers to connect to on startup          |
| `tags`               | `[]`    | Tags to advertise about this node               |
| `announce_interval`  | 45s     | How often to re-announce via gossip             |
| `announce_jitter`    | 0.5     | Max jitter factor (0.0–1.0) for announce timing |
| `purge_after`        | 300s    | Duration after which stale entries are purged   |
| `max_time_drift`     | 50s     | Maximum acceptable clock drift between peers    |
| `events_backlog`     | 100     | Past events retained in event broadcast channel |
| `rtt_probe_interval` | 30s     | Interval for RTT ping probes to cataloged peers |
| `max_rtt`            | `None`  | Maximum acceptable RTT for peers (see below)    |
| `no_dht_bootstrap`   | `false` | Disables the DHT bootstrap mechanism entirely   |

Both `with_bootstrap()` and `with_tags()` are **additive** — calling them multiple times adds to the list.

### RTT tracking

The discovery system continuously measures round-trip times to cataloged
peers using periodic ping probes (controlled by `rtt_probe_interval`).
RTT samples are also collected passively from active connections (group
bonds, stream producers and consumers).

Smoothing uses EWMA following RFC 6298 (α = 1/8, β = 1/4) so that
transient spikes are absorbed while sustained latency changes are
reflected.

#### Network-wide RTT filtering with `max_rtt`

When `max_rtt` is set in the discovery config, the catalog filters out
peers whose smoothed RTT exceeds the threshold. **High-latency peers
become invisible to the local node** — they will not appear in
`catalog.peers()`, `catalog.get()`, or `catalog.peers_count()`, and
consequently will not be discovered by stream consumers, connected to
by stream producers, or considered for group bond formation. This is a
hard cut-off at the discovery layer that affects all subsystems:

```rust,ignore
let network = Network::builder(network_id)
    .with_discovery(
        discovery::Config::builder()
            .with_max_rtt(Duration::from_millis(300))
    )
    .build()
    .await?;

// catalog.peers() only returns peers with RTT ≤ 300ms
// (peers with no RTT data are included optimistically)
```

Peers with no RTT measurements are still included (optimistic
admission) — they are not excluded until the discovery system has
measured their latency. This avoids rejecting newly discovered peers
before they have had a chance to establish direct connections.

> **Note:** Use `max_rtt` when you want a blanket latency requirement
> for the entire node. For per-stream thresholds (e.g. a fast stream
> with strict latency alongside a best-effort stream), use
> `require(|peer| peer.rtt_below(...))` on individual producers or
> consumers instead.

#### Per-stream RTT filtering with `require()`

For finer-grained control, RTT data is also available to `require()`
predicates on individual producers and consumers via the `PeerInfo`
type — see
[Producers > RTT-based filtering](./streams/producers.md#rtt-based-filtering)
and
[Consumers > RTT-based filtering](./streams/consumers.md#rtt-based-filtering).

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
    println!("Catalog updated: {} peers", catalog.peers_count());
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

### Managing Tickets

Tickets are opaque, typed credentials attached to a peer's discovery entry. They are commonly used to authenticate consumers in stream subscriptions.

```rust,ignore
use mosaik::{Ticket, UniqueId, unique_id, Bytes};

const MY_AUTH: UniqueId = unique_id!("my-app.auth");

// Add a ticket
let ticket = Ticket::new(MY_AUTH, Bytes::from("credential-data"));
discovery.add_ticket(ticket.clone());

// Remove a specific ticket
discovery.remove_ticket(ticket.id());

// Remove all tickets of a given class
discovery.remove_tickets_of(MY_AUTH);
```

Tickets propagate through gossip and catalog sync like tags and streams. See the [Auth Tickets](./discovery/tickets.md) sub-chapter for a full walkthrough including JWT-based authentication.

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
- Auth tickets
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
