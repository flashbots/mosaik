# Self-Organization

One of mosaik's defining features is that nodes **self-organize** into the correct topology without manual configuration. This chapter explains how that works step by step.

## The Self-Organization Loop

When a new node joins the network, the following sequence happens automatically:

```text
1. Bootstrap       Node connects to a known bootstrap peer
                       │
2. Gossip          Announce protocol broadcasts presence
                       │
3. Catalog Sync    Full catalog exchange with bootstrap peer
                       │
4. Discovery       Node learns about all other peers, their
                   tags, streams, and groups
                       │
5. Streams         Consumer discovers matching producers,
                   opens subscriptions automatically
                       │
6. Groups          Node finds peers with matching group keys,
                   forms bonds, joins Raft cluster
                       │
7. Convergence     Network reaches a stable topology where
                   all nodes are connected to the right peers
```

## Step 1: Bootstrap & Gossip

A new node starts with at least one bootstrap peer address. It connects and begins participating in the gossip protocol:

```rust,ignore
let network = Network::builder(network_id)
    .with_discovery(
        discovery::Config::builder()
            .with_bootstrap(bootstrap_addr)
            .with_tags("matcher")
    )
    .build()
    .await?;
```

The node immediately:
- **Announces** itself via the gossip protocol (`/mosaik/announce`), broadcasting its `PeerEntry` (identity, tags, streams, groups)
- **Receives** announcements from other peers
- **Triggers** a full catalog sync (`/mosaik/catalog-sync`) with the bootstrap peer to catch up on all known peers

## Step 2: Catalog Convergence

The catalog converges through two complementary protocols:

### Real-time: Gossip Announcements

Every node periodically re-announces its `PeerEntry` via iroh-gossip. The announce interval is configurable (default: 15 seconds) with jitter to avoid thundering herds:

```text
announce_interval = 15s
announce_jitter = 0.5  →  actual interval: 7.5s – 22.5s
```

When a node changes (adds a tag, creates a stream, joins a group), it re-announces immediately.

### Catch-up: Full Catalog Sync

When a new node connects, it performs a bidirectional catalog sync with its peer. Both nodes exchange their complete catalogs, and entries are merged:

```text
Node A                          Node B
  │                               │
  │── CatalogSyncRequest ───────► │
  │                               │
  │◄── CatalogSyncResponse ──────│
  │                               │
  │  (both merge received         │
  │   entries into local catalog) │
```

### Signed Entries

Each `PeerEntry` is cryptographically signed by its owner. This proves authenticity — you can trust that a peer entry's tags, streams, and groups are genuine, even when received via gossip through intermediaries.

### Staleness & Purging

Entries that haven't been updated within the `purge_after` duration (default: 300 seconds) are considered stale and hidden from the public catalog API. This ensures departed nodes are eventually removed.

## Step 3: Automatic Stream Connections

Once discovery populates the catalog, the Streams subsystem automatically connects producers and consumers:

```text
1. Node A creates Producer<Order>
   → Discovery advertises: "I produce stream 'Order'"

2. Node B creates Consumer<Order>
   → Discovery observes: "Node A produces 'Order'"
   → Consumer worker opens subscription to Node A

3. Data flows: Node A ──[Order]──► Node B
```

This is fully automatic. The consumer's background worker monitors the catalog for matching producers and establishes connections as they appear.

Filtering can restrict which connections form:

```rust,ignore
// Producer only accepts nodes tagged "authorized"
let producer = network.streams().producer::<Order>()
    .accept_if(|peer| peer.tags.contains(&"authorized".into()))
    .build()?;

// Consumer only subscribes to nodes tagged "primary"
let consumer = network.streams().consumer::<Order>()
    .subscribe_if(|peer| peer.tags.contains(&"primary".into()))
    .build();
```

## Step 4: Automatic Group Formation

Groups form through a similar discovery-driven process:

```text
1. Node A joins group with key K
   → Discovery advertises: "I'm in group G" (where G = hash(K, config, ...))

2. Node B joins group with same key K
   → Discovery observes: "Node A is in group G"
   → Bond worker opens connection to Node A

3. Mutual handshake proves both know key K
   → Bond established

4. Raft consensus begins
   → Leader elected among bonded peers
   → Commands can be replicated
```

The bond handshake uses HMAC over the TLS session secrets combined with the group key. This proves knowledge of the group secret without transmitting it.

## Step 5: Late Joiners

A node joining an already-running network catches up automatically:

### Stream Catch-up
Consumers connecting to an active producer receive data from the point of subscription. There's no historical replay — streams are real-time.

### Group Catch-up
A node joining an existing Raft group:
1. Forms bonds with existing members
2. Receives the current log from peers (distributed across multiple peers for efficiency)
3. Applies all log entries to bring its state machine up to date
4. Begins participating normally (can vote, can become leader)

For collections, catch-up can use either log replay or snapshot sync:
- **Log replay** (default): Replays the entire log from the beginning
- **Snapshot sync**: Transfers a point-in-time snapshot of the state, avoiding log replay for large states

## Topology Example

Consider a distributed trading system with three roles:

```text
Tags: "trader"          Tags: "matcher"         Tags: "reporter"
┌──────────┐            ┌──────────┐            ┌──────────┐
│  Node 1  │            │  Node 3  │            │  Node 5  │
│ Producer │──[Order]──►│ Consumer │            │ Consumer │
│ <Order>  │            │ <Order>  │            │  <Fill>  │
└──────────┘            │          │            └──────────┘
                        │ Group:   │──[Fill]──►      ▲
┌──────────┐            │ OrderBook│            ┌──────────┐
│  Node 2  │            │ (Raft)   │            │  Node 6  │
│ Producer │──[Order]──►│          │            │ Consumer │
│ <Order>  │            │ Producer │──[Fill]──►│  <Fill>  │
└──────────┘            │  <Fill>  │            └──────────┘
                        └──────────┘
                        ┌──────────┐
                        │  Node 4  │
                        │ (matcher │
                        │  replica)│
                        └──────────┘
```

All of this topology forms **automatically** from:
- Node 1–2: `with_tags("trader")`, creates `Producer<Order>`
- Node 3–4: `with_tags("matcher")`, creates `Consumer<Order>`, joins orderbook group, creates `Producer<Fill>`
- Node 5–6: `with_tags("reporter")`, creates `Consumer<Fill>`

No node needs to know the addresses of any other node except one bootstrap peer.
