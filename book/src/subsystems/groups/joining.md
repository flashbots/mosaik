# Joining Groups

The `GroupBuilder` uses a **typestate pattern** to ensure groups are configured correctly at compile time. You cannot call `join()` until both storage and state machine are set (or you use the shorthand `join()` which defaults to `NoOp`).

## Builder Flow

```
groups.with_key(key)
  ├── .join()                          // NoOp machine, InMemoryLogStore
  ├── .require_ticket(validator)       // ticket-based peer auth (any stage)
  └── .with_state_machine(machine)
        ├── .join()                    // InMemoryLogStore (default)
        ├── .require_ticket(validator)
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

Leadership preference is configured per-node through the `StateMachine` trait,
not through `ConsensusConfig`. Override `leadership_preference()` to control
whether a node actively participates in elections:

```rust,ignore
use mosaik::LeadershipPreference;

fn leadership_preference(&self, _: &PeerEntry) -> LeadershipPreference {
    // Never self-nominate, don't count toward quorum
    LeadershipPreference::Observer

    // Or: longer election timeouts (3x default), still eligible
    // LeadershipPreference::reluctant()
}
```

See [State Machines > Leadership Preference](state-machines.md#leadership-preference)
for details on the three variants (`Normal`, `Reluctant`, `Observer`).

## Ticket-Based Peer Authentication

By default, any peer that knows the `GroupKey` can join a group. To add an
extra layer of verification, call `.require_ticket()` with a
`TicketValidator` implementation. During bonding, each peer's discovery
tickets are checked against the validator -- only peers carrying a valid
ticket of the expected class are allowed to form bonds.

```rust,ignore
use mosaik::tickets::{Jwt, Hs256};

let validator = Jwt::with_key(Hs256::new(secret))
    .allow_issuer("my-app");

let group = network.groups()
    .with_key(GroupKey::from(&validator))
    .with_state_machine(MyStateMachine::new())
    .require_ticket(validator)
    .join();
```

For asymmetric keys (e.g. ECDSA P-256):

```rust,ignore
use mosaik::tickets::{Jwt, Es256};

// Compressed P-256 public key (33 bytes, SEC1 format)
let validator = Jwt::with_key(Es256::hex("02abcd..."))
    .allow_issuer("my-app");

let group = network.groups()
    .with_key(GroupKey::from(&validator))
    .with_state_machine(MyStateMachine::new())
    .require_ticket(validator)
    .join();
```

### Key points

- **Affects `GroupId` derivation.** Each validator's `signature()` is mixed
  into the group ID. All members must use the same validators in the same
  order, or they will derive different group IDs and never bond.
- **Peers must attach tickets via discovery.** Each peer calls
  `network.discovery().add_ticket(ticket)` with a `Ticket` whose `class`
  matches the validator's `class()`. See
  [Auth Tickets](../discovery/tickets.md) for details.
- **Revocation is automatic.** If a bonded peer removes its ticket (or the
  ticket expires and fails re-validation), the bond is terminated.
- **Expiration-aware bonds.** When `validate` returns
  `Ok(Expiration::At(time))`, the bond worker schedules a timer. When the
  ticket expires, the peer's ticket is re-validated automatically. If
  re-validation fails, the bond is terminated with `NotAllowed`. Peers can
  refresh their ticket by publishing a new one via discovery — the bond
  worker picks up the updated expiration on the next peer entry update.
- **`GroupKey` can be derived from the validator.** Instead of manually
  generating a key, you can use `GroupKey::from(&validator)` to derive a
  deterministic key from the validator's `signature()`.
- **Accepts `Box<dyn TicketValidator>` and `Arc<dyn TicketValidator>`** in
  addition to concrete types.

## Idempotent Joins

If you `join()` a group whose `GroupId` already exists on this node, the existing `Group` handle is returned. No duplicate worker is spawned.

## Lifecycle

When a `Group<M>` handle is dropped:
1. Bonds notify peers of the departure
2. The group's cancellation token is triggered
3. The group is removed from the active groups map
