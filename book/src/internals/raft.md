# Raft Consensus

Mosaik's groups subsystem implements a **modified Raft** consensus algorithm
optimized for dynamic, self-organizing peer sets. This chapter covers the
differences from standard Raft and explains the internal implementation.

## Standard Raft recap

Raft organizes a cluster into a single **leader** and multiple **followers**.
The leader accepts client commands, appends them to a replicated log, and only
commits entries once a **quorum** (majority) of nodes has acknowledged them.
If the leader fails, an **election** promotes a new one.

## Mosaik's modifications

### 1. Non-voting followers (Abstention)

In standard Raft, every follower participates in elections and log replication
quorum counts. In mosaik, a follower can **abstain** from voting:

```text
enum Vote {
    Granted,    // Standard yes vote
    Denied,     // Standard no vote
    Abstained,  // Mosaik-specific: "I'm too far behind to vote"
}
```

A follower abstains when it detects that it is **lagging behind** the
leader's log and cannot verify log consistency. Abstaining removes the node
from the quorum denominator until it catches up. This prevents stale nodes
from blocking progress while still allowing them to receive new entries and
rejoin the quorum later.

### 2. No per-follower tracking on the leader

Standard Raft leaders maintain `nextIndex[]` and `matchIndex[]` arrays to
track each follower's log position. Mosaik's leader does **not** maintain
per-follower state. Instead:

- Each `AppendEntriesResponse` includes the follower's `last_log_index`.
- The leader uses these responses to calculate commit progress dynamically.
- This simplifies the leader and avoids stale state when group membership
  changes frequently.

### 3. Dynamic quorum

Because nodes can abstain, the quorum denominator changes at runtime:

```text
effective_quorum = (voting_nodes / 2) + 1
```

Where `voting_nodes = total_bonded_peers - abstaining_peers`. This allows
the cluster to make progress even when some nodes are syncing or offline,
as long as a majority of the **voting** members agree.

### 4. Distributed catch-up (state sync)

When a follower falls too far behind to replay individual log entries, mosaik
uses a **state sync** mechanism rather than the leader shipping log snapshots:

1. The follower sends a `RequestSnapshot` to the leader.
2. The leader wraps it as a command and replicates it through the log.
3. **All** peers create a snapshot at the committed position of that command.
4. The follower fetches snapshot data in batches from **multiple** peers in
   parallel, distributing the load.
5. Once complete, the follower installs the snapshot and replays any buffered
   commands.

This is fundamentally different from standard Raft's approach where only the
leader sends snapshots.

### 5. Leadership deprioritization

Nodes can configure longer election timeouts to reduce the probability of
becoming leader:

```rust,ignore
ConsensusConfig::default().deprioritize_leadership()
```

This is used by collection readers, which prefer to leave leadership to writer
nodes.

### 6. Bootstrap delay

The first term (`Term::zero()`) adds an extra `bootstrap_delay` (default 3s)
to the election timeout. This gives all nodes time to start, discover each
other, and form bonds before the first election fires.

## Roles and state transitions

```text
          bootstrap_delay
               │
               ▼
     ┌────────────────┐     election timeout
     │    Follower     │─────────────────────┐
     │  (passive)      │                     │
     └────────┬───────┘                     │
              │ AppendEntries               │
              │ from leader                 ▼
              │                    ┌──────────────┐
              │                    │   Candidate   │
              │                    │ (requesting   │
              │                    │    votes)     │
              │                    └──────┬───────┘
              │                           │ majority
              │                           │ granted
              │                           ▼
              │                    ┌──────────────┐
              └────────────────────│    Leader     │
                 higher term       │ (active,      │
                 received          │  heartbeats)  │
                                   └──────────────┘
```

Each role has specific responsibilities:

| Role          | Key actions                                                                                                                            |
| ------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| **Follower**  | Respond to AppendEntries, vote in elections, forward commands to leader, detect leader failure via election timeout                    |
| **Candidate** | Increment term, vote for self, send RequestVote to all peers, transition to Leader on majority or back to Follower on higher term      |
| **Leader**    | Accept client commands, replicate log entries, send heartbeats, calculate dynamic quorum, commit entries, respond to forwarded queries |

## Message types

| Message                  | Direction          | Purpose                                                        |
| ------------------------ | ------------------ | -------------------------------------------------------------- |
| `AppendEntries`          | Leader → Followers | Replicate log entries / heartbeat                              |
| `AppendEntriesResponse`  | Follower → Leader  | Acknowledge entries, report last log index, grant/deny/abstain |
| `RequestVote`            | Candidate → All    | Request vote for election                                      |
| `RequestVoteResponse`    | All → Candidate    | Grant, deny, or abstain                                        |
| `Forward::Command`       | Follower → Leader  | Forward client commands                                        |
| `Forward::CommandAck`    | Leader → Follower  | Return assigned log indices                                    |
| `Forward::Query`         | Follower → Leader  | Forward strong-consistency query                               |
| `Forward::QueryResponse` | Leader → Follower  | Return query result and position                               |
| `StateSync(...)`         | Peer ↔ Peer        | State sync protocol messages                                   |

## Election timing

Elections are controlled by `ConsensusConfig`:

| Parameter                 | Default | Purpose                                             |
| ------------------------- | ------- | --------------------------------------------------- |
| `heartbeat_interval`      | 500ms   | How often the leader sends heartbeats               |
| `heartbeat_jitter`        | 150ms   | Random jitter subtracted from heartbeat interval    |
| `election_timeout`        | 2s      | Base timeout before a follower starts an election   |
| `election_timeout_jitter` | 500ms   | Random jitter added to election timeout             |
| `bootstrap_delay`         | 3s      | Extra delay for the very first election (term 0)    |
| `max_missed_heartbeats`   | 10      | Bond heartbeats missed before considering peer dead |

The randomized timeouts ensure that in most cases only one node transitions to
candidate at a time, avoiding split votes.

## Command flow

### Write path (leader)

```text
Client ──execute()──► Leader
                        │
                        ├─ append to local log
                        ├─ send AppendEntries to followers
                        │
                        │◄── AppendEntriesResponse (majority)
                        │
                        ├─ advance commit index
                        ├─ apply to state machine
                        └─ return Result to client
```

### Write path (follower)

```text
Client ──execute()──► Follower
                         │
                         ├─ Forward::Command to leader
                         │
                         │◄── Forward::CommandAck (assigned index)
                         │
                         │ ... wait for local commit to reach index ...
                         │
                         └─ return Result to client
```

### Read path

- **Weak consistency**: Read directly from local state machine (any role).
- **Strong consistency**: Forward query to leader, which reads from its
  always-up-to-date state machine and returns the result with commit position.

## Internal types

The implementation is split across several modules:

| Module              | Contents                                                                        |
| ------------------- | ------------------------------------------------------------------------------- |
| `raft/mod.rs`       | `Raft<S, M>` — top-level driver, delegates to current role                      |
| `raft/role.rs`      | `Role` enum (Follower, Candidate, Leader), shared message handling              |
| `raft/shared.rs`    | `Shared<S, M>` — state shared across all roles (storage, state machine, config) |
| `raft/leader.rs`    | Leader-specific logic: heartbeats, replication, dynamic quorum                  |
| `raft/follower.rs`  | Follower-specific logic: elections, forwarding, catch-up                        |
| `raft/candidate.rs` | Candidate-specific logic: vote collection, timeout                              |
| `raft/protocol.rs`  | Message type definitions                                                        |
