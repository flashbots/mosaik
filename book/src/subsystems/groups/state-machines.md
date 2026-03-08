# State Machines

Every group runs a replicated state machine (RSM) that processes commands and answers queries. The `StateMachine` trait is the primary extension point for application logic.

## The StateMachine Trait

```rust,ignore
pub trait StateMachine: Sized + Send + Sync + 'static {
    type Command: Command;       // Mutating operations
    type Query: Query;           // Read-only operations
    type QueryResult: QueryResult; // Query responses
    type StateSync: StateSync<Machine = Self>; // Catch-up strategy

    fn signature(&self) -> UniqueId;
    fn apply(&mut self, command: Self::Command, ctx: &dyn ApplyContext);
    fn query(&self, query: Self::Query) -> Self::QueryResult;
    fn state_sync(&self) -> Self::StateSync;

    // Optional overrides
    fn apply_batch(&mut self, commands: impl IntoIterator<Item = Self::Command>, ctx: &dyn ApplyContext) { ... }
    fn leadership_preference(&self) -> LeadershipPreference { LeadershipPreference::Normal }
}
```

### Associated Types

| Type          | Bound                                                   | Purpose                                           |
| ------------- | ------------------------------------------------------- | ------------------------------------------------- |
| `Command`     | `Clone + Send + Serialize + DeserializeOwned + 'static` | State-mutating operations replicated via Raft log |
| `Query`       | `Clone + Send + Serialize + DeserializeOwned + 'static` | Read-only operations, not replicated              |
| `QueryResult` | `Clone + Send + Serialize + DeserializeOwned + 'static` | Responses to queries                              |
| `StateSync`   | `StateSync<Machine = Self>`                             | How lagging followers catch up                    |

All message types get blanket implementations from `StateMachineMessage`, so any `Clone + Send + Serialize + DeserializeOwned + 'static` type qualifies.

## Implementing a State Machine

Here is a complete counter example:

```rust,ignore
use mosaik::groups::*;
use serde::{Serialize, Deserialize};

#[derive(Default)]
struct Counter {
    value: i64,
}

#[derive(Clone, Serialize, Deserialize)]
enum CounterCmd {
    Increment(i64),
    Decrement(i64),
    Reset,
}

#[derive(Clone, Serialize, Deserialize)]
enum CounterQuery {
    Value,
}

impl StateMachine for Counter {
    type Command = CounterCmd;
    type Query = CounterQuery;
    type QueryResult = i64;
    type StateSync = LogReplaySync<Self>;

    fn signature(&self) -> UniqueId {
        UniqueId::from("counter_v1")
    }

    fn apply(&mut self, command: CounterCmd, _ctx: &dyn ApplyContext) {
        match command {
            CounterCmd::Increment(n) => self.value += n,
            CounterCmd::Decrement(n) => self.value -= n,
            CounterCmd::Reset => self.value = 0,
        }
    }

    fn query(&self, query: CounterQuery) -> i64 {
        match query {
            CounterQuery::Value => self.value,
        }
    }

    fn state_sync(&self) -> LogReplaySync<Self> {
        LogReplaySync::default()
    }
}
```

## The `signature()` Method

Returns a `UniqueId` that is part of the `GroupId` derivation. **All group members must return the same signature.** Different signatures → different `GroupId` → nodes cannot bond.

Use it to version your state machine:

```rust,ignore
fn signature(&self) -> UniqueId {
    UniqueId::from("orderbook_matching_engine_v2")
}
```

## ApplyContext

The `apply()` method receives a `&dyn ApplyContext` providing deterministic metadata:

```rust,ignore
pub trait ApplyContext {
    fn committed(&self) -> Cursor;     // Last committed position before this batch
    fn log_position(&self) -> Cursor;  // Last log position
    fn current_term(&self) -> Term;    // Term of the commands being applied
}
```

> **Important:** The context is safe for deterministic state machines — it never exposes non-deterministic data that could diverge across nodes.

## Batch Apply

For performance, override `apply_batch`:

```rust,ignore
fn apply_batch(
    &mut self,
    commands: impl IntoIterator<Item = Self::Command>,
    ctx: &dyn ApplyContext,
) {
    // Apply all commands in a single database transaction
    let tx = self.db.begin();
    for command in commands {
        self.apply_one(&tx, command, ctx);
    }
    tx.commit();
}
```

The default implementation simply calls `apply()` for each command sequentially.

## NoOp State Machine

For leader-election-only use cases, mosaik provides `NoOp`:

```rust,ignore
#[derive(Debug, Default)]
pub struct NoOp;

impl StateMachine for NoOp {
    type Command = ();
    type Query = ();
    type QueryResult = ();
    type StateSync = LogReplaySync<Self>;
    // ...
}
```

Usage:

```rust,ignore
let group = network.groups().with_key(key).join(); // implicitly uses NoOp
```

## State Synchronization

The `state_sync()` method returns a `StateSync` implementation used when followers need to catch up. For most cases, use `LogReplaySync`:

```rust,ignore
fn state_sync(&self) -> LogReplaySync<Self> {
    LogReplaySync::default()
}
```

`LogReplaySync` replays committed log entries to bring the follower's state machine up to date. For advanced use cases (e.g., snapshot-based sync), implement the `StateSync` trait directly.

See the [State Sync](../internals/state-sync.md) deep dive for details.

## Leadership Preference

A state machine can declare its preference for assuming leadership within
the group by overriding `leadership_preference()`. This is a per-node
setting -- different nodes in the same group can return different values
without affecting the `GroupId`.

```rust,ignore
use mosaik::LeadershipPreference;

fn leadership_preference(&self) -> LeadershipPreference {
    LeadershipPreference::Observer
}
```

| Variant                      | Behavior                                                                |
| ---------------------------- | ----------------------------------------------------------------------- |
| `Normal` (default)           | Standard Raft behavior -- participates in elections as a candidate      |
| `Reluctant { factor: u32 }`  | Longer election timeouts (multiplied by `factor`), can still be elected |
| `Observer`                   | Never self-nominates as candidate; still votes and replicates log       |

The convenience constructor `LeadershipPreference::reluctant()` creates a
`Reluctant` variant with the default factor of 3.

> **Liveness warning:** If every node in a group returns `Observer`, no
> leader can ever be elected and the group will be unable to make progress.
> Ensure at least one node has `Normal` or `Reluctant` preference.

This mechanism is used internally by [collection readers](../collections/writer-reader.md),
which return `Observer` so that leadership stays on writer nodes where
writes are handled directly rather than being forwarded.

## Storage

Commands are persisted in a log via the `Storage` trait:

```rust,ignore
pub trait Storage<C: Command>: Send + 'static {
    fn append(&mut self, command: C, term: Term) -> Index;
    fn available(&self) -> RangeInclusive<Index>;
    fn get(&self, index: Index) -> Option<(C, Term)>;
    fn get_range(&self, range: &RangeInclusive<Index>) -> Vec<(Term, Index, C)>;
    fn truncate(&mut self, at: Index);
    fn last(&self) -> Cursor;
    fn term_at(&self, index: Index) -> Option<Term>;
    fn prune_prefix(&mut self, up_to: Index);
    fn reset_to(&mut self, cursor: Cursor);
}
```

`InMemoryLogStore<C>` is the default implementation. For durability, implement `Storage` with a persistent backend (disk, database, etc.).
