# Wire Protocol

All peer-to-peer communication in mosaik flows over **QUIC** streams, using a
consistent framing layer built on `Link<P>`. This chapter explains the protocol
stack from the transport up to application messages.

## Protocol stack

```text
┌─────────────────────────────────┐
│  Application Messages           │  BondMessage, Datum, CatalogSync, ...
├─────────────────────────────────┤
│  postcard serialization         │  compact binary encoding (no_std, varint)
├─────────────────────────────────┤
│  LengthDelimitedCodec framing   │  4-byte big-endian length prefix
├─────────────────────────────────┤
│  QUIC bidirectional stream      │  SendStream + RecvStream
├─────────────────────────────────┤
│  iroh / quinn transport         │  QUIC with TLS 1.3, hole-punching
└─────────────────────────────────┘
```

## `Link<P>`

The `Link<P>` type is the core abstraction for typed, bidirectional
communication over a QUIC stream:

```rust,ignore
struct Link<P: Protocol> {
    connection: Connection,
    cancel: CancellationToken,
    writer: FramedWrite<SendStream, LengthDelimitedCodec>,
    reader: FramedRead<RecvStream, LengthDelimitedCodec>,
}
```

### The `Protocol` trait

Every protocol declares its **ALPN** (Application-Layer Protocol Negotiation)
identifier:

```rust,ignore
trait Protocol: Serialize + DeserializeOwned + Send + 'static {
    const ALPN: &'static [u8];
}
```

ALPN identifiers are exchanged during the TLS handshake, so peers agree on the
protocol before any application data flows. Mosaik uses these ALPNs:

| ALPN                   | Protocol type     | Subsystem           |
| ---------------------- | ----------------- | ------------------- |
| `/mosaik/announce`     | `AnnounceMessage` | Discovery (gossip)  |
| `/mosaik/catalog-sync` | `CatalogSync`     | Discovery (catalog) |
| `/mosaik/streams/1.0`  | `Datum` impl      | Streams             |
| `/mosaik/groups/1`     | `BondMessage`     | Groups (bonds)      |

### Creating a link

Links are created by either connecting to a peer or accepting an incoming
connection:

```rust,ignore
// Outgoing
let link = Link::<BondMessage>::connect(&endpoint, node_addr).await?;

// Incoming (in a ProtocolHandler)
let link = Link::<BondMessage>::accept(connecting).await?;
```

### Sending and receiving

```rust,ignore
// Send a message (serialized with postcard, length-prefixed)
link.send(BondMessage::Ping).await?;

// Receive a message (read length prefix, deserialize with postcard)
let msg: BondMessage = link.recv().await?;
```

Under the hood:
1. `send()` serializes the message with `postcard::to_allocvec()`.
2. The resulting bytes are written through `FramedWrite` which prepends a
   4-byte big-endian length prefix.
3. `recv()` reads the length prefix from `FramedRead`, reads exactly that
   many bytes, and deserializes with `postcard::from_bytes()`.

### Splitting a link

For concurrent send/receive, a link can be split:

```rust,ignore
let (writer, reader) = link.split();

// In one task:
writer.send(msg).await?;

// In another task:
let msg = reader.recv().await?;

// Rejoin if needed:
let link = Link::join(writer, reader);
```

### Cancellation

Every link carries a `CancellationToken`. When cancelled, both send and
receive operations return immediately. This is used for graceful shutdown:

```rust,ignore
link.cancel(); // signals both sides to stop
```

## Wire format

### postcard encoding

[Postcard](https://docs.rs/postcard) is a `#[no_std]`-compatible binary
serialization format based on **variable-length integers** (varints). It
produces very compact output:

- `u8` → 1 byte
- Small `u32` → 1 byte (varint)
- Enum variant → 1 byte discriminant + payload
- Strings → varint length + UTF-8 bytes
- `Vec<T>` → varint length + elements

This keeps message sizes minimal, which matters for high-frequency heartbeats
and Raft messages.

### Framing

Each message on the wire looks like:

```text
┌──────────────┬───────────────────────────────┐
│ Length (4B)   │ postcard-encoded payload       │
│ big-endian    │                               │
└──────────────┴───────────────────────────────┘
```

The `LengthDelimitedCodec` from the `tokio-util` crate handles this
automatically. It supports messages up to 2³² - 1 bytes (4 GiB), though in
practice mosaik messages are typically under a few kilobytes.

## QUIC transport

Mosaik uses **iroh** (built on **quinn**) for QUIC transport. Key features:

| Feature                  | Benefit                                                         |
| ------------------------ | --------------------------------------------------------------- |
| **TLS 1.3**              | All connections encrypted, session secrets used for bond proofs |
| **Multiplexed streams**  | Multiple logical channels over one connection                   |
| **NAT traversal**        | Built-in hole-punching and relay fallback                       |
| **Connection migration** | Connections survive IP changes                                  |
| **mDNS discovery**       | Automatic peer discovery on local networks                      |

### Bidirectional streams

Each `Link<P>` uses a single QUIC bidirectional stream. This means:
- One stream per bond connection.
- One stream per catalog sync session.
- One stream per producer-consumer pair.

QUIC's multiplexing means these streams don't interfere with each other even
when sharing the same underlying UDP connection.

## Error handling

Link operations return `std::io::Error`. Common failure modes:

| Error                 | Cause                                   | Recovery                        |
| --------------------- | --------------------------------------- | ------------------------------- |
| Connection closed     | Peer shut down or network failure       | Reconnect via discovery         |
| Deserialization error | Protocol version mismatch or corruption | Drop connection                 |
| Timeout               | Peer unresponsive                       | Heartbeat detection → reconnect |
| Cancelled             | Local shutdown                          | Graceful cleanup                |
