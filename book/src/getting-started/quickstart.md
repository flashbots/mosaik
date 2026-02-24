# Quick Start

This guide walks through a complete example: two nodes that discover each other and exchange typed data through a stream.

## Step 1: Create the Network

Every mosaik application starts by creating a `Network`. The `NetworkId` ensures only nodes on the same logical network can communicate.

```rust,ignore
use mosaik::{Network, NetworkId};

let network_id: NetworkId = "my-app".into();
let network = Network::new(network_id).await?;
```

`Network::new()` creates a node with default settings:
- A random secret key (new identity each run — this is the recommended default)
- Default relay mode (uses iroh's relay servers for NAT traversal)
- No bootstrap peers (fine for local testing)
- No tags

For production use, you'll want `Network::builder()` to configure bootstrap peers so the node can find the network. A fixed secret key is only needed for bootstrap nodes that need a stable peer ID — regular nodes work fine with the auto-generated key. See the [Network](../subsystems/network.md) chapter.

## Step 2: Define Your Data Type

Streams in mosaik are typed. Any type implementing `Serialize + DeserializeOwned + Send + 'static` automatically implements the `Datum` trait and can be streamed:

```rust,ignore
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SensorReading {
    sensor_id: u64,
    temperature: f64,
    timestamp: u64,
}
```

## Step 3: Create a Producer

A producer publishes data for consumers across the network. The stream ID is derived from the type name by default.

```rust,ignore
let producer = network.streams().produce::<SensorReading>();

// Wait until at least one consumer connects
producer.when().subscribed().await;
```

## Step 4: Send Data

`Producer` implements the `futures::Sink` trait. You can use `SinkExt::send()` for awaitable sends:

```rust,ignore
use futures::SinkExt;

producer.send(SensorReading {
    sensor_id: 1,
    temperature: 22.5,
    timestamp: 1700000000,
}).await?;
```

For non-blocking sends, use `try_send()`:

```rust,ignore
producer.try_send(SensorReading {
    sensor_id: 1,
    temperature: 23.1,
    timestamp: 1700000001,
})?;
```

## Step 5: Create a Consumer (On Another Node)

On a different node (or the same node for testing), create a consumer for the same type:

```rust,ignore
let other_network = Network::new(network_id).await?;

let mut consumer = other_network.streams().consume::<SensorReading>();

// Wait for subscription to a producer
consumer.when().subscribed().await;
```

## Step 6: Receive Data

`Consumer` implements `futures::Stream`. Use `StreamExt::next()` to receive:

```rust,ignore
use futures::StreamExt;

while let Some(reading) = consumer.next().await {
    println!("Sensor {}: {}°C", reading.sensor_id, reading.temperature);
}
```

Or use the direct `recv()` method:

```rust,ignore
if let Some(reading) = consumer.recv().await {
    println!("Got reading: {:?}", reading);
}
```

## Step 7: Discovery (Connecting the Nodes)

For nodes to find each other, they need at least one common peer address. In local testing, you can trigger manual discovery:

```rust,ignore
// On the consumer's network, dial the producer's address
other_network.discovery().sync_with(network.local().addr()).await?;
```

In production, you'll configure bootstrap peers:

```rust,ignore
use mosaik::discovery;

let network = Network::builder(network_id)
    .with_discovery(
        discovery::Config::builder()
            .with_bootstrap(bootstrap_addr)
    )
    .build()
    .await?;
```

## Putting It All Together

Here's the complete example as two async tasks simulating two nodes:

```rust,ignore
use futures::{SinkExt, StreamExt};
use mosaik::{Network, NetworkId};
use serde::{Deserialize, Serialize};
use std::pin::pin;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SensorReading {
    sensor_id: u64,
    temperature: f64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let network_id: NetworkId = "sensor-network".into();

    // Node 1: Producer
    let node1 = Network::new(network_id).await?;
    let producer = node1.streams().produce::<SensorReading>();

    // Node 2: Consumer
    let node2 = Network::new(network_id).await?;
    let mut consumer = node2.streams().consume::<SensorReading>();

    // Connect the nodes
    node2.discovery().dial(node1.local().addr()).await?;

    // Wait for the stream to be established
    producer.when().online().await;

    // Send data
    for i in 0..10 {
        producer.send(SensorReading {
            sensor_id: 1,
            temperature: 20.0 + i as f64 * 0.5,
        }).await?;
    }

    // Receive data
    for _ in 0..10 {
        let reading = consumer.recv().await.unwrap();
        println!("{:?}", reading);
    }

    Ok(())
}
```

## What's Next

- **[Tutorials](../tutorials/bootstrap.md)** — Walk through the included examples
- **[Streams](../subsystems/streams.md)** — Producer/consumer configuration, filtering, backpressure
- **[Groups](../subsystems/groups.md)** — Raft consensus and replicated state machines
- **[Collections](../subsystems/collections.md)** — Distributed Map, Vec, Set, PriorityQueue
