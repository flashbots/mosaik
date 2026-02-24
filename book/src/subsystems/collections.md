# Collections

The **Collections** subsystem provides replicated, eventually consistent data
structures that feel like local Rust collections but are automatically
synchronized across all participating nodes using Raft consensus.

```text
┌──────────────────────────────────────────────────────┐
│                   Collections                        │
│                                                      │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌────────┐  │
│  │   Map    │ │   Vec    │ │   Set    │ │  DEPQ  │  │
│  │ <K, V>   │ │ <T>      │ │ <T>      │ │<P,K,V> │  │
│  └────┬─────┘ └────┬─────┘ └────┬─────┘ └───┬────┘  │
│       └─────────┬───┴───────┬────┘           │       │
│            ┌────▼───────────▼────────────────▼──┐    │
│            │    Groups (Raft Consensus)          │    │
│            │    One group per collection         │    │
│            └────────────────────────────────────┘    │
└──────────────────────────────────────────────────────┘
```

Each collection instance creates its own Raft consensus group. Different
collections (or the same type with different `StoreId`s) run as independent
groups and can span different subsets of the network.

## Quick start

```rust,ignore
use mosaik::collections::{Map, StoreId};

// On every node — using the same StoreId joins the same group
let prices = Map::<String, f64>::writer(&network, StoreId::from("prices"));

// Wait until the collection is online
prices.when().online().await;

// Write (only available on writers)
let version = prices.insert("ETH".into(), 3812.50).await?;

// Wait for the write to be committed
prices.when().reaches(version).await;

// Read (available on both writers and readers)
assert_eq!(prices.get(&"ETH".into()), Some(3812.50));
```

## Writer / Reader split

Every collection type offers two modes, distinguished at the **type level**
using a const-generic boolean:

| Mode       | Type alias                             | Can write? | Leadership priority |
| ---------- | -------------------------------------- | ---------- | ------------------- |
| **Writer** | `MapWriter<K,V>`, `VecWriter<T>`, etc. | Yes        | Normal              |
| **Reader** | `MapReader<K,V>`, `VecReader<T>`, etc. | No         | Deprioritized       |

Both modes provide identical read access. Readers automatically use
`deprioritize_leadership()` in their consensus configuration to reduce the
chance of being elected leader, since leaders handle write forwarding.

```rust,ignore
// Writer — can read AND write
let w = Map::<String, u64>::writer(&network, store_id);

// Reader — can only read, lower chance of becoming leader
let r = Map::<String, u64>::reader(&network, store_id);
```

## Available collections

| Collection                                                | Description                         | Backing structure             |
| --------------------------------------------------------- | ----------------------------------- | ----------------------------- |
| [`Map<K, V>`](collections/map.md)                         | Unordered key-value map             | `im::HashMap` (deterministic) |
| [`Vec<T>`](collections/vec.md)                            | Ordered, index-addressable sequence | `im::Vector`                  |
| [`Set<T>`](collections/set.md)                            | Unordered set of unique values      | `im::HashSet` (deterministic) |
| [`PriorityQueue<P, K, V>`](collections/priority-queue.md) | Double-ended priority queue         | `im::HashMap` + `im::OrdMap`  |

All collections use the [`im` crate](https://docs.rs/im) for their internal
state, which provides **O(1) structural sharing** — cloning a snapshot of the
entire collection is essentially free. This is critical for the snapshot-based
state sync mechanism.

## Trait requirements

Collections use blanket-implemented marker traits to constrain their type
parameters:

| Trait        | Required bounds                                                                                | Used by                         |
| ------------ | ---------------------------------------------------------------------------------------------- | ------------------------------- |
| `Value`      | `Clone + Debug + Serialize + DeserializeOwned + Hash + PartialEq + Eq + Send + Sync + 'static` | All element/value types         |
| `Key`        | `Clone + Serialize + DeserializeOwned + Hash + PartialEq + Eq + Send + Sync + 'static`         | Map keys, Set elements, PQ keys |
| `OrderedKey` | `Key + Ord`                                                                                    | PriorityQueue priorities        |

These traits are automatically implemented for any type that satisfies their
bounds — you never need to implement them manually.

## StoreId and group identity

Each collection derives its Raft group identity from:

1. **A fixed prefix** per collection type (e.g., `"mosaik_collections_map"`)
2. **The `StoreId`** — a `UniqueId` you provide at construction time
3. **The Rust type names** of the type parameters

This means `Map::<String, u64>::writer(&net, id)` and
`Map::<u32, u64>::writer(&net, id)` will join **different** groups even with
the same `StoreId`, because the key type differs.

## Version tracking

All mutating operations return a `Version`, which wraps the Raft log `Index`
at which the mutation will be committed:

```rust,ignore
let version: Version = map.insert("key".into(), 42).await?;

// Wait until the mutation is committed and visible
map.when().reaches(version).await;
```

## Error handling

All write operations return `Result<Version, Error<T>>`:

| Variant              | Meaning                                                                       |
| -------------------- | ----------------------------------------------------------------------------- |
| `Error::Offline(T)`  | The node is temporarily offline. The value that failed is returned for retry. |
| `Error::NetworkDown` | The network is permanently down. The collection is no longer usable.          |

## SyncConfig

Collections use a snapshot-based state sync mechanism to bring lagging
followers up to date. The `SyncConfig` controls this behavior:

| Parameter                  | Default | Description                                       |
| -------------------------- | ------- | ------------------------------------------------- |
| `fetch_batch_size`         | `2000`  | Items per batch during snapshot transfer          |
| `snapshot_ttl`             | `10s`   | How long a snapshot stays valid after last access |
| `snapshot_request_timeout` | `15s`   | Timeout waiting for a `SnapshotReady` response    |
| `fetch_timeout`            | `5s`    | Timeout per `FetchDataResponse`                   |

> **Important:** Different `SyncConfig` values produce different group
> signatures. Collections using different configs will **not** see each other.

```rust,ignore
use mosaik::collections::{Map, StoreId, SyncConfig};
use std::time::Duration;

let config = SyncConfig::default()
    .with_fetch_batch_size(5000)
    .with_snapshot_ttl(Duration::from_secs(30));

let map = Map::<String, u64>::writer_with_config(&network, store_id, config);
```

## How snapshot sync works

1. A lagging follower sends a `RequestSnapshot` to the leader.
2. The leader wraps the request as a special command and replicates it.
3. When committed, **all** peers create a snapshot at the same log position.
4. The follower fetches snapshot data in batches from available peers.
5. Once complete, the follower installs the snapshot and replays any buffered
   commands received during sync.

Because `im` data structures support O(1) cloning, creating a snapshot is
nearly instant regardless of collection size.

## Deterministic hashing

Map and Set use `BuildHasherDefault<DefaultHasher>` (SipHash-1-3 with a fixed
zero seed) for their internal `im::HashMap` / `im::HashSet`. This ensures
that **iteration order is identical across all nodes** for the same logical
state — a requirement for snapshot sync to produce consistent chunked
transfers.
