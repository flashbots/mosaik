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

| Code     | Name             | Meaning                                                        |
| -------- | ---------------- | -------------------------------------------------------------- |
| `10_404` | `StreamNotFound` | The requested stream does not exist on the producer            |
| `10_403` | `NotAllowed`     | The consumer is rejected by the producer's `require` predicate |
| `10_509` | `NoCapacity`     | The producer has reached `max_consumers`                       |
| `10_413` | `TooSlow`        | The consumer was disconnected for lagging behind               |

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

## The `stream!` macro (recommended)

The `stream!` macro is the recommended way to declare named stream
definitions. It generates a struct with a compile-time `StreamId` and
implements the `StreamProducer` and/or `StreamConsumer` traits, so you
can create producers and consumers without repeating the stream ID or
configuration at every call site.

### Syntax

```rust,ignore
use mosaik::*;

// Type-derived StreamId (most common):
declare::stream!(pub PriceFeed = PriceUpdate);

// Explicit StreamId:
declare::stream!(pub PriceFeed = PriceUpdate, "oracle.price");

// Producer only:
declare::stream!(pub producer PriceFeed = PriceUpdate, "oracle.price");

// Consumer only:
declare::stream!(pub consumer PriceFeed = PriceUpdate, "oracle.price");
```

The three modes control which traits are implemented on the generated struct:

| Mode        | Implements                                |
| ----------- | ----------------------------------------- |
| *(default)* | `StreamProducer` **and** `StreamConsumer` |
| `producer`  | `StreamProducer` only                     |
| `consumer`  | `StreamConsumer` only                     |

### Baked-in configuration

Configuration keys can be added after the stream ID. The macro routes
each key to the correct builder (producer or consumer) automatically:

```rust,ignore
declare::stream!(pub PriceFeed = PriceUpdate, "oracle.price",
    producer require: |peer| peer.tags().contains(&tag!("trusted")),
    producer online_when: |c| c.minimum_of(2),
    consumer require: |peer| true,
);
```

#### Producer-side keys (inferred automatically)

| Key                  | Description                              |
| -------------------- | ---------------------------------------- |
|                      |                                          |
| `max_consumers`      | Maximum number of concurrent consumers   |
| `buffer_size`        | Internal channel buffer size             |
| `disconnect_lagging` | Disconnect slow consumers after duration |

#### Consumer-side keys (inferred automatically)

| Key        | Description          |
| ---------- | -------------------- |
| `criteria` | Data range criteria  |
| `backoff`  | Retry backoff policy |

#### Ambiguous keys (prefix with `producer` or `consumer`)

| Key              | Description                                                |
| ---------------- | ---------------------------------------------------------- |
| `require`        | Conditions for accepting a consumer or producer connection |
| `require_ticket` | Expiration-aware ticket validator (stackable; see below)   |
| `online_when`    | Conditions for the stream to be considered online          |

Without a prefix, ambiguous keys apply to **both** sides. Use
`producer` or `consumer` prefixes to target one:

```rust,ignore
stream!(pub PriceFeed = PriceUpdate,
    producer online_when: |c| c.minimum_of(2),
    consumer online_when: |c| c.minimum_of(1),
);
```

#### Ticket validator

The `require_ticket` key accepts an expression that implements
`TicketValidator`. It maps to `.require_ticket(expr)` on the
underlying builder. Multiple `require_ticket` entries can be specified
(peers must satisfy all of them). When the earliest ticket expires, the
connection is automatically terminated:

```rust,ignore
use mosaik::*;

declare::stream!(pub AuthFeed = PriceUpdate, "auth.feed",
    require_ticket: MyJwtValidator::new(),
);
```

See [Discovery > Auth Tickets](../discovery/tickets.md) for the
`TicketValidator` trait and JWT examples.

### Usage

Call the trait methods on the generated struct to create producer or
consumer instances:

```rust,ignore
use mosaik::*;

declare::stream!(pub PriceFeed = PriceUpdate, "oracle.price");

struct MyType {
	producer: ProducerOf<PriceFeed>,
}

impl MyType {
	pub fn new(network: &mosaik::Network) -> Self {
		Self { producer: PriceFeed::producer(network) }
	}
}
```

#### `online_producer` / `online_consumer`

Each trait also provides a convenience method that creates the handle and
awaits `.when().online()` in a single call:

```rust,ignore
// These two are equivalent:
let producer = PriceFeed::producer(&network);
producer.when().online().await;

let producer = PriceFeed::online_producer(&network).await;
```

This is useful when you don't need to do anything between construction and
the online check — it reduces the common two-step pattern to a single
expression.

The `ProducerOf<S>` and `ConsumerOf<S>` type aliases (re-exported at
the crate root) resolve to the concrete producer or consumer type for
a given stream definition:

| Alias           | Expands to                        |
| --------------- | --------------------------------- |
| `ProducerOf<S>` | `<S as StreamProducer>::Producer` |
| `ConsumerOf<S>` | `<S as StreamConsumer>::Consumer` |

### Stream ID derivation

When no stream ID argument is provided, the `StreamId` is derived from
the datum type's Rust type name via `Datum::derived_stream_id()`. When
a string literal is provided, it is hashed with blake3 at compile time
(or decoded directly if exactly 64 hex characters). You can also pass
a `const` expression:

```rust,ignore
use mosaik::*;

const MY_STREAM: StreamId = unique_id!("my.stream");
declare::stream!(pub MyStream = String, MY_STREAM);
```

### Doc comments

Doc comments can be placed before the visibility modifier:

```rust,ignore
declare::stream!(
    /// The primary price oracle feed.
    pub PriceFeed = PriceUpdate, "oracle.price"
);
```

### When to use the macro vs. direct constructors

Use the `stream!` macro when:

- Multiple modules or crates reference the same stream.
- You want a single source of truth for the stream ID and configuration.
- You want compile-time checked producer/consumer type aliases via
  `ProducerOf<S>` / `ConsumerOf<S>`.

Use direct constructors (`network.streams().produce::<T>()`) when:

- You only need the stream in one place.
- The stream ID or configuration is computed at runtime.

## Stream definitions (`ProducerDef` / `ConsumerDef`)

> **Note:** The [`stream!` macro](#the-stream-macro-recommended) is
> now the recommended approach for declaring named stream definitions.
> `ProducerDef` and `ConsumerDef` still work and are useful when you
> need an explicit definition value (e.g., passing a definition as a
> function argument).

Instead of passing a `StreamId` to each builder call, you can define a
stream's identity once as a constant and create builders from it.

```rust,ignore
use mosaik::{unique_id, streams::{ProducerDef, ConsumerDef}};

const PRICES: ProducerDef<PriceUpdate> =
	ProducerDef::new(Some(unique_id!("prices")));

// Returns a pre-configured builder
let producer = PRICES.open(&network).build()?;
```

| Type             | Description                                      |
| ---------------- | ------------------------------------------------ |
| `ProducerDef<T>` | Producer definition — creates a producer builder |
| `ConsumerDef<T>` | Consumer definition — creates a consumer builder |

Both types are `const`-constructible and can be used as top-level
constants.
