# Configuration Reference

This chapter lists every configuration struct in mosaik, its fields, default
values, and how the pieces fit together.

## `NetworkBuilder` (top-level)

The entry point for creating a mosaik network. All nested configs are
accessible through fluent builder methods.

```rust,ignore
use mosaik::Network;

let network = Network::builder()
    .network_id("my-network")
    .mdns_discovery(true)
    .discovery(|d| d.events_backlog(200))
    .streams(|s| s.with_backoff(my_backoff))
    .groups(|g| g.handshake_timeout(Duration::from_secs(5)))
    .build()
    .await?;
```

| Field            | Type                     | Default                | Description                                  |
| ---------------- | ------------------------ | ---------------------- | -------------------------------------------- |
| `network_id`     | `NetworkId`              | **required**           | Unique identifier for this network           |
| `relay_mode`     | `iroh::RelayMode`        | `RelayMode::Default`   | Relay server mode for NAT/firewall traversal |
| `mdns_discovery` | `bool`                   | `false`                | Enable mDNS local network peer discovery     |
| `addresses`      | `BTreeSet<SocketAddr>`   | empty (all interfaces) | Local bind addresses                         |
| `secret_key`     | `SecretKey`              | random                 | Ed25519 key for peer identity                |
| `discovery`      | `DiscoveryConfigBuilder` | see below              | Nested discovery config                      |
| `streams`        | `StreamsConfigBuilder`   | see below              | Nested streams config                        |
| `groups`         | `GroupsConfigBuilder`    | see below              | Nested groups config                         |

> **Note**: `secret_key` determines the `PeerId`. If omitted, a random key
> is generated on each run, giving the node a new identity every time.
> Specifying a fixed key is only recommended for **bootstrap nodes** that need
> a stable, well-known peer ID across restarts.

---

## `discovery::Config`

Controls peer discovery, gossip, and catalog maintenance.

| Field                       | Type                | Default        | Description                               |
| --------------------------- | ------------------- | -------------- | ----------------------------------------- |
| `events_backlog`            | `usize`             | `100`          | Past events retained in event watchers    |
| `bootstrap_peers`           | `Vec<EndpointAddr>` | empty          | Initial peers to connect to on startup    |
| `tags`                      | `Vec<Tag>`          | empty          | Tags advertised in local `PeerEntry`      |
| `purge_after`               | `Duration`          | `300s` (5 min) | Time before stale peer entries are purged |
| `max_time_drift`            | `Duration`          | `10s`          | Maximum acceptable timestamp drift        |
| `announce_interval`         | `Duration`          | `15s`          | Interval between presence announcements   |
| `announce_jitter`           | `f32`               | `0.5`          | Max jitter factor on announce interval    |
| `graceful_departure_window` | `Duration`          | `500ms`        | Wait for departure gossip to propagate    |

Builder methods `with_bootstrap(peers)` and `with_tags(tags)` accept either a
single item or an iterator (via `IntoIterOrSingle`):

```rust,ignore
Network::builder()
    .discovery(|d| d
        .with_bootstrap(peer_addr)          // single peer
        .with_tags(["validator", "relay"])   // multiple tags
        .purge_after(Duration::from_secs(600))
    )
```

---

## `streams::Config`

Controls stream consumer reconnection behavior.

| Field     | Type             | Default                | Description                                  |
| --------- | ---------------- | ---------------------- | -------------------------------------------- |
| `backoff` | `BackoffFactory` | Exponential, 5 min max | Retry policy for consumer stream connections |

Custom backoff example:

```rust,ignore
use mosaik::streams::backoff::ExponentialBackoff;

Network::builder()
    .streams(|s| s.with_backoff(ExponentialBackoff {
        max_elapsed_time: Some(Duration::from_secs(120)),
        ..Default::default()
    }))
```

---

## `groups::Config`

Controls bond establishment.

| Field               | Type       | Default | Description                                  |
| ------------------- | ---------- | ------- | -------------------------------------------- |
| `handshake_timeout` | `Duration` | `2s`    | Timeout for bond handshake with remote peers |

---

## `ConsensusConfig`

Consensus parameters that **must be identical** across all members of a group.
These values are hashed into the `GroupId`, so any mismatch creates a different
group.

| Field                     | Type       | Default | Description                                            |
| ------------------------- | ---------- | ------- | ------------------------------------------------------ |
| `heartbeat_interval`      | `Duration` | `500ms` | Bond heartbeat interval                                |
| `heartbeat_jitter`        | `Duration` | `150ms` | Max heartbeat jitter                                   |
| `max_missed_heartbeats`   | `u32`      | `10`    | Missed heartbeats before bond is considered dead       |
| `election_timeout`        | `Duration` | `2s`    | Raft election timeout (must exceed heartbeat interval) |
| `election_timeout_jitter` | `Duration` | `500ms` | Election timeout randomization                         |
| `bootstrap_delay`         | `Duration` | `3s`    | Extra delay for the first election (term 0)            |
| `forward_timeout`         | `Duration` | `2s`    | Timeout for forwarding commands to leader              |
| `query_timeout`           | `Duration` | `2s`    | Timeout for leader to respond to queries               |

```rust,ignore
use mosaik::groups::ConsensusConfig;

let config = ConsensusConfig::builder()
    .heartbeat_interval(Duration::from_millis(250))
    .election_timeout(Duration::from_secs(1))
    .build()
    .unwrap();
```

### Leadership deprioritization

```rust,ignore
let config = ConsensusConfig::default().deprioritize_leadership();
```

This multiplies the election timeout by a factor, making this node less likely
to become leader. Used by collection readers.

---

## `SyncConfig` (collections)

Controls snapshot-based state synchronization for collections.

| Field                      | Type       | Default | Description                           |
| -------------------------- | ---------- | ------- | ------------------------------------- |
| `fetch_batch_size`         | `usize`    | `2000`  | Max items per batch request           |
| `snapshot_ttl`             | `Duration` | `10s`   | How long a snapshot remains available |
| `snapshot_request_timeout` | `Duration` | `15s`   | Timeout for requesting a snapshot     |
| `fetch_timeout`            | `Duration` | `5s`    | Timeout for each batch fetch          |

---

## Configuration hierarchy

```text
NetworkBuilder
├── network_id          (identity)
├── secret_key          (identity)
├── relay_mode          (transport)
├── mdns_discovery      (transport)
├── addresses           (transport)
│
├── discovery::Config
│   ├── bootstrap_peers
│   ├── tags
│   ├── events_backlog
│   ├── purge_after
│   ├── max_time_drift
│   ├── announce_interval
│   └── announce_jitter
│
├── streams::Config
│   └── backoff
│
└── groups::Config
    └── handshake_timeout

Per-group:
ConsensusConfig         (consensus timing, hashed into GroupId)

Per-collection:
SyncConfig              (state sync tuning)
```

## Environment influence

Configuration structs are pure Rust — there is no automatic environment
variable parsing. However, test utilities honor:

| Variable            | Used by            | Effect                                           |
| ------------------- | ------------------ | ------------------------------------------------ |
| `TEST_TRACE`        | Test tracing setup | Controls log level (`debug`/`trace`/`info`/etc.) |
| `TEST_TRACE_UNMUTE` | Test tracing setup | Set to `1` to show all log output                |
| `TIME_FACTOR`       | Test time helpers  | Float multiplier for all test durations          |
