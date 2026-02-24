# Testing Guide

This chapter covers how to test mosaik applications and how to work with
mosaik's own test infrastructure when contributing.

## Test setup

### Dependencies

Add these to your `[dev-dependencies]`:

```toml
[dev-dependencies]
mosaik = { version = "0.2" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

### Basic test structure

Every mosaik test follows the same pattern:

1. Create networks with in-process endpoints.
2. Connect them via `sync_with` (or mDNS for local tests).
3. Wait for discovery to propagate.
4. Exercise the API.
5. Assert state.

```rust,ignore
#[tokio::test]
async fn two_nodes_discover_each_other() {
    let net_a = Network::builder()
        .network_id("test")
        .build()
        .await
        .unwrap();

    let net_b = Network::builder()
        .network_id("test")
        .build()
        .await
        .unwrap();

    // Connect the two endpoints directly
    net_a.sync_with(net_b.endpoint_addr()).await.unwrap();

    // Wait for mutual discovery
    let event = net_a.discovery().events().recv().await.unwrap();
    assert!(matches!(event, Event::Discovered { .. }));
}
```

## Connecting test networks

### `sync_with`

The simplest way to connect two test nodes:

```rust,ignore
net_a.sync_with(net_b.endpoint_addr()).await?;
```

This synchronizes discovery catalogs between the two nodes, establishing
mutual awareness.

### Discover all (fan-out)

For multi-node tests, connect all pairs:

```rust,ignore
async fn discover_all(networks: &[&Network]) {
    let futs: Vec<_> = networks.iter()
        .flat_map(|a| networks.iter().map(move |b| {
            a.sync_with(b.endpoint_addr())
        }))
        .collect();
    futures::future::try_join_all(futs).await.unwrap();
}
```

Mosaik's test suite provides this as a utility function.

## Time management

### `TIME_FACTOR` environment variable

All test durations are multiplied by `TIME_FACTOR` (default `1.0`). This is
useful for running tests on slow CI machines or over high-latency networks:

```bash
# Double all timeouts for slow CI
TIME_FACTOR=2.0 cargo test

# 10x for debugging with breakpoints
TIME_FACTOR=10.0 cargo test -- --nocapture
```

### Time helper functions

| Function             | Description                              |
| -------------------- | ---------------------------------------- |
| `secs(n)`            | `Duration::from_secs(n) × TIME_FACTOR`   |
| `millis(n)`          | `Duration::from_millis(n) × TIME_FACTOR` |
| `sleep_s(n)`         | `tokio::time::sleep(secs(n))`            |
| `sleep_ms(n)`        | `tokio::time::sleep(millis(n))`          |
| `timeout_s(n, fut)`  | Timeout with location tracking           |
| `timeout_ms(n, fut)` | Timeout with location tracking           |

The `timeout_*` functions use `#[track_caller]` so timeout errors report the
**test** line number, not the utility function.

## Tracing

### Automatic initialization

Mosaik's test suite uses `#[ctor::ctor]` to initialize tracing before any test
runs. Control it with environment variables:

```bash
# Enable debug logging
TEST_TRACE=debug cargo test

# Available levels: trace, debug, info, warn, error
TEST_TRACE=trace cargo test -- test_name

# Show all modules (including noisy deps)
TEST_TRACE=debug TEST_TRACE_UNMUTE=1 cargo test
```

### Muted modules

By default, these noisy modules are filtered out:

- `iroh`, `rustls`, `igd_next`, `hickory_*`
- `hyper_util`, `portmapper`, `reqwest`
- `netwatch`, `mio`, `acto`, `swarm_discovery`
- `events.net.relay.connected`

Set `TEST_TRACE_UNMUTE=1` to see their output.

### Panic handling

The test harness installs a custom panic hook that:
1. Logs the panic via `tracing::error!`.
2. Calls `std::process::abort()` to prevent test framework from masking the
   panic in async contexts.

## Testing patterns

### Waiting for conditions

Use the `When` API instead of arbitrary sleeps:

```rust,ignore
// Wait for a group to come online
group.when().online().await;

// Wait for a collection to reach a version
map.when().updated(|v| v.index() >= 10).await;

// Wait for bonds to form
group.when().bonds(|b| b.len() >= 2).await;
```

### Testing state machines

Test your state machine in isolation before running it in a group:

```rust,ignore
#[test]
fn state_machine_logic() {
    let mut sm = Counter::default();
    let ctx = ApplyContext {
        index: 1,
        term: 1,
        leader: false,
    };

    let result = sm.apply(CounterCommand::Increment, ctx);
    assert_eq!(result, 0); // returns previous value

    let result = sm.apply(CounterCommand::Increment, ctx);
    assert_eq!(result, 1);
}
```

### Testing collections

```rust,ignore
#[tokio::test]
async fn replicated_map() {
    let (net_a, net_b) = create_connected_pair().await;

    let writer: MapWriter<String, u64> = net_a.collections()
        .map_writer("my-store")
        .build();

    let reader: MapReader<String, u64> = net_b.collections()
        .map_reader("my-store")
        .build();

    // Write on node A
    writer.insert("key".into(), 42).await.unwrap();

    // Wait for replication on node B
    reader.when().updated(|v| v.index() >= 1).await;

    // Read on node B
    assert_eq!(reader.get(&"key".into()), Some(42));
}
```

### Testing streams

```rust,ignore
#[tokio::test]
async fn stream_delivery() {
    let (net_a, net_b) = create_connected_pair().await;

    let producer = net_a.streams()
        .producer::<Message>("topic")
        .build();

    let consumer = net_b.streams()
        .consumer::<Message>("topic")
        .build();

    // Wait for consumer to connect
    producer.when().active().await;

    // Send and receive
    producer.send(Message("hello".into())).await.unwrap();
    let msg = consumer.recv().await.unwrap();
    assert_eq!(msg.0, "hello");
}
```

## Polling futures in tests

For testing poll-based logic:

```rust,ignore
use std::task::Poll;

/// Poll a future exactly once with a no-op waker
fn poll_once<F: Future + Unpin>(f: &mut F) -> Poll<F::Output> {
    let waker = futures::task::noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);
    Pin::new(f).poll(&mut cx)
}
```

## CI considerations

- Set `TIME_FACTOR=2.0` or higher for CI environments.
- Use `TEST_TRACE=debug` to capture logs on failure.
- Run tests with `--test-threads=1` if you encounter port conflicts.
- The test suite uses real networking (loopback), so ensure localhost UDP is
  available.

## Project test structure

```text
tests/
├── basic.rs            # Test harness, data types, module declarations
├── collections/        # Map, Vec, Set, DEPQ tests
├── discovery/          # Catalog, departure tests
├── groups/             # Bonds, builder, execute, feed, leader, catchup
├── streams/            # Smoke tests, stats, producer/consumer
└── utils/
    ├── mod.rs          # discover_all helper
    ├── tracing.rs      # Auto-init tracing with ctor
    ├── time.rs         # TIME_FACTOR-aware duration helpers
    └── fut.rs          # poll_once, forever
```
