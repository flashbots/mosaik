# Metrics

Mosaik instruments all subsystems with the [`metrics`](https://docs.rs/metrics)
facade crate. When no recorder is installed the calls are zero-cost no-ops.
Install any compatible recorder (Prometheus, StatsD, etc.) to start collecting
data.

## Prometheus exporter

Enable the `prometheus` feature (on by default) and pass an address to the
network builder:

```rust,ignore
let network = Network::builder("my-app")
    .with_prometheus_addr("0.0.0.0:9000".parse().unwrap())
    .build()
    .await?;
```

An HTTP server starts at the given address and responds to any `GET` request
with the current metrics in Prometheus text exposition format.

## Labels

Most metrics carry labels so you can filter by resource:

| Label     | Applied to              | Value                       |
| --------- | ----------------------- | --------------------------- |
| `network` | discovery, groups       | Short hex of the NetworkId  |
| `group`   | groups, collections     | Short hex of the GroupId    |
| `stream`  | streams (producer side) | Short hex of the StreamId   |

## Metric catalog

### Discovery

| Metric                               | Type    | Labels    | Description                         |
| ------------------------------------ | ------- | --------- | ----------------------------------- |
| `mosaik.discovery.catalog.size`      | Gauge   | `network` | Number of known peers in catalog    |
| `mosaik.discovery.peers.discovered`  | Counter | `network` | Peers discovered via gossip         |
| `mosaik.discovery.peers.updated`     | Counter | `network` | Peer entry updates received         |
| `mosaik.discovery.peers.departed`    | Counter | `network` | Peers departed or purged as stale   |
| `mosaik.discovery.catalog.syncs`     | Counter | `network` | Completed catalog syncs             |
| `mosaik.discovery.gossip.neighbors`  | Gauge   | `network` | Active gossip swarm neighbors       |

### Groups

| Metric                                      | Type    | Labels             | Description                             |
| ------------------------------------------- | ------- | ------------------ | --------------------------------------- |
| `mosaik.groups.active`                       | Gauge   | —                  | Joined groups on this node              |
| `mosaik.groups.raft.elections`               | Counter | `network`, `group` | Leader elections started                |
| `mosaik.groups.raft.leader.changes`          | Counter | `network`, `group` | Leadership transitions                  |
| `mosaik.groups.raft.committed`               | Gauge   | `network`, `group` | Latest committed log index              |
| `mosaik.groups.raft.committee.voters`        | Gauge   | `network`, `group` | Voting members in the committee         |
| `mosaik.groups.raft.committee.non_voters`    | Gauge   | `network`, `group` | Caught-up non-voting followers          |
| `mosaik.groups.raft.committee.size`          | Gauge   | `network`, `group` | Total committee size (voters + others)  |
| `mosaik.groups.bonds.active`                 | Gauge   | `network`, `group` | Active bond connections in the group    |
| `mosaik.groups.bonds.bytes.sent`             | Counter | `network`, `group` | Bytes sent over bonds                   |
| `mosaik.groups.bonds.bytes.received`         | Counter | `network`, `group` | Bytes received over bonds               |
| `mosaik.groups.bonds.messages.sent`          | Counter | `network`, `group` | Messages sent over bonds                |
| `mosaik.groups.bonds.messages.received`      | Counter | `network`, `group` | Messages received over bonds            |

### Streams

| Metric                                          | Type    | Labels             | Description                             |
| ----------------------------------------------- | ------- | ------------------ | --------------------------------------- |
| `mosaik.streams.producers.active`                | Gauge   | —                  | Active producer sinks on this node      |
| `mosaik.streams.producer.consumers`              | Gauge   | `stream`, `network`| Connected consumers per producer        |
| `mosaik.streams.producer.consumers.accepted`     | Counter | `stream`, `network`| Consumer subscriptions accepted         |
| `mosaik.streams.producer.consumers.rejected`     | Counter | `stream`, `network`| Consumer subscriptions rejected         |
| `mosaik.streams.producer.items.sent`             | Counter | `stream`, `network`| Items fanned out to consumers           |
| `mosaik.streams.producer.bytes.sent`             | Counter | `stream`, `network`| Bytes fanned out to consumers           |
| `mosaik.streams.producer.items.dropped`          | Counter | `stream`, `network`| Items dropped (consumer lagging)        |
| `mosaik.streams.connections.accepted`            | Counter | —                  | Stream protocol connections accepted    |
| `mosaik.streams.connections.rejected`            | Counter | —                  | Stream protocol connections rejected    |
| `mosaik.streams.consumers.active`                | Gauge   | `stream`, `network`| Active consumer workers on this node    |
| `mosaik.streams.consumer.producers`              | Gauge   | `stream`, `network`| Connected producers per consumer        |
| `mosaik.streams.consumer.items.received`         | Counter | `stream`, `network`| Items received by consumers             |
| `mosaik.streams.consumer.bytes.received`         | Counter | `stream`, `network`| Bytes received by consumers             |
| `mosaik.streams.datums.total`                    | Counter | —                  | Global datums transmitted (all streams) |
| `mosaik.streams.bytes.total`                     | Counter | —                  | Global bytes transmitted (all streams)  |

### Collections

| Metric                               | Type    | Labels             | Description                                |
| ------------------------------------ | ------- | ------------------ | ------------------------------------------ |
| `mosaik.collections.map.size`        | Gauge   | `network`, `group` | Number of entries in a Map                 |
| `mosaik.collections.vec.size`        | Gauge   | `network`, `group` | Number of elements in a Vec                |
| `mosaik.collections.set.size`        | Gauge   | `network`, `group` | Number of elements in a Set                |
| `mosaik.collections.cell.size`       | Gauge   | `network`, `group` | 1 if Cell has a value, 0 otherwise         |
| `mosaik.collections.once.size`       | Gauge   | `network`, `group` | 1 if Once has been written, 0 otherwise    |
| `mosaik.collections.depq.size`       | Gauge   | `network`, `group` | Number of entries in a PriorityQueue       |
| `mosaik.collections.syncs.started`   | Counter | `network`, `group` | Snapshot sync operations initiated         |

### Network

| Metric                               | Type    | Labels | Description                           |
| ------------------------------------ | ------- | ------ | ------------------------------------- |
| `mosaik.network.connections.opened`  | Counter | —      | Total Link connections opened         |
| `mosaik.network.egress.bytes`        | Counter | —      | Total bytes sent across all links     |
| `mosaik.network.ingress.bytes`       | Counter | —      | Total bytes received across all links |

## Custom recorder

If you prefer a different exporter or need to combine mosaik metrics with your
application's own metrics, install any `metrics::Recorder` before building the
network and omit `with_prometheus_addr`:

```rust,ignore
// install your own recorder
metrics::set_global_recorder(my_recorder).unwrap();

// build without the built-in exporter
let network = Network::builder("my-app").build().await?;
```

All `mosaik.*` metrics will flow through your recorder automatically.
