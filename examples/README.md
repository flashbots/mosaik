# Examples

Runnable examples demonstrating mosaik's core primitives.

## Bootstrap Node

**[`bootstrap.rs`](bootstrap.rs)** — a ready-to-use bootstrap node for any mosaik network.

A bootstrap node is the initial point of contact that allows other peers to join and discover each other on the network. It runs as a long-lived process and can be deployed as-is without modification.

Features demonstrated:
- Creating a `Network` with a stable identity using a secret key
- Configuring peer discovery with tags and bootstrap peers

### Running

```bash
cargo run --example bootstrap -- --network-id=mynet --secret=mysecret
```

Key options:

| Flag                 | Description                                             |
| -------------------- | ------------------------------------------------------- |
| `--secret`, `-s`     | Secret key (hex) or seed string for a stable peer ID    |
| `--network-id`, `-n` | Network ID (hex) or seed string                         |
| `--peers`, `-p`      | Other bootstrap peer IDs to connect to on startup       |
| `--tags`, `-t`       | Discovery tags (default: `bootstrap`)                   |
| `--no-relay`         | Disable relay servers (node must be directly reachable) |
| `-v` / `-vv`         | Verbose / very verbose logging                          |
| `--quiet`, `-q`      | Suppress all output                                     |

All flags can also be set via `MOSAIK_BOOTSTRAP_*` environment variables.

---

## Orderbook

**[`orderbook/`](orderbook/)** — a distributed order-matching engine built on mosaik.

A more complete example that combines multiple mosaik primitives into a working application: a price-time priority orderbook replicated across a Raft consensus group, with typed streams for order submission and fill dissemination.

Features demonstrated:
- **Streams** — typed pub/sub channels for disseminating `Order` and `Fill` events between nodes
- **Groups** — a Raft consensus group running an `OrderBook` state machine that replicates order matching across three nodes
- **State machines** — implementing the `StateMachine` trait for deterministic command application and queries
- **Discovery** — cross-discovering multiple nodes on the same network

### Topology

```
Traders (stream producers) → OrderBook Group (Raft RSM) → Fill stream (consumers)
```

1. **Trader nodes** publish `Order` events onto a typed stream
2. **Matcher nodes** (3-node Raft group) consume orders, match them through consensus, and produce `Fill` events
3. **Downstream consumers** receive fill notifications via a separate typed stream

### Running

```bash
cargo run -p orderbook
```

The example spins up 5 in-process nodes (3 matchers + 2 traders), submits a handful of orders on the ETH/USDC pair, matches them through the replicated orderbook, and prints the resulting fills and top-of-book state.