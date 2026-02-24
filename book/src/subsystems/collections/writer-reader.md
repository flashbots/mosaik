# Writer/Reader Pattern

All mosaik collections share a common access-control pattern: every collection
type is parameterized by a const-generic boolean (`IS_WRITER`) that determines
whether the instance has write access.

```text
                 ┌──────────────────┐
                 │  Collection<T>   │
                 │  const IS_WRITER │
                 └────────┬─────────┘
                          │
              ┌───────────┴───────────┐
              │                       │
       IS_WRITER = true        IS_WRITER = false
       ┌──────────────┐       ┌──────────────┐
       │    Writer     │       │    Reader     │
       │ read + write  │       │  read-only   │
       │ normal leader │       │ deprioritized │
       │   priority    │       │   leader      │
       └──────────────┘       └──────────────┘
```

## Type aliases

Each collection provides convenient type aliases:

| Collection               | Writer type                    | Reader type                    |
| ------------------------ | ------------------------------ | ------------------------------ |
| `Map<K, V>`              | `MapWriter<K, V>`              | `MapReader<K, V>`              |
| `Vec<T>`                 | `VecWriter<T>`                 | `VecReader<T>`                 |
| `Set<T>`                 | `SetWriter<T>`                 | `SetReader<T>`                 |
| `PriorityQueue<P, K, V>` | `PriorityQueueWriter<P, K, V>` | `PriorityQueueReader<P, K, V>` |

## Construction

Every collection offers the same set of constructors:

```rust,ignore
// Writer (default)
Collection::writer(&network, store_id)
Collection::writer_with_config(&network, store_id, sync_config)

// Convenience aliases for writer
Collection::new(&network, store_id)
Collection::new_with_config(&network, store_id, sync_config)

// Reader
Collection::reader(&network, store_id)
Collection::reader_with_config(&network, store_id, sync_config)
```

The `StoreId` and `SyncConfig` must match between writers and readers for them
to join the same consensus group.

## How it works

Internally, the const-generic boolean controls two things:

1. **Method availability** — Write methods (`insert`, `push_back`, `remove`,
   etc.) are only implemented for `IS_WRITER = true`. This is enforced at
   compile time.

2. **Leadership priority** — Readers return a `ConsensusConfig` with
   `deprioritize_leadership()`, which increases their election timeout. This
   makes it less likely for a reader to become the Raft leader, keeping
   leadership on writer nodes where write operations are handled directly
   rather than being forwarded.

```rust,ignore
// Inside every collection's StateMachine impl:
fn consensus_config(&self) -> Option<ConsensusConfig> {
    (!self.is_writer)
        .then(|| ConsensusConfig::default().deprioritize_leadership())
}
```

## Shared read API

Both writers and readers have identical read access. The read methods are
implemented on `Collection<T, IS_WRITER>` without constraining `IS_WRITER`:

```rust,ignore
// Works on both MapWriter and MapReader
impl<K: Key, V: Value, const IS_WRITER: bool> Map<K, V, IS_WRITER> {
    pub fn len(&self) -> usize { ... }
    pub fn get(&self, key: &K) -> Option<V> { ... }
    pub fn iter(&self) -> impl Iterator<Item = (K, V)> { ... }
    pub fn when(&self) -> &When { ... }
    pub fn version(&self) -> Version { ... }
}
```

## Trait requirements

The type parameters for collection elements must satisfy blanket-implemented
trait bounds:

### Value

Required for all element and value types:

```rust,ignore
pub trait Value:
    Clone + Debug + Serialize + DeserializeOwned
    + Hash + PartialEq + Eq + Send + Sync + 'static
{}

// Blanket impl — any conforming type is a Value
impl<T> Value for T where T: Clone + Debug + Serialize + ... {}
```

### Key

Required for map keys, set elements, and priority queue keys:

```rust,ignore
pub trait Key:
    Clone + Serialize + DeserializeOwned
    + Hash + PartialEq + Eq + Send + Sync + 'static
{}
```

Note that `Key` does not require `Debug` (unlike `Value`).

### OrderedKey

Required for priority queue priorities:

```rust,ignore
pub trait OrderedKey: Key + Ord {}
impl<T: Key + Ord> OrderedKey for T {}
```

## Version

All write operations return `Version`, which wraps the Raft log `Index` where
the mutation will be committed. Use it with the `When` API to synchronize:

```rust,ignore
let version = map.insert("key".into(), value).await?;
map.when().reaches(version).await;
// Now the insert is guaranteed to be committed
```

`Version` implements `Deref<Target = Index>`, `PartialOrd`, `Ord`, `Display`,
and `Copy`.

## When (collections)

The collections `When` is a thin wrapper around the groups `When` that exposes
collection-relevant observers:

| Method             | Description                                                       |
| ------------------ | ----------------------------------------------------------------- |
| `online()`         | Resolves when the collection has joined and synced with the group |
| `offline()`        | Resolves when the collection loses sync or leadership             |
| `updated()`        | Resolves when any new state version is committed                  |
| `reaches(Version)` | Resolves when committed state reaches at least the given version  |

```rust,ignore
// Typical lifecycle pattern
loop {
    collection.when().online().await;
    println!("online, version = {}", collection.version());

    // ... do work ...

    collection.when().offline().await;
    println!("went offline, waiting to reconnect...");
}
```

## Multiple writers

Multiple nodes can be writers for the same collection simultaneously. All
writes are funneled through Raft consensus, so there is no conflict — every
write is serialized in the log and applied in the same order on all nodes.

```rust,ignore
// Node A
let map = Map::<String, u64>::writer(&network, store_id);
map.insert("from-a".into(), 1).await?;

// Node B (same StoreId)
let map = Map::<String, u64>::writer(&network, store_id);
map.insert("from-b".into(), 2).await?;

// Both nodes see both entries after sync
```

## Choosing writer vs. reader

| Use a Writer when…                           | Use a Reader when…                      |
| -------------------------------------------- | --------------------------------------- |
| The node needs to modify the collection      | The node only observes state            |
| You want normal leadership election priority | You want to reduce leadership overhead  |
| The node is in the "hot path" for writes     | The node is a monitoring/dashboard node |
