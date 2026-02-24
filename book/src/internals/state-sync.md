# State Synchronization

When a new peer joins a group or falls too far behind the leader's log, it
needs a **snapshot** of the current state rather than replaying potentially
thousands of log entries. Mosaik implements a distributed state sync mechanism
that spreads the load across all group members.

## Why not leader-only snapshots?

In standard Raft, the leader ships snapshots to lagging followers. This has
two problems:

1. **Leader bottleneck** — the leader must serialize and transmit potentially
   large state to each lagging follower.
2. **Snapshot inconsistency** — the snapshot must be taken at a consistent
   point, which may block the leader.

Mosaik solves both by having **all peers** participate in snapshot creation and
distribution.

## The six-step process

```text
1. Follower             2. Leader              3. All peers
   detects lag              wraps request         create snapshot
   ────────────             as log entry          at committed
   sends                    ────────────          position
   RequestSnapshot          replicates it         ──────────────

4. Follower             5. Follower            6. Follower
   discovers which          fetches batches       installs snapshot
   peers have               from multiple         and resumes
   snapshots                peers in parallel     normal operation
   ──────────────           ─────────────────     ──────────────────
```

### Step 1: Detect lag

A follower realizes it is behind when it receives an `AppendEntries` message
referencing a log prefix it does not have. It sends a `StateSync` message
to the leader requesting a snapshot.

### Step 2: Replicate the request

The leader wraps the snapshot request as a special log entry and replicates
it through the normal Raft consensus path. This ensures all peers see the
request at the **same log position**.

### Step 3: Create snapshots

When each peer commits the snapshot request entry, it creates a point-in-time
snapshot of its state machine. Because all peers see the request at the same
committed index, **all snapshots are consistent** — they represent the same
logical state.

The `im` crate's persistent data structures make snapshotting O(1) by
sharing structure with the live state (copy-on-write).

### Step 4: Discover snapshot sources

The follower queries peers to find out which ones have a snapshot available
for the requested position. Snapshots have a **TTL** (default 10 seconds),
so they eventually expire if not fetched.

### Step 5: Parallel fetching

The follower fetches snapshot data in **batches** from multiple peers
simultaneously:

```text
Follower ──batch request──► Peer A  (items 0..2000)
           batch request──► Peer B  (items 2000..4000)
           batch request──► Peer C  (items 4000..6000)
                   ...
```

Each batch contains up to `fetch_batch_size` (default 2000) items. By
distributing fetches across peers, the load is balanced and the follower
receives data faster.

### Step 6: Install and resume

Once all batches are received, the follower:

1. Installs the snapshot as its new state machine state.
2. Updates its committed cursor to the snapshot position.
3. Replays any log entries received **after** the snapshot position.
4. Resumes normal follower operation (including voting).

## Traits

### `SnapshotStateMachine`

State machines that support sync must implement:

```rust,ignore
trait SnapshotStateMachine: StateMachine {
    type Snapshot: Snapshot;

    fn snapshot(&self) -> Self::Snapshot;
    fn install_snapshot(&mut self, items: Vec<SnapshotItem>);
}
```

- `snapshot()` creates a serializable snapshot of the current state.
- `install_snapshot()` replaces the state with data received from peers.

### `Snapshot`

The snapshot itself is iterable:

```rust,ignore
trait Snapshot {
    fn len(&self) -> usize;
    fn into_items(self) -> impl Iterator<Item = SnapshotItem>;
}
```

Each collection type implements this — for example, `MapSnapshot` yields
key-value pairs as serialized `SnapshotItem` bytes.

### `SnapshotItem`

```rust,ignore
struct SnapshotItem {
    key: Bytes,
    value: Bytes,
}
```

The generic key/value format allows any collection type to participate in the
same sync protocol.

## Configuration

State sync behavior is controlled by `SyncConfig`:

| Parameter                  | Default | Description                                  |
| -------------------------- | ------- | -------------------------------------------- |
| `fetch_batch_size`         | 2000    | Maximum items per batch request              |
| `snapshot_ttl`             | 10s     | How long a snapshot remains available        |
| `snapshot_request_timeout` | 15s     | Timeout for requesting a snapshot from peers |
| `fetch_timeout`            | 5s      | Timeout for each batch fetch operation       |

### Tuning tips

- **Large state**: Increase `fetch_batch_size` to reduce round trips, but
  watch memory usage.
- **Slow networks**: Increase `fetch_timeout` and `snapshot_request_timeout`.
- **Fast churn**: Increase `snapshot_ttl` if snapshots expire before followers
  can fetch them.

## Log replay sync

For groups that use `Storage` (log persistence) instead of in-memory state,
a `LogReplaySync` alternative exists. Instead of shipping snapshots, the
joining peer replays log entries from a **replay provider** — another peer
that streams its stored log entries.

This approach is simpler but only works when:

- The log fits in available storage.
- Replay is fast enough relative to new entries arriving.

The `replay` module provides `ReplayProvider` and `ReplaySession` types for
this path.

## Collections and state sync

All built-in collection types (`Map`, `Vec`, `Set`, `PriorityQueue`) implement
`SnapshotStateMachine`. Their internal state machines produce snapshots using
the `im` crate's persistent data structures:

```text
MapStateMachine ──snapshot()──► MapSnapshot (HashMap entries)
VecStateMachine ──snapshot()──► VecSnapshot (indexed entries)
SetStateMachine ──snapshot()──► SetSnapshot (set members)
DepqStateMachine ──snapshot()──► PriorityQueueSnapshot (by_key + by_priority)
```

The dual-index structure of `PriorityQueue` is preserved across sync — both
the `by_key` and `by_priority` indices are transmitted and reconstructed.
