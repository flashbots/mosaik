# Introduction

<p align="center">
  <a href="https://github.com/flashbots/mosaik/blob/main/LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <a href="https://crates.io/crates/mosaik"><img alt="Status" src="https://img.shields.io/crates/v/mosaik.svg?color=blue"></a>
  <a href="https://github.com/flashbots/mosaik"><img alt="Rust" src="https://img.shields.io/badge/rust-1.93%2B-blue.svg"></a>
  <a href="https://github.com/flashbots/mosaik"><img alt="Status" src="https://img.shields.io/badge/status-experimental-orange.svg"></a>
</p>

**[Mosaik](https://github.com/flashbots/mosaik)** is a Rust runtime for building **self-organizing, leaderless distributed systems**. It is designed for trusted, permissioned networks — environments where all participating nodes are assumed honest, such as L2 blockchain infrastructure operated by a single organization.

## What Mosaik Does

When you deploy mosaik-based binaries on arbitrary machines, the network **self-organizes**: nodes discover each other via gossip, infer the data-flow topology, elect leaders where needed, and converge to a stable operational state. Each node needs only a **network ID** to participate — a string that identifies which logical network to join. Mosaik automatically discovers peers via [Mainline DHT](https://en.wikipedia.org/wiki/Mainline_DHT), requiring no manual configuration. A bootstrap peer can optionally be provided to speed up initial discovery or when DHT discovery is disabled (the [bootstrap example](https://github.com/flashbots/mosaik/blob/main/examples/bootstrap.rs) can be used as a universal bootstrap node in production).

A secret key is automatically generated on each run, giving the node a unique identity. Specifying a fixed secret key is only recommended for bootstrap nodes that need a stable, well-known peer ID across restarts.

From these minimal inputs, mosaik handles peer discovery, typed pub/sub data streaming, Raft-based consensus groups, and replicated data structures — all automatically.

## Design Philosophy

- **Not Byzantine fault tolerant.** All members are assumed honest. This simplifies the protocol stack and enables higher throughput compared to BFT systems. For stronger integrity guarantees, groups can optionally require hardware attestations (e.g. Intel TDX) to cryptographically prove that every member is running the expected software.
- **Self-organizing.** No central coordinator, no manual topology configuration. Nodes find each other and form the right connections.
- **Built on modern networking.** Uses [iroh](https://github.com/n0-computer/iroh) for QUIC-based peer-to-peer transport with relay support and hole-punching.
- **Composable primitives.** Four subsystems (`Discovery`, `Streams`, `Groups`, `Collections`) accessible from a `Network` instance compose to support a wide range of distributed application patterns.
- **First-class TEE support.** Hardware-attested identity via Intel TDX (with AMD SEV-SNP and ARM CCA planned), enabling secure access control based on cryptographic proof of the software running on each node.

## System Overview

```text
┌──────────────────────────────────────────────────────┐
│                      Network                         │
│  (QUIC endpoint, identity, protocol routing)         │
├──────────┬──────────┬──────────┬──────────┬──────────┤
│ Discovery│ Streams  │  Groups  │Collection│   TEE    │
│  gossip, │  typed   │   Raft   │ Map/Vec/ │  TDX /   │
│  catalog │ pub/sub  │ consensus│ Set/Cell │  SNP /   │
│          │          │  groups  │ Once/PQ  │  CCA     │
└──────────┴──────────┴──────────┴──────────┴──────────┘
```

- **Network** is the entry point. It manages the QUIC transport, node identity, and composes all subsystems.
- **Discovery** uses gossip to maintain a catalog of all peers, their capabilities, and their available streams/groups.
- **Streams** provides typed, async pub/sub channels. Any serializable Rust type can be streamed between nodes.
- **Groups** implements Raft consensus for clusters of nodes that need shared state and leader election.
- **Collections** builds on Groups to offer replicated data structures (`Map`, `Vec`, `Set`, `Cell`, `Once`, `PriorityQueue`) that stay synchronized across nodes.
- **TEE** provides first-class support for Trusted Execution Environments (Intel TDX, with AMD SEV-SNP and ARM CCA planned), enabling hardware-attested identity and access control across the network.

## Who Should Read This Book

This book serves two audiences:

- **Application developers** building distributed systems with mosaik — start with [Getting Started](./getting-started/overview.md) and the [Tutorials](./tutorials/bootstrap.md).
- **Contributors** to mosaik itself — the [Architecture Deep Dives](./internals/raft.md) section covers protocol internals, Raft modifications, and design decisions.

## Quick Example

A minimal mosaik node that joins a network and starts streaming data:

```rust,ignore
use mosaik::*;
use futures::SinkExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let network_id: NetworkId = "my-app".into();
    let network = Network::new(network_id).await?;

    // The node is now online and discoverable
    println!("Node {} is online", network.local().id());

    // Create a typed producer (any serializable type works)
    let producer = network.streams().produce::<String>();

    // Wait for at least one consumer to connect
    producer.when().online().await;

    // Send data
    producer.send("hello, world".to_string()).await?;

    Ok(())
}
```
