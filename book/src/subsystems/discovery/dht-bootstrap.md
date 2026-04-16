# DHT Bootstrap

Mosaik includes an automatic peer discovery mechanism based on the [Mainline DHT](https://en.wikipedia.org/wiki/Mainline_DHT) (the same DHT used by BitTorrent). Nodes sharing the same `NetworkId` automatically discover each other — no hardcoded bootstrap peers are needed.

## How It Works

Rather than storing all peers in a single shared record, Mosaik uses a **chain of 16 DHT slots**. Each slot holds the address of exactly one peer and is identified by a deterministic key derived from the `NetworkId`. Slot 0 is keyed directly from the network ID, and each subsequent slot is keyed by hashing the previous slot's key with blake3, forming a linked list any node can independently compute.

On each poll cycle, all 16 slots are resolved in parallel. For every slot:

- If it contains a **healthy peer**, that peer's address is fed into the discovery system.
- If it contains an **unhealthy peer** (ping timeout), the slot becomes a candidate for replacement.
- If it is **empty**, it is also a candidate for claiming.

If the local node is not already present in any slot, it picks a random candidate and publishes its own address there. Randomizing the slot selection spreads concurrent publishers across different DHT keys, reducing write contention when many nodes start simultaneously.

### Record Format

Each DHT slot is a [pkarr](https://pkarr.org) signed packet containing:

- A `_id` TXT record — the peer's ID, base58-encoded
- One or more `_ip` TXT records — the peer's direct socket addresses (`IP:port`)
- One or more `_r` TXT records — the peer's relay URLs

Records have a TTL of **10 minutes**. After publishing, the node will not attempt to republish until the TTL has elapsed, preventing it from claiming additional slots if its own entry isn't yet visible due to DHT propagation latency.

### Stale Address Refresh

If the local node's own slot is found but contains outdated addresses (e.g., after a network change), the node targets that exact slot for an immediate republish rather than claiming a new one.

### Adaptive Polling

Polling uses an adaptive interval:

- **Aggressive polling (5 seconds)** — when the node has no known peers yet, it polls frequently to bootstrap quickly.
- **Relaxed polling (60 seconds)** — once the node has discovered at least one peer, polling slows down to reduce DHT load. Automatically switches back to aggressive if all peers disconnect.

Both intervals include random jitter (up to 1/3 of the base duration) to desynchronize concurrent pollers.

### Compare-and-Swap

Publishes use **compare-and-swap (CAS)** to safely update a slot without overwriting a concurrent change. On a CAS conflict, the node retries immediately against the new state of the chain.

## Configuration

DHT bootstrap is **enabled by default**. To disable it (e.g., when using only explicit bootstrap peers):

```rust,ignore
let network = Network::builder(network_id)
    .with_discovery(
        discovery::Config::builder()
            .no_dht_bootstrap()
            .with_bootstrap(bootstrap_addr)
    )
    .build()
    .await?;
```

### Configuration Options

| Option              | Default | Description                                                     |
| ------------------- | ------- | --------------------------------------------------------------- |
| `no_dht_bootstrap`  | `false` | Disables the DHT bootstrap mechanism entirely.                  |
| `bootstrap_peers`   | `[]`    | Explicit peers to connect to on startup (via `with_bootstrap`). |

Poll and publish intervals are not currently configurable and use the hardcoded values above.

## When to Use

DHT bootstrap is ideal for:

- **Decentralized deployments** where there is no fixed infrastructure to serve as bootstrap nodes
- **Dynamic environments** where nodes come and go frequently
- **Zero-configuration setups** where nodes only need to agree on a network name

For networks with stable infrastructure, you can combine DHT bootstrap with explicit bootstrap peers — nodes will use whichever method finds peers first.
