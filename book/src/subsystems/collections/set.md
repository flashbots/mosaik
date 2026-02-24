# Set

`Set<T>` is a replicated, unordered, eventually consistent set of unique
values. Internally it is backed by an `im::HashSet` with a deterministic
hasher (SipHash-1-3 with a fixed zero seed), so iteration order is identical
across all nodes.

## Construction

```rust,ignore
use mosaik::collections::{Set, StoreId, SyncConfig};

// Writer — can read and write
let set = Set::<String>::writer(&network, StoreId::from("active-peers"));

// Writer with custom sync config
let set = Set::<String>::writer_with_config(&network, store_id, config);

// Reader — read-only, deprioritized for leadership
let set = Set::<String>::reader(&network, store_id);

// Reader with custom sync config
let set = Set::<String>::reader_with_config(&network, store_id, config);

// Aliases: new() == writer(), new_with_config() == writer_with_config()
let set = Set::<String>::new(&network, store_id);
```

## Read operations

Available on both writers and readers.

| Method                              | Time     | Description                     |
| ----------------------------------- | -------- | ------------------------------- |
| `len() -> usize`                    | O(1)     | Number of elements              |
| `is_empty() -> bool`                | O(1)     | Whether the set is empty        |
| `contains(&T) -> bool`              | O(log n) | Test membership                 |
| `is_subset(&Set<T, W>) -> bool`     | O(n)     | Test subset relationship        |
| `iter() -> impl Iterator<Item = T>` | O(1)*    | Iterate over all elements       |
| `version() -> Version`              | O(1)     | Current committed state version |
| `when() -> &When`                   | O(1)     | Access the state observer       |

\* Iterator creation is O(1); full traversal is O(n).

```rust,ignore
// Membership test
if set.contains(&"node-42".into()) {
    println!("node-42 is active");
}

// Subset check between two sets (can differ in writer/reader mode)
if allowed.is_subset(&active) {
    println!("all allowed nodes are active");
}

// Iteration
for peer in set.iter() {
    println!("active: {peer}");
}
```

## Write operations

Only available on `SetWriter<T>`.

| Method                                                                  | Description                               |
| ----------------------------------------------------------------------- | ----------------------------------------- |
| `insert(T) -> Result<Version, Error<T>>`                                | Insert a value (no-op if already present) |
| `extend(impl IntoIterator<Item = T>) -> Result<Version, Error<Vec<T>>>` | Batch insert                              |
| `remove(&T) -> Result<Version, Error<T>>`                               | Remove a value                            |
| `clear() -> Result<Version, Error<()>>`                                 | Remove all elements                       |

```rust,ignore
// Insert
let v = set.insert("node-1".into()).await?;

// Batch insert
let v = set.extend(["node-2".into(), "node-3".into()]).await?;
set.when().reaches(v).await;

// Remove
set.remove(&"node-1".into()).await?;

// Clear
set.clear().await?;
```

## Error handling

Writes return the failed value on `Error::Offline`:

```rust,ignore
match set.insert(value).await {
    Ok(version) => { /* committed */ }
    Err(Error::Offline(value)) => {
        // Retry later with the same value
    }
    Err(Error::NetworkDown) => {
        // Permanent failure
    }
}
```

## Status & observation

```rust,ignore
set.when().online().await;

let v = set.insert("x".into()).await?;
set.when().reaches(v).await;

set.when().updated().await;
set.when().offline().await;
```

## Group identity

```text
UniqueId::from("mosaik_collections_set")
    .derive(store_id)
    .derive(type_name::<T>())
```
