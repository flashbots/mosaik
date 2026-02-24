# Deterministic Hashing

In a distributed replicated system, all peers must arrive at the **exact same
state** after applying the same sequence of operations. Mosaik's collections
use deterministic hashing to guarantee consistent behavior across nodes.

## The problem

Rust's default `HashMap` and `HashSet` use **RandomState** — a hasher seeded
with random data at process startup. This means:

- Iteration order differs between processes.
- Two nodes applying the same inserts will have different internal layouts.
- Snapshots of hash-based structures would differ even with identical contents.

For replicated state machines, this is unacceptable.

## The solution

Mosaik's `Map` and `Set` collections use a **zero-seeded deterministic
hasher**:

```rust,ignore
type DeterministicHasher = BuildHasherDefault<DefaultHasher>;
```

`BuildHasherDefault<DefaultHasher>` constructs a `DefaultHasher` (SipHash)
with a fixed zero seed on every call. This ensures:

1. **Same input → same hash** across all nodes and restarts.
2. **Same insertion order → same internal layout** in the hash table.
3. **Snapshots are byte-identical** when state is identical.

## Where it's used

### Collections

All hash-based collections use the deterministic hasher internally:

- `Map<K, V>` uses `im::HashMap<K, V, DeterministicHasher>`.
- `Set<V>` uses `im::HashSet<V, DeterministicHasher>`.

The `im` crate's persistent hash structures accept a custom hasher parameter,
which mosaik sets to the zero-seeded variant.

### Ordered collections

`Vec` and `PriorityQueue` do not use hashing for their primary index:

- `Vec` uses `im::Vector` (a balanced tree indexed by position).
- `PriorityQueue` uses:
  - `im::HashMap` (deterministic hasher) for key → value+priority lookup.
  - `im::OrdMap` for priority-ordered access (requires `Ord`, not hashing).

## Identity derivation

Beyond collection hashing, determinism matters for **identity**:

### UniqueId

Group and store identities are derived deterministically from their inputs:

```rust,ignore
// GroupId is derived from key + network
let group_id = UniqueId::new(&key, &network_id);

// StoreId is derived from group + type signature
let store_id = StoreId::new(&group_id, &type_signature);
```

This uses `blake3` hashing, which is inherently deterministic.

### Type signatures

Collection state machines compute a **signature** from their Rust type names:

```rust,ignore
fn signature() -> Digest {
    Digest::from(
        std::any::type_name::<MapStateMachine<K, V>>()
    )
}
```

This ensures that two collections are only considered the same store if they
have identical key and value types. A `Map<String, u64>` and a
`Map<String, i64>` will have different `StoreId` values and will never
attempt to sync with each other.

## Snapshot consistency

Deterministic hashing is critical for snapshot-based state sync:

```text
Node A state: { "alice" → 1, "bob" → 2 }
Node B state: { "alice" → 1, "bob" → 2 }

With deterministic hashing:
  Node A snapshot bytes == Node B snapshot bytes  ✓

With random hashing:
  Node A snapshot bytes != Node B snapshot bytes  ✗
  (different internal bucket layout)
```

When a joining peer fetches snapshot batches from multiple source peers, the
items must be compatible. Deterministic hashing ensures all sources produce
the same serialized representation for identical logical state.

## The `im` crate

Mosaik uses the [`im`](https://docs.rs/im) crate for persistent
(copy-on-write) data structures. Key properties:

| Property               | Benefit                                                                    |
| ---------------------- | -------------------------------------------------------------------------- |
| **Structural sharing** | O(1) snapshot via `clone()` — only divergent nodes are copied              |
| **Custom hasher**      | Accepts `BuildHasherDefault<DefaultHasher>` for determinism                |
| **Thread-safe clones** | `Arc`-based sharing, safe to snapshot from one task and iterate in another |
| **Balanced trees**     | `OrdMap` and `Vector` use RRB trees with O(log n) operations               |

The O(1) cloning is especially important for state sync — creating a snapshot
does not block the state machine from processing new commands.

## Trait requirements

The deterministic hashing strategy imposes trait bounds on collection keys and
values:

| Trait                          | Required by                | Purpose                                      |
| ------------------------------ | -------------------------- | -------------------------------------------- |
| `Hash`                         | `Map` keys, `Set` values   | Deterministic bucket placement               |
| `Eq`                           | `Map` keys, `Set` values   | Equality comparison for collision resolution |
| `Clone`                        | All keys and values        | Structural sharing in `im` data structures   |
| `Serialize + DeserializeOwned` | All keys and values        | Snapshot and replication encoding            |
| `Send + Sync + 'static`        | All keys and values        | Cross-task sharing                           |
| `Ord`                          | `PriorityQueue` priorities | Ordered access in `OrdMap`                   |

These are codified as blanket trait aliases:

```rust,ignore
// Satisfied by types that are Hash + Eq + Clone + Serialize + DeserializeOwned + Send + Sync + 'static
trait Key {}

// Adds Ord to Key requirements
trait OrderedKey {}

// Clone + Serialize + DeserializeOwned + Send + Sync + 'static (no Hash/Eq)
trait Value {}
```
