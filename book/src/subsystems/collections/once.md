# Once

`Once<T>` is a replicated write-once cell. It holds at most one value —
once a value has been set, all subsequent writes are silently ignored by the
state machine. This is the distributed equivalent of a
`tokio::sync::OnceCell`.

Unlike [`Cell<T>`](cell.md), which allows overwriting the stored value,
`Once<T>` guarantees that the first accepted write is permanent. This makes it
ideal for one-time initialization patterns where you need exactly one value to
win across a distributed cluster.

Internally the state is an `Option<T>`, identical to `Cell`, but the
`apply` logic rejects writes when the value is already `Some`.

## Construction

```rust,ignore
use mosaik::collections::{Once, StoreId, SyncConfig};

// Writer — can read and write
let once = Once::<String>::writer(&network, StoreId::from("config-seed"));

// Writer with custom sync config
let once = Once::<String>::writer_with_config(&network, store_id, config);

// Reader — read-only, deprioritized for leadership
let once = Once::<String>::reader(&network, store_id);

// Reader with custom sync config
let once = Once::<String>::reader_with_config(&network, store_id, config);

// Aliases: new() == writer(), new_with_config() == writer_with_config()
let once = Once::<String>::new(&network, store_id);
```

## Read operations

Available on both writers and readers. Reads operate on the local committed
state and never touch the network.

| Method                 | Time | Description                                 |
| ---------------------- | ---- | ------------------------------------------- |
| `read() -> Option<T>`  | O(1) | Get the current value                       |
| `get() -> Option<T>`   | O(1) | Alias for `read()`                          |
| `is_empty() -> bool`   | O(1) | Whether the cell has been set           |
| `is_none() -> bool`    | O(1) | Alias for `is_empty()`                      |
| `is_some() -> bool`    | O(1) | Whether the cell holds a value          |
| `version() -> Version` | O(1) | Current committed state version             |
| `when() -> &When`      | O(1) | Access the state observer                   |

```rust,ignore
// Read the current value
if let Some(seed) = once.read() {
    println!("Config seed: {seed}");
}

// Check if a value has been written
if once.is_none() {
    println!("No seed set yet");
}

// Equivalent check
assert_eq!(once.is_empty(), once.is_none());
assert_eq!(once.is_some(), !once.is_empty());
```

## Write operations

Only available on `OnceWriter<T>`.

| Method                                      | Time | Description                                      |
| ------------------------------------------- | ---- | ------------------------------------------------ |
| `write(T) -> Result<Version, Error<T>>`     | O(1) | Set the value (ignored if already set)           |
| `set(T) -> Result<Version, Error<T>>`       | O(1) | Alias for `write()`                              |

The key behavior: **only the first write takes effect.** If a value has already
been set, subsequent writes are silently ignored by the state machine. The
returned `Version` still advances (the command is committed to the Raft log),
but the stored value does not change.

```rust,ignore
// First write — this value is stored permanently
let v = once.write("genesis".into()).await?;
once.when().reaches(v).await;
assert_eq!(once.read(), Some("genesis".into()));

// Second write — silently ignored
let v2 = once.write("overwrite-attempt".into()).await?;
once.when().reaches(v2).await;
assert_eq!(once.read(), Some("genesis".into())); // still "genesis"

// Using the alias
once.set("another-attempt".into()).await?;
assert_eq!(once.read(), Some("genesis".into())); // unchanged
```

## Error handling

Writes return the failed value on `Error::Offline` so you can retry:

```rust,ignore
match once.write(value).await {
    Ok(version) => {
        once.when().reaches(version).await;
    }
    Err(Error::Offline(value)) => {
        // Retry with the same value later
    }
    Err(Error::Encoding(value, err)) => {
        // Serialization failed
    }
    Err(Error::NetworkDown) => {
        // Permanent failure
    }
}
```

## Status & observation

```rust,ignore
// Wait until online
once.when().online().await;

// Wait for a specific committed version
let v = once.write("init".into()).await?;
once.when().reaches(v).await;

// Wait for any update
once.when().updated().await;

// React to going offline
once.when().offline().await;
```

## Group identity

The group key for a `Once<T>` is derived from:

```text
UniqueId::from("mosaik_collections_once")
    .derive(store_id)
    .derive(type_name::<T>())
```

A `Once<T>` and a `Cell<T>` with the same `StoreId` and type parameter
will be in **separate** consensus groups because the prefix differs
(`"mosaik_collections_once"` vs `"mosaik_collections_register"`).

## Once vs Cell

| Behavior            | `Once<T>`                               | `Cell<T>`                         |
| ------------------- | --------------------------------------- | ------------------------------------- |
| Write semantics     | First write wins; subsequent ignored    | Every write replaces the stored value |
| `clear()`           | Not supported                           | Supported                             |
| `compare_exchange`  | Not supported                           | Supported                             |
| Distributed analog  | `tokio::sync::OnceCell`                 | `tokio::sync::watch`                  |
| State machine sig   | `"mosaik_collections_once"`             | `"mosaik_collections_register"`       |

## When to use Once

`Once` is ideal for:

- **One-time initialization** — a genesis config, cluster seed, or bootstrap
  value that should only be set once
- **Distributed elections** — the first node to write a value claims a slot
- **Immutable assignments** — assigning a resource or identity permanently
- **Configuration anchors** — values that must not change after initial
  deployment

If you need to update the value after setting it, use
[`Cell`](cell.md) instead.
