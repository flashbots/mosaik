# Primitives Reference

This chapter documents the foundational types and utilities that the rest of
mosaik builds on. These live in the `primitives` module (public types) and
internal helpers.

## `Digest` / `UniqueId` / `Tag`

The core identifier type — a 32-byte blake3 hash.

```rust,ignore
use mosaik::Digest;  // re-exported at crate root
```

`Digest`, `UniqueId`, and `Tag` are all the same type, aliased for clarity in
different contexts:

| Alias       | Used for                                   |
| ----------- | ------------------------------------------ |
| `Digest`    | General-purpose 32-byte identifier         |
| `UniqueId`  | Derived identity (groups, stores, bonds)   |
| `Tag`       | Discovery tags for peer classification     |
| `NetworkId` | Network identifier (`= Digest`)            |
| `GroupId`   | Group identifier (`= Digest`)              |
| `StreamId`  | Stream identifier (`= Digest`)             |
| `StoreId`   | Collection store identifier (`= UniqueId`) |

### Construction

```rust,ignore
// From a string (hashes the string with blake3)
let tag = Digest::from("validator");

// From a hex string (64 chars = direct decode, otherwise hashed)
let id = Digest::from("a1b2c3d4...");

// From numeric types (little-endian encoded, then hashed)
let id = Digest::from_u64(42);

// From multiple parts (concatenated, then hashed)
let id = Digest::from_parts(&["prefix", "suffix"]);

// Derive a child ID deterministically
let child = parent.derive("child-name");

// Random
let id = Digest::random();

// Zero (sentinel value)
let zero = Digest::zero();
```

### Compile-time construction

```rust,ignore
use mosaik::unique_id;

const MY_ID: UniqueId = unique_id!("a1b2c3d4e5f6...");  // 64-char hex literal
```

### Display

- `Display` — short hex (first 5 bytes): `a1b2c3d4e5`
- `Debug` — full 64-character hex string
- `Short<T>` wrapper — always shows first 5 bytes
- `Abbreviated<const LEN, T>` — shows `first..last` if longer than LEN

### Serialization

- **Human-readable** formats (JSON, TOML): hex string
- **Binary** formats (postcard): raw 32 bytes

---

## `PeerId`

```rust,ignore
type PeerId = iroh::EndpointId;
```

A peer's public identity, derived from their Ed25519 public key. This is an
iroh type, not a mosaik `Digest`. It is used in discovery, bonds, and
connection authentication.

## `SecretKey`

Re-exported from iroh. An Ed25519 secret key that determines a node's
`PeerId`. If not provided to `NetworkBuilder`, a random key is generated
automatically — this is the recommended default for regular nodes.

Specifying a fixed secret key is only recommended for **bootstrap nodes**
that need a stable, well-known peer ID across restarts:

```rust,ignore
use mosaik::SecretKey;

// Auto-generated (default) — new identity each run
let network = Network::builder()
    .network_id("my-network")
    .build().await?;

// Fixed key — stable identity, recommended only for bootstrap nodes
let key = SecretKey::generate(&mut rand::rng());
let bootstrap = Network::builder()
    .network_id("my-network")
    .secret_key(key)
    .build().await?;
```

---

## Wire encoding

All network messages use **postcard** — a compact, `#[no_std]`-compatible
binary format using variable-length integers.

| Function                                                          | Description                            |
| ----------------------------------------------------------------- | -------------------------------------- |
| `serialize<T: Serialize>(&T) -> Bytes`                            | Serialize to bytes (panics on failure) |
| `deserialize<T: DeserializeOwned>(impl AsRef<[u8]>) -> Result<T>` | Deserialize from bytes                 |

These are internal crate functions. Application code interacts with them
indirectly through `Link<P>` send/receive and collection operations.

---

## `Bytes` / `BytesMut`

Re-exported from the `bytes` crate. Used throughout mosaik for zero-copy
byte buffers:

```rust,ignore
use mosaik::Bytes;

let data: Bytes = serialize(&my_message);
```

---

## `BackoffFactory`

A factory type for creating retry backoff strategies:

```rust,ignore
type BackoffFactory = Arc<
    dyn Fn() -> Box<dyn Backoff + Send + Sync + 'static>
        + Send + Sync + 'static
>;
```

Used in `streams::Config` to configure consumer reconnection. The `backoff`
crate is re-exported at `mosaik::streams::backoff`.

---

## Formatting utilities

Internal helpers for consistent debug output:

| Type                        | Description                                          |
| --------------------------- | ---------------------------------------------------- |
| `Pretty<T>`                 | Pass-through wrapper (for trait integration)         |
| `Short<T>`                  | Display first 5 bytes as hex                         |
| `Abbreviated<const LEN, T>` | Show `first..last` hex if longer than LEN bytes      |
| `Redacted<T>`               | Always prints `<redacted>`                           |
| `FmtIter<W, I>`             | Format iterator elements comma-separated in brackets |

---

## `IntoIterOrSingle<T>`

An ergonomic trait that lets API methods accept either a single item or an
iterator:

```rust,ignore
// Both work:
discovery.with_tags("validator");            // single tag
discovery.with_tags(["validator", "relay"]); // multiple tags
```

This is implemented via two blanket impls using `Variant<0>` (single item
via `Into<T>`) and `Variant<1>` (iterator of `Into<T>`).

---

## Internal async primitives

These are `pub(crate)` and not part of the public API, but understanding them
helps when reading mosaik's source code.

### `UnboundedChannel<T>`

A wrapper around `tokio::sync::mpsc::unbounded_channel` that keeps both the
sender and receiver in one struct:

```rust,ignore
let channel = UnboundedChannel::new();
channel.send(42);
let val = channel.recv().await;
```

| Method                           | Description                     |
| -------------------------------- | ------------------------------- |
| `sender()`                       | Get `&UnboundedSender<T>`       |
| `receiver()`                     | Get `&mut UnboundedReceiver<T>` |
| `send(T)`                        | Send (errors silently ignored)  |
| `recv()`                         | Async receive                   |
| `poll_recv(cx)`                  | Poll-based receive              |
| `poll_recv_many(cx, buf, limit)` | Batch poll receive              |
| `is_empty()` / `len()`           | Queue inspection                |

### `AsyncWorkQueue<T>`

A `FuturesUnordered` wrapper with a permanently-pending sentinel future so
that polling never returns `None`:

```rust,ignore
let queue = AsyncWorkQueue::new();
queue.enqueue(async { do_work().await });

// Poll via Stream trait — never completes while empty
while let Some(result) = queue.next().await {
    handle(result);
}
```

### `BoxPinFut<T>`

Type alias for boxed, pinned, send futures:

```rust,ignore
type BoxPinFut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
```

The `InternalFutureExt` trait adds a `.pin()` method to any
`Future + Send + 'static` for ergonomic boxing.
