# PriorityQueue

`PriorityQueue<P, K, V>` is a replicated, eventually consistent double-ended
priority queue (DEPQ). Each entry has a **priority** (`P`), a unique **key**
(`K`), and a **value** (`V`). It supports efficient access to both the minimum
and maximum priority elements, key-based lookups, priority updates, and range
removals.

Internally it maintains two indexes:

- `by_key: im::HashMap<K, (P, V)>` — O(log n) key lookups
- `by_priority: im::OrdMap<P, im::HashMap<K, V>>` — O(log n) min/max and range
  operations

Both indexes use deterministic hashers for consistent iteration order across
nodes.

## Construction

```rust,ignore
use mosaik::collections::{PriorityQueue, StoreId, SyncConfig};

// Writer — can read and write
let pq = PriorityQueue::<u64, String, Order>::writer(
    &network,
    StoreId::from("orderbook"),
);

// Writer with custom sync config
let pq = PriorityQueue::<u64, String, Order>::writer_with_config(
    &network, store_id, config,
);

// Reader — read-only, deprioritized for leadership
let pq = PriorityQueue::<u64, String, Order>::reader(&network, store_id);

// Reader with custom sync config
let pq = PriorityQueue::<u64, String, Order>::reader_with_config(
    &network, store_id, config,
);

// Aliases
let pq = PriorityQueue::<u64, String, Order>::new(&network, store_id);
let pq = PriorityQueue::<u64, String, Order>::new_with_config(
    &network, store_id, config,
);
```

## Read operations

Available on both writers and readers.

| Method                                           | Time     | Description                                     |
| ------------------------------------------------ | -------- | ----------------------------------------------- |
| `len() -> usize`                                 | O(1)     | Number of entries                               |
| `is_empty() -> bool`                             | O(1)     | Whether the queue is empty                      |
| `contains_key(&K) -> bool`                       | O(log n) | Test if a key exists                            |
| `get(&K) -> Option<V>`                           | O(log n) | Get the value for a key                         |
| `get_priority(&K) -> Option<P>`                  | O(log n) | Get the priority for a key                      |
| `get_min() -> Option<(P, K, V)>`                 | O(log n) | Entry with the lowest priority                  |
| `get_max() -> Option<(P, K, V)>`                 | O(log n) | Entry with the highest priority                 |
| `min_priority() -> Option<P>`                    | O(log n) | Lowest priority value                           |
| `max_priority() -> Option<P>`                    | O(log n) | Highest priority value                          |
| `iter() -> impl Iterator<Item = (P, K, V)>`      | —        | Ascending priority order (alias for `iter_asc`) |
| `iter_asc() -> impl Iterator<Item = (P, K, V)>`  | —        | Ascending priority order                        |
| `iter_desc() -> impl Iterator<Item = (P, K, V)>` | —        | Descending priority order                       |
| `version() -> Version`                           | O(1)     | Current committed state version                 |
| `when() -> &When`                                | O(1)     | Access the state observer                       |

When multiple entries share the same priority, `get_min()` and `get_max()`
return an arbitrary entry from that priority bucket.

```rust,ignore
// Peek at extremes
if let Some((priority, key, value)) = pq.get_min() {
    println!("Best bid: {key} at priority {priority}");
}

// Look up by key
let price = pq.get_priority(&"order-42".into());

// Iterate in order
for (priority, key, value) in pq.iter_desc() {
    println!("{priority}: {key} = {value:?}");
}
```

## Write operations

Only available on `PriorityQueueWriter<P, K, V>`.

| Method                                                                                  | Description                            |
| --------------------------------------------------------------------------------------- | -------------------------------------- |
| `insert(P, K, V) -> Result<Version, Error<(P, K, V)>>`                                  | Insert or update an entry              |
| `extend(impl IntoIterator<Item = (P, K, V)>) -> Result<Version, Error<Vec<(P, K, V)>>>` | Batch insert                           |
| `update_priority(&K, P) -> Result<Version, Error<K>>`                                   | Change priority of an existing key     |
| `update_value(&K, V) -> Result<Version, Error<K>>`                                      | Change value of an existing key        |
| `compare_exchange_value(&K, V, Option<V>) -> Result<Version, Error<K>>`                 | Atomic compare-and-swap on value       |
| `remove(&K) -> Result<Version, Error<K>>`                                               | Remove by key                          |
| `remove_range(impl RangeBounds<P>) -> Result<Version, Error<()>>`                       | Remove all entries in a priority range |
| `clear() -> Result<Version, Error<()>>`                                                 | Remove all entries                     |

If `insert` is called with a key that already exists, both its priority and
value are updated. `update_priority` and `update_value` are no-ops if the key
doesn't exist (they still commit to the log).

```rust,ignore
// Insert
let v = pq.insert(100, "order-1".into(), order).await?;

// Batch insert
let v = pq.extend([
    (100, "order-1".into(), order1),
    (200, "order-2".into(), order2),
]).await?;

// Update just the priority
pq.update_priority(&"order-1".into(), 150).await?;

// Update just the value
pq.update_value(&"order-1".into(), new_order).await?;

// Atomic compare-and-swap on value (priority is preserved)
let v = pq.compare_exchange_value(
    &"order-1".into(),
    order1,        // expected current value
    Some(updated), // new value
).await?;

// Compare-and-swap to remove: expected matches, new is None
let v = pq.compare_exchange_value(
    &"order-1".into(),
    updated,       // expected current value
    None,          // removes the entry
).await?;

// Remove a single entry
pq.remove(&"order-2".into()).await?;

// Remove all entries with priority below 50
pq.remove_range(..50u64).await?;

// Remove entries in a range
pq.remove_range(10..=20).await?;

// Clear everything
pq.clear().await?;
```

### Range syntax

`remove_range` accepts any `RangeBounds<P>`, so all standard Rust range
syntaxes work:

| Syntax      | Meaning                         |
| ----------- | ------------------------------- |
| `..cutoff`  | Priorities below `cutoff`       |
| `..=cutoff` | Priorities at or below `cutoff` |
| `cutoff..`  | Priorities at or above `cutoff` |
| `lo..hi`    | Priorities in `[lo, hi)`        |
| `lo..=hi`   | Priorities in `[lo, hi]`        |
| `..`        | All (equivalent to `clear()`)   |

### Compare-and-swap semantics

`compare_exchange_value` atomically checks the value of an existing entry and
only applies the mutation if it matches the `expected` parameter. Unlike
`compare_exchange` on `Map` and `Register`, this method operates **only on the
value** — the entry's priority is always preserved.

- **`key`**: The key of the entry to operate on (must already exist).
- **`expected`**: The expected current value (type `V`, not `Option<V>` — the
  key must exist for the exchange to succeed).
- **`new`**: The replacement value. `Some(v)` updates the value in-place;
  `None` removes the entire entry.

If the key does not exist or its current value does not match `expected`, the
operation is a **no-op** — it commits to the Raft log but does not change the
queue.

> **Note:** The entry's priority is never changed by `compare_exchange_value`.
> To atomically update priorities, use `update_priority` instead.

## Error handling

Same pattern as other collections — failed values are returned for retry:

```rust,ignore
match pq.insert(priority, key, value).await {
    Ok(version) => { /* committed */ }
    Err(Error::Offline((priority, key, value))) => {
        // Retry later
    }
    Err(Error::NetworkDown) => {
        // Permanent failure
    }
}
```

## Status & observation

```rust,ignore
pq.when().online().await;

let v = pq.insert(100, "k".into(), val).await?;
pq.when().reaches(v).await;

pq.when().updated().await;
pq.when().offline().await;
```

## Dual-index architecture

The DEPQ maintains two synchronized indexes:

```text
by_key:      HashMap<K, (P, V)>     ← key lookups, membership tests
by_priority: OrdMap<P, HashMap<K, V>> ← min/max, range ops, ordered iteration
```

When a key is inserted or updated, both indexes are updated atomically within
the state machine's `apply_batch`. During snapshot sync, only the `by_key`
index is serialized; the `by_priority` index is reconstructed on the receiving
side during `append`.

## Group identity

```text
UniqueId::from("mosaik_collections_depq")
    .derive(store_id)
    .derive(type_name::<P>())
    .derive(type_name::<K>())
    .derive(type_name::<V>())
```
