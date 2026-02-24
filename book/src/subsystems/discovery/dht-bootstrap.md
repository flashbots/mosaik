# DHT Bootstrap

Mosaik includes an automatic peer discovery mechanism based on the [Mainline DHT](https://en.wikipedia.org/wiki/Mainline_DHT) (the same DHT used by BitTorrent). Nodes sharing the same `NetworkId` automatically discover each other — no hardcoded bootstrap peers are needed.

## How It Works

Each node derives a DHT key from its `NetworkId` and uses it to both **publish** its own address and **poll** for other nodes' addresses. The general flow is:

1. **Publish** — The node resolves the current DHT record for the network key, appends its own address, and publishes the updated record back to the DHT.
2. **Poll** — The node periodically reads the DHT record and dials any peers it finds.

Because both publish and poll use the same deterministic key derived from the `NetworkId`, all nodes in the same network naturally converge on the same record.

### Record Structure

Each DHT record is a [pkarr](https://pkarr.org) signed packet containing DNS resource records. Every peer in the record has:

- An **A record** with a unique subdomain to identify the peer
- A **TXT record** with a `peers=N` field indicating how many peers that node currently knows about

### Capacity Management

Mainline DHT records are limited to 1000 bytes. To stay within this limit, a maximum of **12 peers** are stored per record. When the record is full and a new node needs to publish, the **least-connected peer** (the one with the lowest `peers=N` count) is evicted. Ties are broken randomly to avoid deterministic starvation.

Evicted peers are not forgotten — they are dialed directly so they can still join the network through normal gossip-based discovery.

### Adaptive Polling

Polling uses an adaptive interval:

- **Aggressive polling (5 seconds)** — when the node has no known peers yet, it polls frequently to bootstrap quickly.
- **Relaxed polling (configurable, default 60 seconds)** — once the node has discovered at least one peer, polling slows down to reduce DHT load.

### Publish Cycle

Publishing runs on a slower interval (default **5 minutes**) and uses **compare-and-swap (CAS)** to safely update the shared record without overwriting concurrent changes from other nodes. A random startup jitter prevents all nodes from publishing simultaneously.

## Configuration

DHT bootstrap is **enabled by default**. You can tune the intervals or disable it entirely:

```rust,ignore
use mosaik::{Network, discovery};
use std::time::Duration;

let network = Network::builder(network_id)
    .with_discovery(
        discovery::Config::builder()
            // Customize DHT intervals
            .with_dht_publish_interval(Some(Duration::from_secs(300)))
            .with_dht_poll_interval(Some(Duration::from_secs(60)))
    )
    .build()
    .await?;
```

To disable automatic bootstrap (e.g., when using only explicit bootstrap peers):

```rust,ignore
let network = Network::builder(network_id)
    .with_discovery(
        discovery::Config::builder()
            .no_auto_bootstrap()
            .with_bootstrap(bootstrap_addr)  // use explicit peers instead
    )
    .build()
    .await?;
```

### Configuration Options

| Field                  | Default | Description                                                                                                                 |
| ---------------------- | ------- | --------------------------------------------------------------------------------------------------------------------------- |
| `dht_publish_interval` | 300s    | How often to publish this node's address to the DHT. `None` disables publishing.                                            |
| `dht_poll_interval`    | 60s     | How often to poll the DHT for new peers. `None` disables polling. Actual interval is adaptive (5s when no peers are known). |

## When to Use

DHT bootstrap is ideal for:

- **Decentralized deployments** where there is no fixed infrastructure to serve as bootstrap nodes
- **Dynamic environments** where nodes come and go frequently
- **Zero-configuration setups** where nodes only need to agree on a network name

For networks with stable infrastructure, you can combine DHT bootstrap with explicit bootstrap peers — nodes will use whichever method finds peers first.
