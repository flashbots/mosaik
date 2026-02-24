# Producers

A `Producer<D>` is a handle for sending data of type `D` to all connected consumers. Multiple `Producer` handles can exist for the same stream (mpsc pattern) — they share the same underlying fanout sink.

## Creating Producers

**Simple (default configuration):**

```rust,ignore
let producer = network.streams().produce::<MyDatum>();
```

If a producer for this datum type already exists, the existing one is returned.

**With builder (advanced configuration):**

```rust,ignore
let producer = network.streams()
    .producer::<MyDatum>()
    .accept_if(|peer| peer.tags().contains(&"trusted".into()))
    .online_when(|c| c.minimum_of(2).with_tags("validator"))
    .disconnect_lagging(true)
    .with_buffer_size(2048)
    .with_max_consumers(10)
    .with_stream_id("custom-id")
    .build()?;
```

## Builder Options

| Method                          | Default                  | Description                                              |
| ------------------------------- | ------------------------ | -------------------------------------------------------- |
| `accept_if(predicate)`          | Accept all               | Predicate to accept/reject incoming consumer connections |
| `online_when(conditions)`       | `minimum_of(1)`          | Conditions under which the producer is online            |
| `disconnect_lagging(bool)`      | `true`                   | Disconnect consumers that fall behind `buffer_size`      |
| `with_buffer_size(n)`           | `1024`                   | Internal channel buffer size                             |
| `with_max_consumers(n)`         | `usize::MAX`             | Maximum allowed simultaneous consumers                   |
| `with_stream_id(id)`            | `D::derived_stream_id()` | Custom stream identity                                   |
| `with_undelivered_sink(sender)` | None                     | Capture datum that no consumer matched                   |

## Sending Data

### Via `Sink` Trait

`Producer<D>` implements `futures::Sink<D>`. The `send()` method waits for the producer to be online before accepting data:

```rust,ignore
use futures::SinkExt;

// Blocks until at least one consumer is connected (default online condition)
producer.send(datum).await?;
```

If the producer is offline, `poll_ready` will not resolve until the online conditions are met.

### Via `try_send`

For non-blocking sends:

```rust,ignore
match producer.try_send(datum) {
    Ok(()) => { /* sent to fanout */ }
    Err(Error::Offline(d)) => { /* no consumers, datum returned */ }
    Err(Error::Full(d)) => { /* buffer full, datum returned */ }
    Err(Error::Closed(d)) => { /* producer closed */ }
}
```

All error variants return the unsent datum so you can retry or inspect it.

## Online Conditions

By default, a producer is online when it has at least one connected consumer. Customize this:

```rust,ignore
// Online when at least 3 validators are subscribed
let producer = network.streams()
    .producer::<MyDatum>()
    .online_when(|c| c.minimum_of(3).with_tags("validator"))
    .build()?;
```

Check online status at any time:

```rust,ignore
if producer.is_online() {
    producer.try_send(datum)?;
}
```

## Observing Status

The `when()` API provides reactive status monitoring:

```rust,ignore
// Wait until online
producer.when().online().await;

// Wait until offline
producer.when().offline().await;

// Wait until subscribed by at least one peer
producer.when().subscribed().await;

// Wait until subscribed by at least N peers
producer.when().subscribed().minimum_of(3).await;

// Wait until subscribed by peers with specific tags
producer.when().subscribed().with_tags("validator").await;

// Wait until no subscribers
producer.when().unsubscribed().await;
```

## Inspecting Consumers

Iterate over the currently connected consumers:

```rust,ignore
for info in producer.consumers() {
    println!(
        "Consumer {} connected to stream {} — state: {:?}, stats: {}",
        info.consumer_id(),
        info.stream_id(),
        info.state(),
        info.stats(),
    );
}
```

Each `ChannelInfo` provides access to:
- `stream_id()` — the stream this subscription is for
- `producer_id()` / `consumer_id()` — the peer IDs
- `peer()` — the `PeerEntry` snapshot at subscription time
- `state()` — `Connecting`, `Connected`, or `Terminated`
- `state_watcher()` — a `watch::Receiver<State>` for monitoring changes
- `stats()` — `Stats` with `datums()`, `bytes()`, `uptime()`
- `is_connected()` — shorthand for `state() == Connected`
- `disconnected()` — future that resolves when terminated

## Undelivered Sink

Capture datum that did not match any consumer's criteria:

```rust,ignore
let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

let producer = network.streams()
    .producer::<MyDatum>()
    .with_undelivered_sink(tx)
    .build()?;

// In another task
while let Some(datum) = rx.recv().await {
    tracing::warn!("Datum had no matching consumer: {:?}", datum);
}
```

> **Note:** In default configuration (online when ≥ 1 subscriber), undelivered events only occur if connected consumers' criteria reject the datum. If you customize `online_when` to allow publishing with zero subscribers, the sink captures all datum sent while nobody is listening.

## Builder Errors

`build()` returns `Err(BuilderError::AlreadyExists(existing))` if a producer for the stream ID already exists. The error contains the existing `Producer<D>` handle so you can use it directly:

```rust,ignore
match network.streams().producer::<MyDatum>().build() {
    Ok(new) => new,
    Err(BuilderError::AlreadyExists(existing)) => existing,
}
```

The simple `produce()` method handles this automatically — returning the existing producer if one exists.
