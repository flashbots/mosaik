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

---

## Group Chat

**[`group-chat.rs`](group-chat.rs)** — a peer-to-peer group chat room using mosaik replicated collections.

The simplest possible networked application: each running instance is one participant. Participants in the same room automatically discover each other — no server, no configuration.

Features demonstrated:

- **Collections** — `Map<String, UserInfo>` for the live participant registry; `Vec<Message>` for the append-only chat log
- **Intent-addressed stores** — `unique_id!()` derives a stable `StoreId` from a string so every node converges on the same collection without prior coordination
- **`when().online()`** — waits until the node has joined both collection groups and caught up with existing state before showing history or accepting input

### Running

Start two or more instances in separate terminals:

```bash
cargo run --example group-chat -- --nickname Alice
cargo run --example group-chat -- --nickname Bob --color 33
cargo run --example group-chat -- --nickname Charlie --color 34
```

Key options:

| Flag               | Description                                                                  |
| ------------------ | ---------------------------------------------------------------------------- |
| `--nickname`, `-n` | Your display name (default: `anon`)                                          |
| `--color`, `-c`    | ANSI color code for your messages (31–36, default: `32` = green)             |
| `--room`, `-r`     | Room name — anyone with the same name joins the same chat (default: `lobby`) |

---

## TDX (Trusted Execution Environment)

**[`tee/tdx/`](tee/tdx/)** — hardware-attested secure data streaming using Intel TDX.

A complete example demonstrating mosaik's first-class TEE support. A single binary runs as either a producer (outside TEE) or consumer (inside a TDX enclave), selected automatically based on hardware detection. The producer only accepts consumers whose TDX attestation carries the expected measurement.

Features demonstrated:
- **TEE attestation** — generating and installing hardware-signed TDX tickets via `network.tdx().install_own_ticket()`
- **TDX validation** — using `TdxValidator` with `require_mrtd()` to enforce firmware measurements
- **Local measurement matching** — using `require_own_mrtd()` and `require_own_rtmr2()` to require peers to have the same TDX measurements as the local machine
- **`stream!` macro with `require_ticket`** — compile-time stream declaration with baked-in TDX validation
- **`collection!` macro with `require_ticket`** — compile-time collection declaration with TDX validation derived from local measurements
- **Build-time image builder** — `build.rs` uses `mosaik::tdx::build::alpine()` to cross-compile and package a TDX guest image

### Building

```bash
cargo build -p tdx-example --release
```

The build script cross-compiles for musl, downloads Alpine Linux and a TDX kernel, and produces a self-extracting launch script for the TDX guest.
