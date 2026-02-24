# Events

The Discovery subsystem emits events when the peer catalog changes. You can subscribe to these events for reactive programming.

## Subscribing to Events

```rust,ignore
let mut events = network.discovery().events();

while let Ok(event) = events.recv().await {
    match event {
        Event::PeerDiscovered(entry) => {
            println!("New peer joined: {}", entry.peer_id());
        }
        Event::PeerUpdated(entry) => {
            println!("Peer updated: {}", entry.peer_id());
        }
        Event::PeerDeparted(peer_id) => {
            println!("Peer departed: {}", peer_id);
        }
    }
}
```

The `events()` method returns a `tokio::sync::broadcast::Receiver<Event>`. Multiple consumers can subscribe independently.

## Event Types

### PeerDiscovered

Emitted when a previously unknown peer is added to the catalog:

```rust,ignore
Event::PeerDiscovered(signed_peer_entry)
```

This fires for both signed entries (from gossip/sync) and unsigned entries (from manual `insert()`).

### PeerUpdated

Emitted when an existing peer's entry changes:

```rust,ignore
Event::PeerUpdated(signed_peer_entry)
```

Common triggers:
- Peer added or removed tags
- Peer started or stopped producing a stream
- Peer joined or left a group
- Peer's address changed

### PeerDeparted

Emitted when a peer is removed from the catalog:

```rust,ignore
Event::PeerDeparted(peer_id)
```

This fires when:
- A peer's entry becomes stale and is purged
- A peer gracefully departs via the departure protocol
- An unsigned entry is removed via `discovery.remove(peer_id)`

## Backlog

The event channel retains up to `events_backlog` (default: 100) past events. If a consumer falls behind, older events are dropped and the consumer receives a `RecvError::Lagged(n)` indicating how many events were missed.

```rust,ignore
match events.recv().await {
    Ok(event) => { /* handle event */ }
    Err(broadcast::error::RecvError::Lagged(n)) => {
        tracing::warn!("Missed {n} events, consider full catalog re-read");
        let catalog = discovery.catalog();
        // Reconcile from snapshot
    }
    Err(broadcast::error::RecvError::Closed) => break,
}
```

## Catalog Watch vs Events

Choose the right tool:

| Use Case                     | Tool              | Why                                      |
| ---------------------------- | ----------------- | ---------------------------------------- |
| React to specific changes    | `events()`        | Get individual events with full context  |
| Get current state            | `catalog()`       | Snapshot of all peers                    |
| React to any change          | `catalog_watch()` | Triggered on every update, borrow latest |
| Build a UI dashboard         | `catalog_watch()` | Re-render on changes, read full state    |
| Log peer arrivals/departures | `events()`        | Specific events about each peer          |
