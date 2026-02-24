# Network

The `Network` is the primary entry point to the mosaik SDK. It creates the QUIC transport layer, establishes node identity, and composes all subsystems (Discovery, Streams, Groups) into a single cohesive runtime.

## Creating a Network

### Quick Start

For prototyping, `Network::new()` creates a node with default settings:

```rust,ignore
use mosaik::{Network, NetworkId};

let network = Network::new("my-app".into()).await?;
```

This creates a node with:
- A random secret key (new identity each run)
- Default relay mode (iroh's relay servers for NAT traversal)
- No bootstrap peers
- No tags
- Default configs for all subsystems

### Builder Pattern

For production use, the builder provides full control:

```rust,ignore
use mosaik::{Network, NetworkId, discovery, streams, groups};
use iroh::SecretKey;

let network = Network::builder("my-app".into())
    .with_secret_key(my_secret_key)
    .with_relay_mode(iroh::RelayMode::Disabled)
    .with_mdns_discovery(true)
    .with_addresses(bind_addrs)
    .with_discovery(
        discovery::Config::builder()
            .with_bootstrap(bootstrap_addr)
            .with_tags("my-role")
            .with_purge_after(Duration::from_secs(600))
    )
    .with_streams(
        streams::Config::builder()
    )
    .with_groups(
        groups::Config::builder()
    )
    .build()
    .await?;
```

### Builder Options

| Method                  | Type                   | Default   | Description                         |
| ----------------------- | ---------------------- | --------- | ----------------------------------- |
| `with_secret_key()`     | `SecretKey`            | Random    | Node identity (determines `PeerId`) |
| `with_relay_mode()`     | `RelayMode`            | `Default` | NAT traversal via relay servers     |
| `with_mdns_discovery()` | `bool`                 | `false`   | Local network mDNS peer discovery   |
| `with_addresses()`      | `BTreeSet<SocketAddr>` | Empty     | Explicit bind addresses             |
| `with_discovery()`      | `ConfigBuilder`        | Defaults  | Discovery subsystem configuration   |
| `with_streams()`        | `ConfigBuilder`        | Defaults  | Streams subsystem configuration     |
| `with_groups()`         | `ConfigBuilder`        | Defaults  | Groups subsystem configuration      |

## Accessing Subsystems

Once built, the `Network` provides access to all subsystems:

```rust,ignore
// Transport & identity
let local = network.local();
let peer_id = network.local().id();
let addr = network.local().addr();
let network_id = network.network_id();

// Subsystems
let discovery = network.discovery();
let streams = network.streams();
let groups = network.groups();
```

All handles are cheap to clone (`Arc<Inner>` internally).

## LocalNode

`LocalNode` represents the local node's transport and identity:

```rust,ignore
let local = network.local();

// Identity
let peer_id = local.id();          // Public key
let secret = local.secret_key();   // Secret key
let network_id = local.network_id();

// Address (for sharing with others)
let addr = local.addr();  // EndpointAddr: public key + relay URL + addrs

// Readiness
local.online().await;  // Blocks until the endpoint is ready

// Low-level access to iroh endpoint
let endpoint = local.endpoint();
```

## Waiting for Readiness

After building, you can wait for the node to be fully online:

```rust,ignore
network.online().await;
```

This resolves once the iroh endpoint is ready and all subsystem handlers are installed. The `build()` method already waits for this, so `online()` is mainly useful when you have a cloned network handle.

## Lifecycle & Shutdown

When a `Network` is dropped, it cancels the internal `CancellationToken`, which propagates shutdown to all subsystems:

```rust,ignore
{
    let network = Network::new(network_id).await?;
    // Node is running...
} // Network dropped here → all tasks cancelled
```

For long-running services, keep the network handle alive:

```rust,ignore
let network = Network::new(network_id).await?;
// ... set up streams, groups, etc.
core::future::pending::<()>().await;  // Block forever
```

## Link: The Wire Protocol

Under the hood, all communication happens through `Link<P>` — a typed, framed, bidirectional QUIC stream:

```rust,ignore
pub struct Link<P: Protocol> { /* ... */ }
```

Links provide:
- **Length-delimited framing** via `LengthDelimitedCodec`
- **Serialization** via `postcard` (compact binary format)
- **Type safety** via the `Protocol` trait and generic parameter

```rust,ignore
// Open a link to a remote peer
let link = Link::<MyProtocol>::open(local, remote_addr).await?;

// Send and receive typed messages
link.send(MyMessage { /* ... */ }).await?;
let response: MyResponse = link.recv().await?;

// Split into independent halves
let (sender, receiver) = link.split();

// Close with a typed reason
link.close(MyCloseReason::Success).await?;
```

Most users won't use `Link` directly — it's the building block that Streams, Groups, and Discovery use internally.

## Error Handling

Network construction can fail with:

| Error                  | Cause                                   |
| ---------------------- | --------------------------------------- |
| `MissingNetworkId`     | Network ID not provided                 |
| `Bind(BindError)`      | Failed to bind the QUIC endpoint        |
| `InvalidAddress`       | Invalid socket address in configuration |
| `DiscoveryConfig(...)` | Invalid discovery configuration         |
| `StreamsConfig(...)`   | Invalid streams configuration           |
| `GroupsConfig(...)`    | Invalid groups configuration            |

Connection-level errors use typed close reasons:

| Code | Name                | Meaning                        |
| ---- | ------------------- | ------------------------------ |
| 200  | `Success`           | Protocol completed normally    |
| 204  | `GracefulShutdown`  | Clean shutdown                 |
| 100  | `InvalidAlpn`       | Wrong protocol identifier      |
| 101  | `DifferentNetwork`  | Peer on a different network    |
| 102  | `Cancelled`         | Operation cancelled            |
| 400  | `ProtocolViolation` | Message deserialization failed |
| 401  | `UnknownPeer`       | Peer not in discovery catalog  |
