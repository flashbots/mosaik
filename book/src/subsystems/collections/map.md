# Map

`Map<K, V>` is a replicated, unordered, eventually consistent key-value map.
Internally it is backed by an `im::HashMap` with a deterministic hasher.

## Construction

```rust,ignore
use mosaik::collections::{Map, StoreId, SyncConfig};

// Writer — can read and write
let map = Map::<String, u64>::writer(&network, StoreId::from("balances"));

// Writer with custom sync config
let map = Map::<String, u64>::writer_with_config(&network, store_id, config);

// Reader — read-only, deprioritized for leadership
let map = Map::<String, u64>::reader(&network, store_id);

// Reader with custom sync config
let map = Map::<String, u64>::reader_with_config(&network, store_id, config);

// Aliases: new() == writer(), new_with_config() == writer_with_config()
let map = Map::<String, u64>::new(&network, store_id);
```

## Read operations

Available on both writers and readers. All reads operate on the **local
committed state** — they are non-blocking and never touch the network.

| Method                                   | Time     | Description                        |
| ---------------------------------------- | -------- | ---------------------------------- |
| `len() -> usize`                         | O(1)     | Number of entries                  |
| `is_empty() -> bool`                     | O(1)     | Whether the map is empty           |
| `contains_key(&K) -> bool`               | O(log n) | Test if a key exists               |
| `get(&K) -> Option<V>`                   | O(log n) | Get a clone of the value for a key |
| `iter() -> impl Iterator<Item = (K, V)>` | O(1)*    | Iterate over all key-value pairs   |
| `keys() -> impl Iterator<Item = K>`      | O(1)*    | Iterate over all keys              |
| `values() -> impl Iterator<Item = V>`    | O(1)*    | Iterate over all values            |
| `version() -> Version`                   | O(1)     | Current committed state version    |
| `when() -> &When`                        | O(1)     | Access the state observer          |

\* Iterator creation is O(1) due to `im::HashMap`'s structural sharing; full
iteration is O(n).

```rust,ignore
// Read the current state
if let Some(balance) = map.get(&"alice".into()) {
    println!("Alice's balance: {balance}");
}

// Snapshot iteration — takes a structural clone, then iterates
for (key, value) in map.iter() {
    println!("{key}: {value}");
}
```

## Write operations

Only available on `MapWriter<K, V>`. All writes go through Raft consensus and
return the `Version` at which the mutation will be committed.

| Method                                                                            | Description                       |
| --------------------------------------------------------------------------------- | --------------------------------- |
| `insert(K, V) -> Result<Version, Error<(K, V)>>`                                  | Insert or update a key-value pair |
| `remove(&K) -> Result<Version, Error<K>>`                                         | Remove a key                      |
| `extend(impl IntoIterator<Item = (K, V)>) -> Result<Version, Error<Vec<(K, V)>>>` | Batch insert                      |
| `clear() -> Result<Version, Error<()>>`                                           | Remove all entries                |

```rust,ignore
// Insert a single entry
let v = map.insert("ETH".into(), 3812).await?;

// Batch insert
let v = map.extend([
    ("BTC".into(), 105_000),
    ("SOL".into(), 178),
]).await?;

// Wait for the batch to commit
map.when().reaches(v).await;

// Remove
map.remove(&"SOL".into()).await?;

// Clear everything
map.clear().await?;
```

## Error handling

On failure, the values you attempted to write are returned inside the error so
you can retry without re-creating them:

```rust,ignore
match map.insert("key".into(), expensive_value).await {
    Ok(version) => println!("committed at {version}"),
    Err(Error::Offline((key, value))) => {
        // Node is temporarily offline — retry later
        println!("offline, got {key} and {value} back");
    }
    Err(Error::NetworkDown) => {
        // Permanent failure
    }
}
```

## Status & observation

```rust,ignore
// Wait until the map is online
map.when().online().await;

// Wait for a specific version
let v = map.insert("x".into(), 1).await?;
map.when().reaches(v).await;

// Wait for any new commit
map.when().updated().await;

// Detect going offline
map.when().offline().await;
```

## Group identity

The group key for a `Map<K, V>` is derived from:

```text
UniqueId::from("mosaik_collections_map")
    .derive(store_id)
    .derive(type_name::<K>())
    .derive(type_name::<V>())
```

Two maps with the same `StoreId` but different key or value types will be in
completely separate consensus groups.
