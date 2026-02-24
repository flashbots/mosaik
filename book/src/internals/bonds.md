# Bonds

Bonds are mosaik's mechanism for maintaining a **fully-connected mesh** of
persistent, authenticated connections between all members of a group. Every
pair of peers in a group maintains exactly one bond.

## Lifecycle

A bond goes through three phases:

### 1. Establishment

When two peers discover they belong to the same group (via discovery events),
one initiates a connection using the `/mosaik/groups/1` ALPN protocol.

The handshake uses **mutual proof-of-knowledge** to verify both sides know the
group key before exchanging any group-internal state:

```text
Initiator                              Acceptor
    │                                      │
    ├── HandshakeStart ───────────────────►│
    │   · network_id                       │
    │   · group_id                         │
    │   · proof = blake3(session_secret    │
    │             ⊕ group_key)             │
    │   · known bonds list                 │
    │                                      │
    │                    verify proof       │
    │                                      │
    │◄─── HandshakeEnd ───────────────────┤
    │     · proof (acceptor's)             │
    │     · known bonds list               │
    │                                      │
    │   verify proof                       │
    │                                      │
    ▼   Bond established                   ▼
```

The **proof** is computed from the TLS session secret (unique per connection)
combined with the group key. This ensures:

- Both sides know the group key (authentication).
- The proof cannot be replayed on a different connection (freshness).
- No group secrets are transmitted in the clear.

If proof verification fails, the connection is dropped immediately.

### 2. Steady state (BondWorker)

Once established, a bond is managed by a `BondWorker` that:

- Sends and receives **heartbeats** (Ping/Pong) on a configurable interval.
- Relays **Raft messages** between peers.
- Propagates **peer entry updates** when discovery information changes.
- Announces **new bonds** (`BondFormed`) so peers learn about topology changes.
- Handles **graceful departure** when a peer is shutting down.

### 3. Failure and reconnection

If heartbeats are missed beyond the configured threshold, the bond is
considered failed and torn down. Both sides independently detect the failure.
When the peer reappears (via discovery), a new bond is established from
scratch with a fresh handshake.

## BondMessage protocol

After the handshake, all communication over a bond uses the `BondMessage`
enum:

| Variant                      | Direction | Purpose                                |
| ---------------------------- | --------- | -------------------------------------- |
| `Ping`                       | A → B     | Heartbeat probe                        |
| `Pong`                       | B → A     | Heartbeat response                     |
| `Departure`                  | Either    | Graceful shutdown notification         |
| `PeerEntryUpdate(PeerEntry)` | Either    | Discovery information changed          |
| `BondFormed(BondId, PeerId)` | Either    | Announce new bond to populate topology |
| `Raft(Bytes)`                | Either    | Wrapped Raft protocol message          |

All messages are serialized with **postcard** and framed with
`LengthDelimitedCodec` over a QUIC bidirectional stream (via `Link<P>`).

## Heartbeat mechanism

The `Heartbeat` struct tracks liveness:

```rust,ignore
struct Heartbeat {
    tick: Interval,        // fires every `base - jitter` to `base`
    last_recv: Instant,    // last time we received a Ping or Pong
    missed: u64,           // how many consecutive ticks without a response
    max_missed: u64,       // threshold from ConsensusConfig (default: 10)
    alert: Notify,         // signals when threshold is exceeded
}
```

On each tick:
1. Check if `last_recv` was recent enough.
2. If not, increment `missed`.
3. If `missed ≥ max_missed`, notify the alert future — the bond worker
   tears down the bond.

Receiving any message from the peer resets the counter:

```rust,ignore
heartbeat.reset(); // sets missed = 0 and refreshes last_recv
```

Timing uses **jitter** to prevent synchronized heartbeat storms. The actual
interval for each tick is randomized between `base - jitter` and `base`.

## Bond identity

Each bond is identified by a `BondId`:

```rust,ignore
type BondId = UniqueId;
```

The `BondId` is derived deterministically from both peer identities and the
group key, so both sides compute the same identifier independently.

## Topology tracking

Every group maintains a `Bonds` collection (accessible via
`Group::bonds()` or `When::bonds()`). This is an immutable set of all
currently active bonds in the group, including bonds between **other** peers
(learned via `BondFormed` messages).

This topology awareness lets each peer know the full mesh state, which is used
for:

- Dynamic quorum calculations in Raft.
- Determining when enough peers are connected to start elections.
- Providing the topology snapshot for state sync coordination.

## Connection handling

The `Acceptor` struct implements the `ProtocolHandler` trait for the
`/mosaik/groups/1` ALPN:

```text
Incoming connection
       │
       ▼
  Wrap in Link<BondMessage>
       │
       ▼
  ensure_known_peer()
       │
       ▼
  wait_for_handshake()
  · receive HandshakeStart
  · verify proof
  · send HandshakeEnd
       │
       ▼
  ensure_same_network()
       │
       ▼
  Hand off to Group worker
```

If any step fails, the connection is dropped cleanly. The group worker then
decides whether to accept the bond (it may already have one with that peer)
and spawns a `BondWorker` if accepted.
