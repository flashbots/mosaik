# Register

`Register<T>` is a replicated single-value register. It holds at most one
value at a time — writing a new value replaces the previous one entirely. This
is the distributed equivalent of a `tokio::sync::watch` channel: all nodes
observe the latest value.

Internally the state is simply an `Option<T>`, making the Register the
simplest collection in the mosaik toolkit.

## Construction

```rust,ignore
use mosaik::collections::{Register, StoreId, SyncConfig};

// Writer — can read and write
let reg = Register::<String>::writer(&network, StoreId::from("config"));

// Writer with custom sync config
let reg = Register::<String>::writer_with_config(&network, store_id, config);

// Reader — read-only, deprioritized for leadership
let reg = Register::<String>::reader(&network, store_id);

// Reader with custom sync config
let reg = Register::<String>::reader_with_config(&network, store_id, config);

// Aliases: new() == writer(), new_with_config() == writer_with_config()
let reg = Register::<String>::new(&network, store_id);
```

## Read operations

Available on both writers and readers. Reads operate on the local committed
state and never touch the network.

| Method                 | Time | Description                        |
| ---------------------- | ---- | ---------------------------------- |
| `read() -> Option<T>`  | O(1) | Get the current value              |
| `get() -> Option<T>`   | O(1) | Alias for `read()`                 |
| `is_empty() -> bool`   | O(1) | Whether the register holds a value |
| `version() -> Version` | O(1) | Current committed state version    |
| `when() -> &When`      | O(1) | Access the state observer          |

```rust,ignore
// Read the current value
if let Some(config) = reg.read() {
    println!("Current config: {config}");
}

// Check if a value has been written
if reg.is_empty() {
    println!("No configuration set yet");
}
```

## Write operations

Only available on `RegisterWriter<T>`.

| Method                                                                                     | Time | Description                                 |
| ------------------------------------------------------------------------------------------ | ---- | ------------------------------------------- |
| `write(T) -> Result<Version, Error<T>>`                                                    | O(1) | Write a value (replaces any existing value) |
| `set(T) -> Result<Version, Error<T>>`                                                      | O(1) | Alias for `write()`                         |
| `compare_exchange(Option<T>, Option<T>) -> Result<Version, Error<(Option<T>, Option<T>)>>` | O(1) | Atomic compare-and-swap                     |
| `clear() -> Result<Version, Error<()>>`                                                    | O(1) | Remove the stored value                     |

```rust,ignore
// Write a value
let v = reg.write("v1".into()).await?;

// Overwrite (replaces previous value)
let v = reg.write("v2".into()).await?;
reg.when().reaches(v).await;

// Using the alias
reg.set("v3".into()).await?;

// Atomic compare-and-swap: only writes if current value matches expected
let v = reg.compare_exchange(Some("v3".into()), Some("v4".into())).await?;
reg.when().reaches(v).await;
assert_eq!(reg.read(), Some("v4".to_string()));

// Compare-and-swap from empty to a value
reg.clear().await?;
let v = reg.compare_exchange(None, Some("first".into())).await?;
reg.when().reaches(v).await;

// Compare-and-swap to remove (set to None)
let v = reg.compare_exchange(Some("first".into()), None).await?;
reg.when().reaches(v).await;
assert!(reg.is_empty());

// Clear back to empty
reg.clear().await?;
assert!(reg.is_empty());
```

### Compare-and-swap semantics

`compare_exchange` atomically checks the current value of the register and
only applies the write if it matches the `current` parameter. This is useful
for optimistic concurrency control — multiple writers can attempt a swap, and
only the one whose expectation matches the actual state will succeed.

- **`current`**: The expected current value (`None` means the register must be
  empty).
- **`new`**: The value to write if the expectation holds (`None` clears the
  register).

If the current value does not match `current`, the operation is a **no-op** —
it still commits to the Raft log (incrementing the version) but does not
change the stored value.

## Error handling

Writes return the failed value on `Error::Offline` so you can retry:

```rust,ignore
match reg.write(value).await {
    Ok(version) => {
        reg.when().reaches(version).await;
    }
    Err(Error::Offline(value)) => {
        // Retry with the same value later
    }
    Err(Error::NetworkDown) => {
        // Permanent failure
    }
}
```

## Status & observation

```rust,ignore
// Wait until online
reg.when().online().await;

// Wait for a specific committed version
let v = reg.write("new-config".into()).await?;
reg.when().reaches(v).await;

// Wait for any update
reg.when().updated().await;

// React to going offline
reg.when().offline().await;
```

## Group identity

The group key for a `Register<T>` is derived from:

```text
UniqueId::from("mosaik_collections_register")
    .derive(store_id)
    .derive(type_name::<T>())
```

Two registers with the same `StoreId` but different value types will be in
separate consensus groups.

## When to use Register

Register is ideal for:

- **Configuration** — a single shared config object replicated across the cluster
- **Leader state** — the current leader's address or identity
- **Latest snapshot** — the most recent version of a computed result
- **Feature flags** — a single boolean or enum toggled cluster-wide

If you need to store multiple values, use [`Map`](map.md), [`Vec`](vec.md),
or [`Set`](set.md) instead.
