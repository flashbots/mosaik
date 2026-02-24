# Error Handling

Mosaik uses typed error enums specific to each subsystem, plus a
close-reason system for QUIC connection-level codes. This chapter catalogs
every public error type.

## Network errors

`network::Error` — returned by `NetworkBuilder::build()`:

| Variant                               | Description                                |
| ------------------------------------- | ------------------------------------------ |
| `MissingNetworkId`                    | No `network_id` was set on the builder     |
| `Bind(BindError)`                     | Failed to bind the QUIC endpoint           |
| `InvalidAddress(InvalidSocketAddr)`   | An address in `addresses` is invalid       |
| `DiscoveryConfig(ConfigBuilderError)` | Discovery config builder failed validation |
| `StreamsConfig(ConfigBuilderError)`   | Streams config builder failed validation   |
| `GroupsConfig(ConfigBuilderError)`    | Groups config builder failed validation    |

## Discovery errors

`discovery::Error`:

| Variant                                              | Description                                 |
| ---------------------------------------------------- | ------------------------------------------- |
| `InvalidSecretKey(PeerId, PeerId)`                   | Secret key does not match expected `PeerId` |
| `DifferentNetwork { local_network, remote_network }` | Remote peer is on a different network       |
| `InvalidSignature`                                   | Peer entry signature verification failed    |
| `GossipJoin(ApiError)`                               | Failed to join gossip topic (iroh_gossip)   |
| `PeerIdChanged(PeerId, PeerId)`                      | Attempted to change the local peer's ID     |
| `Link(LinkError)`                                    | Transport-level link error                  |
| `Other(Box<dyn Error>)`                              | Generic boxed error                         |
| `Cancelled`                                          | Operation was cancelled                     |

## Command errors

`groups::CommandError<M>` — returned by `execute()`, `execute_many()`,
`feed()`, `feed_many()`:

| Variant                    | Recoverable? | Description                                            |
| -------------------------- | ------------ | ------------------------------------------------------ |
| `Offline(Vec<M::Command>)` | **Yes**      | Node is offline; carries the unsent commands for retry |
| `NoCommands`               | No           | Empty command batch was submitted                      |
| `GroupTerminated`          | No           | The group has been permanently closed                  |

### Recovering from `Offline`

```rust,ignore
match group.execute(MyCommand::Increment).await {
    Ok(result) => println!("committed: {result:?}"),
    Err(CommandError::Offline(commands)) => {
        // Wait for the group to come online, then retry
        group.when().online().await;
        for cmd in commands {
            group.execute(cmd).await?;
        }
    }
    Err(CommandError::GroupTerminated) => {
        // Permanent failure — stop trying
    }
    Err(CommandError::NoCommands) => unreachable!(),
}
```

## Query errors

`groups::QueryError<M>` — returned by `query()`:

| Variant             | Recoverable? | Description                               |
| ------------------- | ------------ | ----------------------------------------- |
| `Offline(M::Query)` | **Yes**      | Node is offline; carries the unsent query |
| `GroupTerminated`   | No           | The group has been permanently closed     |

## Collection errors

`collections::Error<T>` — returned by collection write operations:

| Variant       | Recoverable? | Description                                  |
| ------------- | ------------ | -------------------------------------------- |
| `Offline(T)`  | **Yes**      | Node is offline; carries the value for retry |
| `NetworkDown` | No           | Network has shut down                        |

The generic `T` contains the value that failed to be written, enabling retry
without re-creating the data:

```rust,ignore
match map.insert("key".into(), 42).await {
    Ok(prev) => println!("previous: {prev:?}"),
    Err(collections::Error::Offline(value)) => {
        // value == 42, retry later
    }
    Err(collections::Error::NetworkDown) => {
        // permanent failure
    }
}
```

## Producer errors

`streams::producer::Error<D>` — returned by `Sink::send()` and `try_send()`:

| Variant             | Description                             |
| ------------------- | --------------------------------------- |
| `Closed(Option<D>)` | Producer has been closed                |
| `Full(D)`           | Internal buffer is full (back-pressure) |
| `Offline(D)`        | No active consumers connected           |

All variants carry the datum back when possible, enabling retry.

## Close reasons (QUIC application codes)

Mosaik uses typed **close reasons** for QUIC `ApplicationClose` codes. These
are generated with the `make_close_reason!` macro and appear in connection
close frames.

### Reserved ranges

| Range | Owner               |
| ----- | ------------------- |
| 0–199 | mosaik internal     |
| 200+  | Application-defined |

### Built-in close reasons

| Name                | Code | Description                         |
| ------------------- | ---- | ----------------------------------- |
| `Success`           | 200  | Protocol completed successfully     |
| `GracefulShutdown`  | 204  | Graceful shutdown                   |
| `InvalidAlpn`       | 100  | Wrong ALPN protocol                 |
| `DifferentNetwork`  | 101  | Peer on a different network         |
| `Cancelled`         | 102  | Operation cancelled                 |
| `UnexpectedClose`   | 103  | Unexpected connection close         |
| `ProtocolViolation` | 400  | Protocol message violation          |
| `UnknownPeer`       | 401  | Peer not found in discovery catalog |

### Group close reasons

| Name               | Code   | Description                  |
| ------------------ | ------ | ---------------------------- |
| `InvalidHandshake` | 30,400 | Handshake decode error       |
| `GroupNotFound`    | 30,404 | Unknown group ID             |
| `InvalidProof`     | 30,405 | Invalid authentication proof |
| `Timeout`          | 30,408 | Peer response timeout        |
| `AlreadyBonded`    | 30,429 | Duplicate bond between peers |

### Defining custom close reasons

```rust,ignore
use mosaik::network::make_close_reason;

// Code must be >= 200
make_close_reason!(MyAppError, 500, "Application-specific error");
```

## Error design patterns

### Temporary vs. permanent

Mosaik errors follow a consistent pattern:

- **Temporary** errors carry the original data back (e.g., `Offline(commands)`,
  `Full(datum)`) so you can retry without data loss.
- **Permanent** errors (e.g., `GroupTerminated`, `NetworkDown`) indicate the
  resource is gone and retrying is pointless.

### Matching on recoverability

```rust,ignore
use mosaik::collections::Error;

loop {
    match map.insert("key".into(), value.clone()).await {
        Ok(_) => break,
        Err(Error::Offline(_)) => {
            map.when().online().await;
            continue;
        }
        Err(Error::NetworkDown) => {
            panic!("network is gone");
        }
    }
}
```

### The `CloseReason` trait

All close reason types implement:

```rust,ignore
trait CloseReason:
    Error + Into<ApplicationClose> + PartialEq<ApplicationClose>
    + Clone + Send + Sync + 'static
{}
```

This lets you match connection close frames against typed reasons:

```rust,ignore
if close_frame == InvalidProof {
    // handle proof failure
}
```
