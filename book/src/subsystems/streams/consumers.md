# Consumers

A `Consumer<D>` receives data of type `D` from remote producers. Each consumer has its own worker task that discovers producers, manages subscriptions, and delivers data.

## Creating Consumers

**Simple (default configuration):**

```rust,ignore
let mut consumer = network.streams().consume::<MyDatum>();
```

**With builder (advanced configuration):**

```rust,ignore
let mut consumer = network.streams()
    .consumer::<MyDatum>()
    .subscribe_if(|peer| peer.tags().contains(&"primary".into()))
    .with_criteria(Criteria::default())
    .with_stream_id("custom-id")
    .with_backoff(ExponentialBackoffBuilder::default()
        .with_max_elapsed_time(Some(Duration::from_secs(60)))
        .build())
    .build();
```

## Builder Options

| Method                    | Default                  | Description                               |
| ------------------------- | ------------------------ | ----------------------------------------- |
| `subscribe_if(predicate)` | Accept all               | Filter which producers to subscribe to    |
| `with_criteria(criteria)` | `Criteria::default()`    | Data selection criteria sent to producers |
| `with_stream_id(id)`      | `D::derived_stream_id()` | Custom stream identity                    |
| `with_backoff(policy)`    | From `Streams` config    | Backoff policy for reconnection retries   |

## Receiving Data

### Via `recv()`

The primary receiving method — async, blocks until data is available:

```rust,ignore
while let Some(datum) = consumer.recv().await {
    process(datum);
}
// Returns None when the consumer is closed
```

### Via `try_recv()`

Non-blocking receive for polling patterns:

```rust,ignore
match consumer.try_recv() {
    Ok(datum) => process(datum),
    Err(TryRecvError::Empty) => { /* no data available */ }
    Err(TryRecvError::Disconnected) => { /* consumer closed */ }
}
```

### Via `Stream` Trait

`Consumer<D>` implements `futures::Stream<Item = D>`:

```rust,ignore
use futures::StreamExt;

while let Some(datum) = consumer.next().await {
    process(datum);
}

// Or with combinators
let filtered = consumer
    .filter(|d| futures::future::ready(d.price > 100.0))
    .take(10)
    .collect::<Vec<_>>()
    .await;
```

## Producer Selection

By default, a consumer subscribes to **every** discovered producer of the same stream ID. Use `subscribe_if` to be selective:

```rust,ignore
// Only subscribe to producers tagged as "primary"
let consumer = network.streams()
    .consumer::<PriceUpdate>()
    .subscribe_if(|peer| peer.tags().contains(&"primary".into()))
    .build();
```

The predicate receives a `&PeerEntry` and is evaluated each time a new producer is discovered.

## Criteria

`Criteria` are sent to the producer when subscribing. They allow content-based filtering at the source. Currently `Criteria` is a placeholder that matches everything:

```rust,ignore
pub struct Criteria {}

impl Criteria {
    pub const fn matches<D: Datum>(&self, _item: &D) -> bool {
        true
    }
}
```

## Backoff & Reconnection

When a connection to a producer fails, the consumer retries with an exponential backoff policy.

**Global default** (from `Streams` config): exponential backoff up to 5 minutes.

**Per-consumer override:**

```rust,ignore
use backoff::ExponentialBackoffBuilder;

let consumer = network.streams()
    .consumer::<MyDatum>()
    .with_backoff(ExponentialBackoffBuilder::default()
        .with_max_elapsed_time(Some(Duration::from_secs(30)))
        .build())
    .build();
```

## Observing Status

The `when()` API mirrors the producer side:

```rust,ignore
// Wait until connected to at least one producer
consumer.when().online().await;

// Wait until connected to at least one producer
consumer.when().subscribed().await;

// Wait until connected to at least N producers
consumer.when().subscribed().to_at_least(3).await;

// Wait until no producers are connected
consumer.when().unsubscribed().await;
```

Check online status imperatively:

```rust,ignore
if consumer.is_online() {
    // consumer has active connections
}
```

## Statistics

Each consumer tracks aggregated stats:

```rust,ignore
let stats = consumer.stats();
println!("Received {} datums ({} bytes)", stats.datums(), stats.bytes());

if let Some(uptime) = stats.uptime() {
    println!("Connected for {:?}", uptime);
}
```

`Stats` provides:
- `datums()` — total number of datums received
- `bytes()` — total serialized bytes received
- `uptime()` — `Option<Duration>` since last connection (None if currently disconnected)

## Inspecting Producers

```rust,ignore
for info in consumer.producers() {
    println!(
        "Connected to producer {} — {:?}, {}",
        info.producer_id(),
        info.state(),
        info.stats(),
    );
}
```

Each `ChannelInfo` provides the same API as on the [producer side](producers.md#inspecting-consumers).

## Multiple Consumers

Multiple consumers can be created for the same stream. Each gets its own independent copy of the data:

```rust,ignore
let mut consumer_a = network.streams().consume::<PriceUpdate>();
let mut consumer_b = network.streams().consume::<PriceUpdate>();

// Both receive all PriceUpdate data independently
```

## Lifecycle

When a `Consumer` handle is dropped, its background worker task is cancelled (via a `DropGuard`) and all connections to producers are closed. No explicit shutdown is needed.
