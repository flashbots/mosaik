# Groups

Availability Groups are clusters of trusted nodes on the same mosaik network that coordinate with each other for load balancing and failover. Members of a group share a secret key, maintain a consistent replicated state through a modified Raft consensus protocol, and stay connected via an all-to-all mesh of persistent **bonds**.

## Overview

```
          ┌──────────┐
          │  Node A   │
          │ (Leader)  │
          └────┬──┬───┘
          bond/  \bond
              /    \
 ┌──────────┐      ┌──────────┐
 │  Node B   │─bond─│  Node C   │
 │(Follower) │      │(Follower) │
 └───────────┘      └───────────┘
```

Every pair of group members maintains a persistent **bond** — an authenticated, bidirectional QUIC connection. Bonds carry Raft consensus messages, heartbeats, and log-sync traffic.

## Trust Model

Groups are **not** Byzantine fault tolerant. All members within a group are assumed to be honest and operated by the same entity. The `GroupKey` acts as the sole admission control — only nodes that know the key can join.

## Quick Start

```rust,ignore
use mosaik::groups::GroupKey;

// All group members must use the same key
let key = GroupKey::generate();

// Join with default settings (NoOp state machine)
let group = network.groups().with_key(key).join();

// Wait for leader election
let leader = group.when().leader_elected().await;
println!("Leader: {leader}");

// Check local role
if group.is_leader() {
    println!("I am the leader");
}
```

## Group Identity

A `GroupId` is derived from three components:

1. **Group key** — the shared secret (`GroupKey`)
2. **Consensus configuration** — election timeouts, heartbeat intervals, etc.
3. **State machine signature** — the state machine's `signature()` + state sync `signature()`

Any divergence in these values across nodes produces a different `GroupId`, preventing misconfigured nodes from bonding.

```rust,ignore
// GroupId derivation (internal)
let id = key.secret().hashed()
    .derive(consensus.digest())
    .derive(state_machine.signature())
    .derive(state_machine.state_sync().signature());
```

## Key Types

| Type              | Description                                                      |
| ----------------- | ---------------------------------------------------------------- |
| `Groups`          | Public API gateway — one per `Network`                           |
| `GroupBuilder`    | Typestate builder for configuring and joining a group            |
| `Group<M>`        | Handle for interacting with a joined group                       |
| `GroupKey`        | Shared secret for admission control                              |
| `GroupId`         | Unique identifier (`Digest`) derived from key + config + machine |
| `Bond` / `Bonds`  | Persistent connections between group members                     |
| `When`            | Reactive status API for group state changes                      |
| `ConsensusConfig` | Raft timing parameters                                           |

## ALPN Protocol

Groups use `/mosaik/groups/1` as their ALPN identifier.

## Close Reason Codes

| Code     | Name               | Meaning                                   |
| -------- | ------------------ | ----------------------------------------- |
| `30_400` | `InvalidHandshake` | Error during handshake decoding           |
| `30_404` | `GroupNotFound`    | Group ID not known to acceptor            |
| `30_405` | `InvalidProof`     | Authentication proof invalid              |
| `30_408` | `Timeout`          | Timed out waiting for response            |
| `30_429` | `AlreadyBonded`    | A bond already exists between these peers |

## Subsystem Configuration

```rust,ignore
use mosaik::groups::Config;

let config = Config::builder()
    .with_handshake_timeout(Duration::from_secs(2))  // default
    .build()?;
```

| Option              | Default   | Description                           |
| ------------------- | --------- | ------------------------------------- |
| `handshake_timeout` | 2 seconds | Timeout for bond handshake completion |
