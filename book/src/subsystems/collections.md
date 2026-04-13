# Collections

The **Collections** subsystem provides replicated, eventually consistent data
structures that feel like local Rust collections but are automatically
synchronized across all participating nodes using Raft consensus.

```text
┌──────────────────────────────────────────────────────────────────────────┐
│                   Collections                                            │
│                                                                          │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌────────┐ ┌──────────┐ ┌──────┐ │
│  │   Map    │ │   Vec    │ │   Set    │ │  DEPQ  │ │ Cell │ │ Once │ │
│  │ <K, V>   │ │ <T>      │ │ <T>      │ │<P,K,V> │ │  <T>     │ │ <T>  │ │
│  └────┬─────┘ └────┬─────┘ └────┬─────┘ └───┬────┘ └────┬─────┘ └──┬───┘ │
│       └────────────┴────────────┴─┬─────────┴───────────┴──────────┘     │
│                                   │                                      │
│                                   │                                      │
│                                   │                                      │
│                  ┌────────────────▼───────────────────┐                  │
│                  │    Groups (Raft Consensus)         │                  │
│                  │    One group per collection        │                  │
│                  └────────────────────────────────────┘                  │
└──────────────────────────────────────────────────────────────────────────┘
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

| Mode       | Type alias                                                                  | Can write? | Leadership priority |
| ---------- | --------------------------------------------------------------------------- | ---------- | ------------------- |
| **Writer** | `MapWriter<K,V>`, `VecWriter<T>`, `CellWriter<T>`, `OnceWriter<T>` etc. | Yes        | Normal              |
| **Reader** | `MapReader<K,V>`, `VecReader<T>`, `CellReader<T>`, `OnceReader<T>` etc. | No         | Deprioritized       |

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
| [`Cell<T>`](collections/cell.md)                  | Single-value cell               | `Option<T>`                   |
| [`Once<T>`](collections/once.md)                          | Write-once cell                 | `Option<T>`                   |
| [`PriorityQueue<P, K, V>`](collections/priority-queue.md) | Double-ended priority queue         | `im::HashMap` + `im::OrdMap`  |

Most collections use the [`im` crate](https://docs.rs/im) for their internal
state, which provides **O(1) structural sharing** — cloning a snapshot of the
entire collection is essentially free. This is critical for the snapshot-based
state sync mechanism. `Cell` and `Once` use a plain `Option<T>` since they
hold at most one value.

## Trait requirements

Collections use blanket-implemented marker traits to constrain their type
parameters:

| Trait        | Required bounds                                                                        | Used by                         |
| ------------ | -------------------------------------------------------------------------------------- | ------------------------------- |
| `Value`      | `Clone + Debug + Serialize + DeserializeOwned + Send + Sync + 'static`                 | All element/value types         |
| `Key`        | `Clone + Serialize + DeserializeOwned + Hash + PartialEq + Eq + Send + Sync + 'static` | Map keys, Set elements, PQ keys |
| `OrderedKey` | `Key + Ord`                                                                            | PriorityQueue priorities        |

These traits are automatically implemented for any type that satisfies their
bounds — you never need to implement them manually.

## Ticket-based authentication

Collections support ticket-based peer authentication through `CollectionConfig`.
This adds an authentication requirement to the underlying Raft group — only
peers carrying valid tickets can join and replicate data.

```rust,ignore
use mosaik::CollectionConfig;
use mosaik::collections::{Map, StoreId};
use mosaik::tickets::{Jwt, Hs256};

let validator = Jwt::with_key(Hs256::new(secret))
    .allow_issuer("my-app");

let config = CollectionConfig::default()
    .require_ticket(validator);

let writer = Map::<String, u64>::writer_with_config(&network, store_id, config);
```

Call `require_ticket` multiple times to require multiple types of tickets.
Peers must satisfy **all** configured validators to participate.

> **Important:** Ticket validators affect the derived group ID. All members
> of the collection must use the same validators in the same order, otherwise
> they will derive different group IDs and will not see each other's changes.

The `collection!` macro supports `require_ticket` directly:

```rust,ignore
use mosaik::tickets::{Jwt, Hs256};

mosaik::collection!(
    pub SecureMap = mosaik::collections::Map<String, String>,
    "secure.store",
    require_ticket: Jwt::with_key(Hs256::new(secret))
        .allow_issuer("my-app"),
);
```

See [Auth Tickets](discovery/tickets.md) for the full JWT API and
[TEE > TDX](tee/tdx.md) for TDX-specific validation.

## StoreId and group identity

Each collection derives its Raft group identity from:

1. **A fixed prefix** per collection type (e.g., `"mosaik_collections_map"`,
   `"mosaik_collections_once"`)
2. **The `StoreId`** — a `UniqueId` you provide at construction time
3. **The Rust type names** of the type parameters
4. **Ticket validator signatures** (optional) — if `require_ticket` is configured

This means `Map::<String, u64>::writer(&net, id)` and
`Map::<u32, u64>::writer(&net, id)` will join **different** groups even with
the same `StoreId`, because the key type differs. Likewise, a `Once<String>`
and a `Cell<String>` with the same `StoreId` form separate groups because
the prefix differs.

## The `collection!` macro (recommended)

The `collection!` macro is the recommended way to declare named collection
definitions. It generates a struct with a compile-time `StoreId` and
implements the `CollectionReader` and/or `CollectionWriter` traits, so you
can create readers and writers without repeating the store ID at every call
site.

### Syntax

```rust,ignore

use mosaik::*;

// Full (reader + writer):
declare::collection!(pub Prices = mosaik::collections::Map<String, f64>, "prices");

// Reader only:
declare::collection!(pub reader Prices = mosaik::collections::Map<String, f64>, "prices");

// Writer only:
declare::collection!(pub writer Prices = mosaik::collections::Map<String, f64>, "prices");

// With generics:
declare::collection!(pub Items<T> = mosaik::collections::Vec<T>, "app.items");
```

The three modes control the public API of the generated struct:

| Mode        | Public                                        | `pub(crate)`                                    |
| ----------- | --------------------------------------------- | ----------------------------------------------- |
| *(default)* | `CollectionReader` **and** `CollectionWriter` | —                                               |
| `reader`    | `CollectionReader`                            | inherent `writer()` / `online_writer()`         |
| `writer`    | `CollectionWriter`                            | inherent `reader()` / `online_reader()`         |

In `reader` or `writer` mode, the restricted side is still available as
inherent methods with `pub(crate)` visibility. This means the crate that
defines the collection can still instantiate both sides internally, while
downstream consumers only see the publicly exported trait.

### Usage

Call the trait methods on the generated struct to create reader or writer
instances:

```rust,ignore
use mosaik::*;

declare::collection!(pub MyVec = mosaik::collections::Vec<String>, "my.vec");

struct MyType {
	reader: ReaderOf<MyVec>,
}

impl MyType {
	pub fn new(network: &mosaik::Network) -> Self {
		Self { reader: MyVec::reader(network) }
	}
}
```

#### `online_reader` / `online_writer`

Each trait also provides a convenience method that creates the handle and
awaits `.when().online()` in a single call:

```rust,ignore
// These two are equivalent:
let reader = MyVec::reader(&network);
reader.when().online().await;

let reader = MyVec::online_reader(&network).await;
```

This is useful when you don't need to do anything between construction and
the online check — it reduces the common two-step pattern to a single
expression.

The `ReaderOf<C>` and `WriterOf<C>` type aliases (re-exported at the crate
root) resolve to the concrete reader or writer type for a given collection
definition:

| Alias         | Expands to                        |
| ------------- | --------------------------------- |
| `ReaderOf<C>` | `<C as CollectionReader>::Reader` |
| `WriterOf<C>` | `<C as CollectionWriter>::Writer` |

### Store ID derivation

The string literal you pass as the last argument is hashed with blake3 at
compile time to produce the `StoreId`. If the string is exactly 64 hex
characters, it is decoded directly instead of hashed. This matches the
behavior of `unique_id!`.

#### Baked-in configuration

The `collection!` macro supports `require_ticket` for ticket-based
authentication:

```rust,ignore
declare::collection!(
    pub Prices = mosaik::collections::Map<String, f64>, "prices",
    require_ticket: MyValidator::new(),
);
```

Multiple `require_ticket` entries can be specified — peers must satisfy
all of them.

### When to use the macro vs. direct constructors

Use the `collection!` macro when:

- Multiple modules or crates reference the same collection.
- You want a single source of truth for the store ID and auth config.
- You want compile-time checked reader/writer type aliases via
  `ReaderOf<C>` / `WriterOf<C>`.

Use direct constructors (`Map::writer(&network, store_id)`) when:

- You only need the collection in one place.
- The store ID is computed at runtime.

## Collection definitions (`CollectionDef`)

> **Note:** The [`collection!` macro](#the-collection-macro-recommended) is
> now the recommended approach for declaring named collection definitions.
> `CollectionDef`, `ReaderDef`, and `WriterDef` still work and are useful
> when you need an explicit value (e.g., passing a definition as a function
> argument), but for most use cases the macro is more ergonomic.

Instead of passing a `StoreId` to each constructor call, you can define a
collection's identity once as a constant and create readers and writers from it.

```rust,ignore
use mosaik::{unique_id, collections::{CollectionDef, ReaderDef, Map}};

// Define the collection identity as a compile-time constant
const PRICES: CollectionDef<Map<String, f64>> =
	CollectionDef::new(unique_id!("prices"));

// Create writer and reader from the definition
let w = PRICES.writer(&network);
let r = PRICES.reader(&network);
```

The type parameter `T` on `CollectionDef<C, T>` encodes the collection's value
types as a tuple. For single-type collections it is the type itself; for
multi-type collections it is a tuple:

| Collection               | `T` parameter |
| ------------------------ | ------------- |
| `Map<K, V>`              | `(K, V)`      |
| `Vec<T>`                 | `T`           |
| `Set<T>`                 | `T`           |
| `Cell<T>`            | `T`           |
| `Once<T>`                | `T`           |
| `PriorityQueue<P, K, V>` | `(P, K, V)`   |

### Exporting reader-only or writer-only definitions

`CollectionDef` can produce narrowed definitions that expose only one side of
the writer/reader split. This is the recommended pattern when a library owns a
collection and wants to let consumers read from it without granting write
access:

```rust,ignore
// In your library crate:
pub const PRICES: CollectionDef<Map<String, f64>> =
	CollectionDef::new(unique_id!("prices"));

// Export only a reader definition for downstream consumers
pub const PRICES_READER: ReaderDef<Map<String, f64>> = PRICES.as_reader();
```

Downstream code uses `PRICES_READER` without needing the `StoreId`:

```rust,ignore
let reader = my_lib::PRICES_READER.open(&network);
reader.when().online().await;
```

A `WriterDef` works the same way via `as_writer()`.

All three definition types (`CollectionDef`, `ReaderDef`, `WriterDef`) are
`const`-constructible and can be used as top-level constants. See
[Writer/Reader Pattern](collections/writer-reader.md) for more on the
writer/reader split.

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
