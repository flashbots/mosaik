# Group Status

The `Group<M>` handle exposes a rich reactive API for monitoring group state: leadership, log progress, and online status.

## Inspecting Current State

```rust,ignore
// Current leader (if any)
let leader: Option<PeerId> = group.leader();

// Am I the leader?
let is_leader: bool = group.is_leader();

// Group identity
let group_id: &GroupId = group.id();

// Current committed index
let committed: Index = group.committed();

// Current log position (may include uncommitted entries)
let cursor: Cursor = group.log_position();

// Active bonds (peer connections)
let bonds: Bonds = group.bonds();

// Group configuration
let config: &GroupConfig = group.config();
```

## The `When` API

`group.when()` returns a `&When` handle for awaiting state transitions.

### Leadership Events

```rust,ignore
// Wait for any leader to be elected
let leader: PeerId = group.when().leader_elected().await;

// Wait for a different leader (leader change)
let new_leader: PeerId = group.when().leader_changed().await;

// Wait until a specific peer becomes leader
group.when().leader_is(expected_peer_id).await;

// Wait until the local node becomes leader
group.when().is_leader().await;

// Wait until the local node becomes a follower
group.when().is_follower().await;
```

### Online / Offline

A node is **online** when:
- It is the leader, or
- It is a fully synced follower (not catching up, not in an election)

```rust,ignore
// Wait until ready to process commands
group.when().online().await;

// Wait until the node goes offline
group.when().offline().await;
```

### Log Position Watchers

The `CursorWatcher` type provides fine-grained observation of log progress:

#### Committed Index

```rust,ignore
let committed = group.when().committed();

// Wait until a specific index is committed
committed.reaches(42).await;

// Wait for any forward progress
let new_pos = committed.advanced().await;

// Wait for any change (including backward, e.g., log truncation)
let new_pos = committed.changed().await;

// Wait for regression (rare — happens during partition healing)
let new_pos = committed.reverted().await;
```

You can also pass an `IndexRange` from `execute_many` / `feed_many`:

```rust,ignore
let range = group.feed_many(commands).await?;
group.when().committed().reaches(range).await;
```

#### Log Position

The same API works for the full log position (including uncommitted entries):

```rust,ignore
let log = group.when().log();

let new_cursor = log.advanced().await;
let new_cursor = log.changed().await;
```

## Index and Cursor Types

```rust,ignore
/// A log index (0-based, where 0 is the sentinel "no entry")
pub struct Index(pub u64);

/// A Raft term number
pub struct Term(pub u64);

/// A (term, index) pair identifying a specific log position
pub struct Cursor { pub term: Term, pub index: Index }
```

Both `Index` and `Term` provide:
- `zero()`, `one()` — constants
- `is_zero()` — check for sentinel
- `prev()`, `next()` — saturating arithmetic
- `distance(other)` — absolute difference

## Bonds

`group.bonds()` returns a `Bonds` handle — a watchable, ordered collection of all active bonds:

```rust,ignore
let bonds = group.bonds();

// Iterate current bonds
for bond in bonds.iter() {
    println!("Bonded to: {}", bond.peer_id());
}
```

Bonds carry Raft messages, heartbeats, and sync traffic between group members. See the [Bonds deep dive](../internals/bonds.md) for internals.

## Putting It Together

A common pattern for group-aware applications:

```rust,ignore
let group = network.groups()
    .with_key(key)
    .with_state_machine(MyMachine::new())
    .join();

// Wait until we can serve traffic
group.when().online().await;

if group.is_leader() {
    // Run leader-specific tasks
    run_leader_loop(&group).await;
} else {
    // Run follower-specific tasks
    run_follower_loop(&group).await;
}

// React to leadership changes
loop {
    let new_leader = group.when().leader_changed().await;
    if group.is_leader() {
        // Transition to leader role
    } else {
        // Transition to follower role
    }
}
```
