# Status & Conditions

Both producers and consumers expose a reactive status API through `When` and `ChannelConditions`. This allows you to write event-driven code that reacts to changes in subscription state.

## The `When` API

Every `Producer` and `Consumer` provides a `when()` method that returns a `&When` handle:

```rust,ignore
let producer = network.streams().produce::<MyDatum>();
let when = producer.when();
```

### Online / Offline

```rust,ignore
// Resolves when the channel is ready (all publishing conditions met)
when.online().await;

// Resolves when the channel goes offline
when.offline().await;

// Check immediately
if when.is_online() { /* ... */ }
```

For producers, "online" means the `online_when` conditions are met (default: at least 1 consumer). For consumers, "online" means at least one producer connection is active.

### Subscribed / Unsubscribed

```rust,ignore
// Resolves when at least one peer is connected
when.subscribed().await;

// Resolves when zero peers are connected
when.unsubscribed().await;
```

## ChannelConditions

`when().subscribed()` returns a `ChannelConditions` — a composable future that resolves when the subscription state matches your criteria.

### Minimum Peers

```rust,ignore
// Wait for at least 3 connected peers
when.subscribed().minimum_of(3).await;
```

### Tag Filtering

```rust,ignore
// Wait for peers that have the "validator" tag
when.subscribed().with_tags("validator").await;

// Multiple tags (all must be present on the peer)
when.subscribed().with_tags(["validator", "us-east"]).await;
```

### Custom Predicates

```rust,ignore
// Wait for a peer matching an arbitrary condition
when.subscribed()
    .with_predicate(|peer: &PeerEntry| peer.tags().len() >= 3)
    .await;
```

### Combining Conditions

Conditions compose naturally:

```rust,ignore
// At least 2 validators from the us-east region
when.subscribed()
    .minimum_of(2)
    .with_tags("validator")
    .with_predicate(|p| p.tags().contains(&"us-east".into()))
    .await;
```

### Inverse Conditions

Use `unmet()` to invert a condition:

```rust,ignore
// Wait until there are fewer than 3 validators
when.subscribed()
    .minimum_of(3)
    .with_tags("validator")
    .unmet()
    .await;
```

### Re-Triggering

`ChannelConditions` implements `Future` and can be polled repeatedly. After resolving, it resets and will resolve again when the condition transitions from **not met → met**. This makes it suitable for use in loops:

```rust,ignore
let mut condition = when.subscribed().minimum_of(2);
loop {
    (&mut condition).await;
    println!("We now have 2+ subscribers again!");
}
```

### Checking Immediately

```rust,ignore
let condition = when.subscribed().minimum_of(3);
if condition.is_condition_met() {
    // There are currently 3+ matching peers
}
```

`ChannelConditions` also supports comparison with `bool`:

```rust,ignore
if when.subscribed().minimum_of(3) == true {
    // equivalent to is_condition_met()
}
```

## ChannelInfo

`ChannelInfo` represents an individual connection between a consumer and a producer:

```rust,ignore
pub struct ChannelInfo {
    stream_id: StreamId,
    criteria: Criteria,
    producer_id: PeerId,
    consumer_id: PeerId,
    stats: Arc<Stats>,
    peer: Arc<PeerEntry>,
    state: watch::Receiver<State>,
}
```

### Connection State

```rust,ignore
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Connecting,   // Handshake in progress
    Connected,    // Active subscription
    Terminated,   // Unrecoverably closed
}
```

Monitor state changes:

```rust,ignore
let info: ChannelInfo = /* from producers() or consumers() */;

// Check current state
match info.state() {
    State::Connected => { /* active */ }
    State::Connecting => { /* handshake in progress */ }
    State::Terminated => { /* closed */ }
}

// Watch for state transitions
let mut watcher = info.state_watcher().clone();
while watcher.changed().await.is_ok() {
    println!("State changed to: {:?}", *watcher.borrow());
}

// Wait for disconnection
info.disconnected().await;
```

## Stats

Every channel tracks real-time statistics:

```rust,ignore
let stats = info.stats();

println!("Datums: {}", stats.datums());   // Total datums transferred
println!("Bytes: {}", stats.bytes());     // Total bytes transferred
println!("Uptime: {:?}", stats.uptime()); // Option<Duration> since connection

// Display formatting included
println!("{}", stats);
// Output: "uptime: 2m 30s, datums: 15432, bytes: 1.23 MB"
```

`Stats` implements `Display` with human-readable formatting using `humansize` and `humantime`.

## Using Status for Online Conditions

The `ChannelConditions` type is also used to configure producer online conditions via the builder:

```rust,ignore
let producer = network.streams()
    .producer::<MyDatum>()
    .online_when(|c| c.minimum_of(2).with_tags("validator"))
    .build()?;
```

The closure receives a `ChannelConditions` and returns it with your conditions applied. This is evaluated every time the subscription set changes.
