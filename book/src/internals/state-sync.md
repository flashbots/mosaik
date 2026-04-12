# State Synchronization

When a new peer joins a group or falls too far behind the leader's log, it needs to catch up to the current committed state before it can participate as a voting member. Mosaik provides a pluggable state synchronization framework through the `StateSync` trait, with two built-in implementations.

## Overview

State sync is triggered when a follower receives an `AppendEntries` message whose preceding log position does not match the follower's local log — indicating a gap. At that point the group creates a **sync session** on the lagging follower and the session drives the catch-up process to completion.

```text
Follower detects gap
        │
        ▼
 ┌─────────────────┐
 │  StateSync       │──creates──► Session (on the lagging follower)
 │  implementation  │──creates──► Provider (on every peer, at group init)
 └─────────────────┘
        │
        ▼
 Session exchanges messages with Providers
 on remote peers until fully caught up
        │
        ▼
 Follower resumes normal operation
```

During catch-up, any new `AppendEntries` from the leader are **buffered** by the session and replayed once the gap is closed.

## The `StateSync` trait

Every state sync implementation provides four things:

1. **`signature()`** — a unique identifier for the implementation and its configuration. This is part of the `GroupId` derivation, so all group members must use the same implementation with the same settings.

2. **`Provider`** — a long-lived object created once per peer at group initialization. Providers serve state to remote sessions (e.g. by responding to fetch requests). They run on **all** peers, including the leader.

3. **`Session`** — a short-lived object created on the lagging follower when a gap is detected. The session drives the catch-up process by exchanging messages with remote providers, and terminates once the follower is fully synchronized.

4. **`Message`** — the wire-level protocol messages exchanged between sessions and providers.

Both the provider and session are polled by the group's internal work scheduler and can send messages to any bonded peer via the `SyncContext`.

## `LogReplaySync` — log entry replay

The built-in `LogReplaySync` is the simplest sync strategy. It recovers missing log entries by fetching them directly from peers that still have them in their log storage.

### How it works

```text
1. Lagging follower            2. Peers respond with
   broadcasts                     available ranges
   AvailabilityRequest            AvailabilityResponse(range)
   to all bonded peers

3. Follower partitions         4. Follower applies
   the needed range               fetched entries to
   across responding peers         the state machine
   and fetches in parallel         and resumes
   (FetchEntriesRequest)
```

1. The lagging follower broadcasts an `AvailabilityRequest` to all bonded peers.
2. Each peer responds with the range of log entries it has available.
3. The follower partitions the missing range across responding peers for balanced load, sends `FetchEntriesRequest` messages, and receives `FetchEntriesResponse` batches in parallel. At most one request is in-flight per peer.
4. Once all entries are fetched, they are applied to the state machine in order, buffered entries are replayed, and the follower resumes normal operation.

### Characteristics

- **Works with any `StateMachine`** — no additional traits required.
- **No snapshots** — relies entirely on the raw log being available.
- **Leader deprioritized** — the follower prefers syncing from other followers to avoid overloading the leader.
- **Does not support log pruning** — all entries must remain in storage for the lifetime of the group. For high-throughput groups, consider on-disk log storage.

| Parameter       | Default | Description                                  |
| --------------- | ------- | -------------------------------------------- |
| `batch_size`    | 2000    | Maximum entries per fetch request            |
| `fetch_timeout` | 25s     | Timeout for each fetch operation             |

### When to use

`LogReplaySync` is a good starting point for custom state machines. It is best suited for:

- Groups with low to moderate command throughput
- State machines where log replay is fast
- Development and testing

## `SnapshotSync` — snapshot transfer (collections)

The built-in collections (`Map`, `Vec`, `Set`, `Cell`, `Once`, `PriorityQueue`) use `SnapshotSync`, a more sophisticated strategy that transfers a point-in-time snapshot of the state machine instead of replaying log entries. This is more efficient for large state because the snapshot size is proportional to the current state, not the full history.

### Why not leader-only snapshots?

In standard Raft, the leader ships snapshots to lagging followers. This has two problems:

1. **Leader bottleneck** — the leader must serialize and transmit potentially large state to each lagging follower.
2. **Snapshot inconsistency** — taking a snapshot at a consistent point is tricky when the leader is also processing new commands.

Mosaik solves both by having **all peers** participate in snapshot creation and distribution.

### How it works

```text
1. Follower              2. Leader wraps        3. All peers create
   detects lag              request as a           snapshot at the
   ─────────────            log entry and          committed position
   sends                    replicates it          of that entry
   RequestSnapshot       ─────────────────      ─────────────────────

4. Follower              5. Follower fetches    6. Follower installs
   discovers which          batches from           snapshot and
   peers have               multiple peers         resumes normal
   snapshots ready          in parallel            operation
─────────────────        ─────────────────      ─────────────────────
```

1. **Detect lag.** A follower receives an `AppendEntries` referencing a log prefix it does not have. It sends a `RequestSnapshot` message to the leader.

2. **Replicate the request.** The leader wraps the snapshot request as a special command and replicates it through normal Raft consensus. This ensures all peers see the request at the **same log position**.

3. **Create snapshots.** When each peer commits the snapshot command, it creates a snapshot of its state at that position. Because all peers see the request at the same committed index, **all snapshots are consistent**. The `im` crate's persistent data structures make snapshotting O(1) via structural sharing.

4. **Discover snapshot sources.** The follower receives `SnapshotReady` messages from peers that have a snapshot available. Snapshots have a configurable **TTL** (default 10s) and are discarded after inactivity.

5. **Parallel fetching.** The follower fetches snapshot data in batches from multiple peers simultaneously:

   ```text
   Follower ──FetchDataRequest──► Peer A  (items 0..2000)
              FetchDataRequest──► Peer B  (items 2000..4000)
              FetchDataRequest──► Peer C  (items 4000..6000)
   ```

   Each batch contains up to `fetch_batch_size` items. Distributing fetches across peers balances the load.

6. **Install and resume.** The follower installs the snapshot as its new state, fast-forwards its committed cursor to the snapshot position, replays any buffered entries received during sync, and resumes as a voting member.

### The `SnapshotStateMachine` trait

State machines that support snapshot sync must implement:

```rust,ignore
trait SnapshotStateMachine: StateMachine {
    type Snapshot: Snapshot;

    fn create_snapshot(&self) -> Self::Snapshot;
    fn install_snapshot(&mut self, snapshot: Self::Snapshot);
}
```

- `create_snapshot()` creates a point-in-time snapshot (O(1) for `im`-backed collections).
- `install_snapshot()` replaces the state machine's state with the fetched snapshot data.

The `Snapshot` trait itself is iterable and appendable:

```rust,ignore
trait Snapshot {
    type Item: SnapshotItem;

    fn len(&self) -> u64;
    fn iter_range(&self, range: Range<u64>) -> Option<impl Iterator<Item = Self::Item>>;
    fn append(&mut self, items: impl IntoIterator<Item = Self::Item>);
}
```

All built-in collection types implement these traits. For example, `Map` yields key-value pairs, `Vec` yields indexed entries, and `PriorityQueue` preserves both its `by_key` and `by_priority` indices across sync.

| Parameter                  | Default | Description                                  |
| -------------------------- | ------- | -------------------------------------------- |
| `fetch_batch_size`         | 2000    | Maximum items per batch request              |
| `snapshot_ttl`             | 10s     | How long a snapshot remains available        |
| `snapshot_request_timeout` | 15s     | Timeout for the initial snapshot request     |
| `fetch_timeout`            | 5s      | Timeout for each batch fetch operation       |

### Tuning tips

- **Large state**: Increase `fetch_batch_size` to reduce round trips, but watch memory usage.
- **Slow networks**: Increase `fetch_timeout` and `snapshot_request_timeout`.
- **Fast churn**: Increase `snapshot_ttl` if snapshots expire before followers can fetch them.

## Writing a custom `StateSync`

For domain-specific needs, you can implement the `StateSync` trait directly. The framework gives you:

- A `SyncContext` with access to the state machine, log storage, committed position, and a message-passing API to any bonded peer.
- Full control over the wire protocol via your own `Message` type.
- A `Provider` that runs on all peers for the group's lifetime.
- A `Session` that runs only on the lagging follower for the duration of catch-up.

This flexibility allows strategies like delta sync, merkle-tree-based reconciliation, or any other approach suited to your state machine's structure.
