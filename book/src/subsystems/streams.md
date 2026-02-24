# Streams

Streams are the primary dataflow primitive in mosaik. They represent typed, asynchronous data channels that connect **producers** and **consumers** across a network.

## Overview

```
Producer Node                       Consumer Node
┌─────────────────┐                ┌─────────────────┐
│  Producer<D>    │────QUIC────────│  Consumer<D>     │
│  (Sink impl)    │  /mosaik/      │  (Stream impl)   │
│                 │  streams/1.0   │                   │
└─────────────────┘                └─────────────────┘
```

A producer announces a stream via Discovery. Consumers discover the producer, open a QUIC connection using the `/mosaik/streams/1.0` ALPN, and begin receiving data. The entire lifecycle is automatic — you only create the handles.

## The Datum Trait

Every type sent through a stream must implement `Datum`:

```rust,ignore
pub trait Datum: Serialize + DeserializeOwned + Send + 'static {
    fn derived_stream_id() -> StreamId {
        core::any::type_name::<Self>().into()
    }
}
```

`Datum` is a blanket impl — any `Serialize + DeserializeOwned + Send + 'static` type is automatically a `Datum`. The `derived_stream_id()` method computes a `StreamId` (a `Digest`) from the Rust type name, so each type naturally maps to a unique stream.

```rust,ignore
#[derive(Serialize, Deserialize)]
struct PriceUpdate {
    symbol: String,
    price: f64,
}

// PriceUpdate is automatically a Datum
// StreamId = blake3("my_crate::PriceUpdate")
```

## Quick Usage

**Producing data:**

```rust,ignore
let producer = network.streams().produce::<PriceUpdate>();

// Wait until at least one consumer is connected
producer.when().online().await;

// Send via Sink trait
use futures::SinkExt;
producer.send(PriceUpdate { symbol: "ETH".into(), price: 3200.0 }).await?;

// Or send immediately (non-blocking)
producer.try_send(PriceUpdate { symbol: "BTC".into(), price: 65000.0 })?;
```

**Consuming data:**

```rust,ignore
let mut consumer = network.streams().consume::<PriceUpdate>();

// Wait until connected to at least one producer
consumer.when().online().await;

// Receive via async method
while let Some(update) = consumer.recv().await {
    println!("{}: ${}", update.symbol, update.price);
}

// Or use as a futures::Stream
use futures::StreamExt;
while let Some(update) = consumer.next().await {
    println!("{}: ${}", update.symbol, update.price);
}
```

## Stream Identity

By default, a stream's identity comes from `Datum::derived_stream_id()`, which hashes the Rust type name. You can override this with a custom `StreamId`:

```rust,ignore
let producer = network.streams()
    .producer::<PriceUpdate>()
    .with_stream_id("custom-price-feed")
    .build()?;
```

This lets you have multiple distinct streams of the same data type.

## Architecture

Streams are built on top of the Discovery and Network subsystems:

1. **Producer creation** — the local discovery entry is updated to advertise the stream
2. **Consumer creation** — the consumer worker discovers producers via the catalog and opens subscriptions
3. **Subscription** — a QUIC bi-directional stream is opened; the consumer sends its `Criteria`, the producer sends data
4. **Fanout** — each consumer gets its own independent sender loop so a slow consumer does not block others
5. **Cleanup** — when handles are dropped, underlying tasks are cancelled

## Close Reason Codes

When a stream subscription fails, the producer sends structured close reasons:

| Code     | Name             | Meaning                                                          |
| -------- | ---------------- | ---------------------------------------------------------------- |
| `10_404` | `StreamNotFound` | The requested stream does not exist on the producer              |
| `10_403` | `NotAllowed`     | The consumer is rejected by the producer's `accept_if` predicate |
| `10_509` | `NoCapacity`     | The producer has reached `max_consumers`                         |
| `10_413` | `TooSlow`        | The consumer was disconnected for lagging behind                 |

## Subsystem Configuration

The `Streams` config currently has one setting:

```rust,ignore
Config::builder()
    .with_backoff(ExponentialBackoffBuilder::default()
        .with_max_elapsed_time(Some(Duration::from_secs(300)))
        .build())
    .build()?;
```

| Option    | Default                 | Description                                            |
| --------- | ----------------------- | ------------------------------------------------------ |
| `backoff` | Exponential (max 5 min) | Default backoff policy for consumer connection retries |

Individual producers and consumers can override this via their respective builders.
