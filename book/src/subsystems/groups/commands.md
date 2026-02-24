# Commands & Queries

Once you have a `Group<M>` handle, you interact with the replicated state machine through **commands** (writes) and **queries** (reads).

## Commands

Commands mutate state and are replicated to all group members through the Raft log. They are guaranteed to be applied in the same order on every node.

### `execute` — Send and Wait

Sends a command and waits for it to be committed (replicated to a quorum):

```rust,ignore
let index = group.execute(CounterCmd::Increment(5)).await?;
println!("Command committed at log index {index}");
```

If the local node is:
- **Leader**: replicates to followers, resolves when quorum acknowledges
- **Follower**: forwards to the leader, resolves when the leader commits

### `execute_many` — Batch Send and Wait

Send multiple commands atomically:

```rust,ignore
let range = group.execute_many([
    CounterCmd::Increment(1),
    CounterCmd::Increment(2),
    CounterCmd::Increment(3),
]).await?;
println!("Commands committed at indices {range:?}");
```

Returns an `IndexRange` (`RangeInclusive<Index>`) covering all committed entries.

### `feed` — Fire and Forget

Sends a command without waiting for commitment. Resolves once the leader acknowledges receipt and assigns a log index:

```rust,ignore
let index = group.feed(CounterCmd::Increment(10)).await?;
println!("Command assigned index {index}, not yet committed");
```

### `feed_many` — Batch Fire and Forget

```rust,ignore
let range = group.feed_many([
    CounterCmd::Reset,
    CounterCmd::Increment(100),
]).await?;
```

### Waiting for Commitment After Feed

Combine `feed` with the cursor watcher:

```rust,ignore
let range = group.feed_many(commands).await?;

// Wait until all commands are committed
group.when().committed().reaches(range.clone()).await;
```

### Command Errors

```rust,ignore
pub enum CommandError<M: StateMachine> {
    Offline(Vec<M::Command>),  // Node is offline; commands returned
    NoCommands,                // Empty command list
    GroupTerminated,           // Group is shut down
}
```

The `Offline` variant returns the unsent commands so they can be retried:

```rust,ignore
match group.execute(cmd).await {
    Ok(index) => println!("Committed at {index}"),
    Err(CommandError::Offline(cmds)) => {
        // Save for retry
        group.when().online().await;
        group.execute(cmds.into_iter().next().unwrap()).await?;
    }
    Err(CommandError::GroupTerminated) => {
        panic!("Group is gone");
    }
    Err(CommandError::NoCommands) => unreachable!(),
}
```

## Queries

Queries are read-only operations against the state machine. They are **not** replicated in the log.

### Weak Consistency

Reads from the local node's state machine. Fast but may return stale data:

```rust,ignore
let result = group.query(CounterQuery::Value, Consistency::Weak).await?;
println!("Local counter value: {} (at index {})", result.result, result.at_position);
```

### Strong Consistency

Forwards the query to the current leader, guaranteeing linearizable reads:

```rust,ignore
let result = group.query(CounterQuery::Value, Consistency::Strong).await?;
println!("Leader counter value: {} (at index {})", *result, result.state_position());
```

### CommittedQueryResult

Query results are wrapped in `CommittedQueryResult<M>`:

```rust,ignore
pub struct CommittedQueryResult<M: StateMachine> {
    pub result: M::QueryResult,    // The actual result
    pub at_position: Index,        // Log index at query time
}
```

It implements `Deref` to `M::QueryResult`, so you can use it directly:

```rust,ignore
let result = group.query(CounterQuery::Value, Consistency::Weak).await?;

// Deref to the inner result
let value: i64 = *result;

// Or access explicitly
let position = result.state_position();
let inner = result.into(); // Consume and get M::QueryResult
```

### Query Errors

```rust,ignore
pub enum QueryError<M: StateMachine> {
    Offline(M::Query),   // Node offline; query returned
    GroupTerminated,     // Group shut down
}
```

## Ordering Guarantee

Consecutive calls to `execute`, `execute_many`, `feed`, or `feed_many` on the same `Group` handle are guaranteed to be processed in the order they were issued.

## Common Patterns

### Command-Query Separation

```rust,ignore
// Write path: fire and forget for throughput
group.feed(OrderCmd::Place(order)).await?;

// Read path: weak consistency for speed
let book = group.query(OrderQuery::Snapshot, Consistency::Weak).await?;

// Read path: strong consistency for accuracy
let book = group.query(OrderQuery::Snapshot, Consistency::Strong).await?;
```

### Execute and Verify

```rust,ignore
let index = group.execute(CounterCmd::Increment(1)).await?;
let result = group.query(CounterQuery::Value, Consistency::Weak).await?;
assert!(result.at_position >= index);
```
