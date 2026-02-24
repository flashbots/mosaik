# Joining Groups

The `GroupBuilder` uses a **typestate pattern** to ensure groups are configured correctly at compile time. You cannot call `join()` until both storage and state machine are set (or you use the shorthand `join()` which defaults to `NoOp`).

## Builder Flow

```
groups.with_key(key)
  ├── .join()                          // NoOp machine, InMemoryLogStore
  └── .with_state_machine(machine)
        ├── .join()                    // InMemoryLogStore (default)
        ├── .with_consensus_config(c)
        │     └── .join()
        └── .with_log_storage(store)
              └── .join()
```

## Minimal Join (NoOp)

Useful for leader election without application logic:

```rust,ignore
let group = network.groups().with_key(key).join();
```

This creates a group with:
- `NoOp` state machine (commands are `()`, queries are `()`)
- `InMemoryLogStore` for storage
- Default `ConsensusConfig`

## With Custom State Machine

```rust,ignore
let group = network.groups()
    .with_key(key)
    .with_state_machine(MyStateMachine::new())
    .join();
```

The state machine must be set **before** storage, since the storage type depends on the command type.

## With Custom Storage

```rust,ignore
let group = network.groups()
    .with_key(key)
    .with_state_machine(MyStateMachine::new())
    .with_log_storage(MyDurableStore::new())
    .join();
```

Storage must implement `Storage<M::Command>`.

## GroupKey

A `GroupKey` is the shared secret that all members must possess:

```rust,ignore
use mosaik::groups::GroupKey;

// Generate a new random key
let key = GroupKey::generate();

// All members use the same key
// The key can be serialized and distributed securely
```

## ConsensusConfig

All consensus parameters are part of `GroupId` derivation — every member must use the same values.

```rust,ignore
use mosaik::groups::ConsensusConfig;

let config = ConsensusConfig::builder()
    .with_heartbeat_interval(Duration::from_millis(500))     // default
    .with_heartbeat_jitter(Duration::from_millis(150))       // default
    .with_max_missed_heartbeats(10)                          // default
    .with_election_timeout(Duration::from_secs(2))           // default
    .with_election_timeout_jitter(Duration::from_millis(500))// default
    .with_bootstrap_delay(Duration::from_secs(3))            // default
    .with_forward_timeout(Duration::from_secs(2))            // default
    .with_query_timeout(Duration::from_secs(2))              // default
    .build()?;

let group = network.groups()
    .with_key(key)
    .with_state_machine(machine)
    .with_consensus_config(config)
    .join();
```

### Parameters

| Parameter                 | Default | Description                                             |
| ------------------------- | ------- | ------------------------------------------------------- |
| `heartbeat_interval`      | 500ms   | Interval between bond heartbeat pings                   |
| `heartbeat_jitter`        | 150ms   | Max random jitter subtracted from heartbeat interval    |
| `max_missed_heartbeats`   | 10      | Missed heartbeats before bond is considered dead        |
| `election_timeout`        | 2s      | Base timeout before a follower starts an election       |
| `election_timeout_jitter` | 500ms   | Max random jitter added to election timeout             |
| `bootstrap_delay`         | 3s      | Wait time before first election to allow peer discovery |
| `forward_timeout`         | 2s      | Timeout for forwarding commands to the leader           |
| `query_timeout`           | 2s      | Timeout for strong-consistency query responses          |

### Leadership Preference

Nodes can deprioritize leadership to prefer being followers:

```rust,ignore
// 3x longer election timeout (default multiplier)
let config = ConsensusConfig::default().deprioritize_leadership();

// Custom multiplier
let config = ConsensusConfig::default().deprioritize_leadership_by(5);
```

This multiplies both `election_timeout` and `bootstrap_delay`, reducing the chance of becoming leader.

## Idempotent Joins

If you `join()` a group whose `GroupId` already exists on this node, the existing `Group` handle is returned. No duplicate worker is spawned.

## Lifecycle

When a `Group<M>` handle is dropped:
1. Bonds notify peers of the departure
2. The group's cancellation token is triggered
3. The group is removed from the active groups map
