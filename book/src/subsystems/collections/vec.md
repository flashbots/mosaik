# Vec

`Vec<T>` is a replicated, ordered, index-addressable sequence. Internally it
is backed by `im::Vector<T>`, which provides efficient push/pop at both ends
and O(log n) random access.

## Construction

```rust,ignore
use mosaik::collections::{Vec, StoreId, SyncConfig};

// Writer — can read and write
let vec = Vec::<String>::writer(&network, StoreId::from("events"));

// Writer with custom sync config
let vec = Vec::<String>::writer_with_config(&network, store_id, config);

// Reader — read-only, deprioritized for leadership
let vec = Vec::<String>::reader(&network, store_id);

// Reader with custom sync config
let vec = Vec::<String>::reader_with_config(&network, store_id, config);

// Aliases: new() == writer(), new_with_config() == writer_with_config()
let vec = Vec::<String>::new(&network, store_id);
```

## Read operations

Available on both writers and readers. Reads operate on the local committed
state and never touch the network.

| Method                              | Time     | Description                      |
| ----------------------------------- | -------- | -------------------------------- |
| `len() -> usize`                    | O(1)     | Number of elements               |
| `is_empty() -> bool`                | O(1)     | Whether the vector is empty      |
| `get(u64) -> Option<T>`             | O(log n) | Get element at index             |
| `front() -> Option<T>`              | O(log n) | First element                    |
| `head() -> Option<T>`               | O(log n) | Alias for `front()`              |
| `back() -> Option<T>`               | O(log n) | Last element                     |
| `last() -> Option<T>`               | O(log n) | Alias for `back()`               |
| `contains(&T) -> bool`              | O(n)     | Test if a value is in the vector |
| `index_of(&T) -> Option<u64>`       | O(n)     | Find the index of a value        |
| `iter() -> impl Iterator<Item = T>` | O(1)*    | Iterate over all elements        |
| `version() -> Version`              | O(1)     | Current committed state version  |
| `when() -> &When`                   | O(1)     | Access the state observer        |

\* Iterator creation is O(1) due to structural sharing; full traversal is O(n).

```rust,ignore
// Random access
if let Some(event) = vec.get(0) {
    println!("First event: {event}");
}

// Peek at ends
let newest = vec.back();
let oldest = vec.front();

// Linear search
if let Some(idx) = vec.index_of(&"restart".into()) {
    println!("Found restart at index {idx}");
}
```

## Write operations

Only available on `VecWriter<T>`. All writes go through Raft consensus.

| Method                                                                  | Time     | Description                              |
| ----------------------------------------------------------------------- | -------- | ---------------------------------------- |
| `push_back(T) -> Result<Version, Error<T>>`                             | O(1)*    | Append to end                            |
| `push_front(T) -> Result<Version, Error<T>>`                            | O(1)*    | Prepend to start                         |
| `insert(u64, T) -> Result<Version, Error<T>>`                           | O(log n) | Insert at index, shifting elements right |
| `extend(impl IntoIterator<Item = T>) -> Result<Version, Error<Vec<T>>>` | O(k)*    | Batch append                             |
| `pop_back() -> Result<Version, Error<()>>`                              | O(1)*    | Remove last element                      |
| `pop_front() -> Result<Version, Error<()>>`                             | O(1)*    | Remove first element                     |
| `remove(u64) -> Result<Version, Error<u64>>`                            | O(log n) | Remove at index, shifting elements left  |
| `swap(u64, u64) -> Result<Version, Error<()>>`                          | O(log n) | Swap two elements                        |
| `truncate(usize) -> Result<Version, Error<()>>`                         | O(log n) | Keep only the first `n` elements         |
| `clear() -> Result<Version, Error<()>>`                                 | O(1)     | Remove all elements                      |

\* Amortized time complexity.

```rust,ignore
// Append
let v = vec.push_back("event-1".into()).await?;

// Prepend
vec.push_front("event-0".into()).await?;

// Batch append
let v = vec.extend(["a".into(), "b".into(), "c".into()]).await?;

// Random insert
vec.insert(2, "inserted".into()).await?;

// Remove from ends
vec.pop_back().await?;
vec.pop_front().await?;

// Remove at index
vec.remove(0).await?;

// Swap positions
vec.swap(0, 1).await?;

// Truncate and clear
vec.truncate(10).await?;
vec.clear().await?;
```

## Error handling

Writes return the failed value on `Error::Offline` so you can retry:

```rust,ignore
match vec.push_back(item).await {
    Ok(version) => {
        vec.when().reaches(version).await;
    }
    Err(Error::Offline(item)) => {
        // Retry with the same item later
    }
    Err(Error::NetworkDown) => {
        // Permanent failure
    }
}
```

## Status & observation

```rust,ignore
// Wait until online
vec.when().online().await;

// Wait for a specific committed version
let v = vec.push_back("x".into()).await?;
vec.when().reaches(v).await;

// Wait for any update
vec.when().updated().await;

// React to going offline
vec.when().offline().await;
```

## Group identity

The group key for a `Vec<T>` is derived from:

```text
UniqueId::from("mosaik_collections_vec")
    .derive(store_id)
    .derive(type_name::<T>())
```

Two vectors with the same `StoreId` but different element types will be in
separate consensus groups.
