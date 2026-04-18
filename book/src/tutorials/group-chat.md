# Building a P2P Group Chat

This tutorial walks through the [group-chat example](https://github.com/flashbots/mosaik/blob/main/examples/group-chat.rs) — a fully decentralized chat room where participants discover each other automatically and share state via replicated collections.

## What Are We Building?

A peer-to-peer chat application where:
- Each running instance is one participant
- Everyone in the same "room" sees the same messages and participant list
- No central server — peers find each other via mosaik's gossip and DHT
- Message history and user presence are replicated across all nodes

```text
   ┌──────────────┐    ┌──────────────┐    ┌──────────────┐
   │   Alice      │    │   Bob        │    │   Charlie    │
   │              │    │              │    │              │
   │  Map<Users>  │◄──►│  Map<Users>  │◄──►│  Map<Users>  │
   │  Vec<Msgs>   │◄──►│  Vec<Msgs>   │◄──►│  Vec<Msgs>   │
   └──────────────┘    └──────────────┘    └──────────────┘
          ▲                    ▲                    ▲
          └────────────────────┴────────────────────┘
                    gossip + DHT discovery
```

Every participant holds a full replica of two shared collections:
1. **`Map<String, UserInfo>`** — who is currently in the room
2. **`Vec<Message>`** — the chat history in append order

Under the hood, mosaik replicates both collections through Raft consensus, so all nodes converge on the same state automatically.

## Project Setup

The group-chat example is a single file at `examples/group-chat.rs`. It uses these dependencies:

```toml
[dependencies]
mosaik = "0.3"
tokio = { version = "1", features = ["full"] }
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
anyhow = "1"
```

## Step 1: Define Shared Types

First, define the data that will be replicated. Because these types derive `Serialize + Deserialize`, they automatically implement `Datum` and work with mosaik collections:

```rust,ignore
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
struct UserInfo {
    nickname: String,
    color: u8,
}

#[derive(Clone, Serialize, Deserialize)]
struct Message {
    from: String,
    color: u8,
    text: String,
}
```

## Step 2: Declare Stable Identifiers

Every mosaik collection needs a `StoreId` so that all participants reference the same replicated state. The `unique_id!()` macro derives a deterministic ID from a string at compile time:

```rust,ignore
use mosaik::{NetworkId, StoreId, unique_id};

const NETWORK_SALT: NetworkId =
    unique_id!("mosaik.example.group-chat");
const USERS: StoreId =
    unique_id!("mosaik.example.group-chat.users");
const MESSAGES: StoreId =
    unique_id!("mosaik.example.group-chat.messages");
```

- **`NETWORK_SALT`** — a base `NetworkId` that gets further derived per room (so each room is its own isolated network)
- **`USERS`** / **`MESSAGES`** — stable `StoreId`s shared by all participants

All nodes that use the same `StoreId` automatically join the same underlying Raft group and replicate the same data.

## Step 3: Create the Network

Each room maps to its own mosaik network. The room name is combined with the base salt to produce a unique `NetworkId`:

```rust,ignore
let network = Network::builder(
        NETWORK_SALT.derive(args.room.as_str())
    )
    .with_discovery(
        discovery::Config::builder()
            .with_bootstrap(args.peer)
    )
    .build()
    .await?;

println!("local peer id: {}", network.local().id());
```

Key points:
- **`NETWORK_SALT.derive(room)`** — deterministically derives a unique network ID per room name. Everyone passing `--room=lobby` ends up on the same network.
- **`with_bootstrap(args.peer)`** — optionally seeds discovery with a known peer ID. Without it, nodes still find each other via DHT, but it may take a few seconds longer.

## Step 4: Open Replicated Collections

With the network running, open writer handles to both collections:

```rust,ignore
let users =
    collections::Map::<String, UserInfo>::writer(&network, USERS);
let messages =
    collections::Vec::<Message>::writer(&network, MESSAGES);

// Block until we've joined both collection groups
// and caught up with peers.
users.when().online().await;
messages.when().online().await;
```

- **`Map::writer()`** / **`Vec::writer()`** — creates a read-write handle to the replicated collection. Every node opens a writer because every participant can both read and write.
- **`when().online().await`** — a reactive condition that resolves once the node has joined the underlying Raft group and synced any existing state from peers. After this returns, reads reflect the current replicated state.

## Step 5: Register and Display State

After going online, insert ourselves into the user map and print any existing participants and message history:

```rust,ignore
// Register ourselves
users
    .insert(
        network.local().id().to_string(),
        UserInfo {
            nickname: args.nickname.clone(),
            color: args.color,
        },
    )
    .await
    .ok();

// Print current participants
let initial_users: HashMap<String, UserInfo> =
    users.iter().collect();
println!("--- {} participant(s) ---", initial_users.len());
for u in initial_users.values() {
    println!("  {}", u.nickname);
}

// Print message history
let history: Vec<_> = messages.iter().collect();
if !history.is_empty() {
    println!(
        "--- {} message(s) in history ---",
        history.len()
    );
    for m in &history {
        println!("{}: {}", m.from, m.text);
    }
}
```

Because the collections are replicated, a node that joins late automatically receives the full history — there's no separate "catch-up" protocol to implement.

## Step 6: The Main Loop

The core of the chat is a `tokio::select!` loop that handles three events: user input, shutdown, and periodic state polling:

```rust,ignore
let (tx, mut rx) = mpsc::unbounded_channel::<String>();
std::thread::spawn(move || {
    for line in std::io::stdin().lock().lines()
        .map_while(Result::ok)
    {
        if tx.send(line).is_err() { break; }
    }
});

let mut displayed = history.len();
let mut known_users = initial_users;
let my_id = network.local().id().to_string();

loop {
    tokio::select! {
        Some(text) = rx.recv() => {
            // User typed a message — append it
            let m = Message {
                from: args.nickname.clone(),
                color: args.color,
                text,
            };
            messages.push_back(m).await.ok();
        }
        _ = tokio::signal::ctrl_c() => {
            // Clean shutdown — remove ourselves
            timeout(
                Duration::from_secs(5),
                users.remove(&my_id),
            ).await.ok();
            return Ok(());
        }
        () = tokio::time::sleep(
            Duration::from_millis(100)
        ) => {}
    }

    // ... detect joins/leaves and print new messages
}
```

Stdin is read in a dedicated OS thread (it's blocking I/O) and fed into the async loop via an `mpsc` channel. On Ctrl-C, the node removes itself from the user map before exiting so that others see a clean "left" notification.

## Step 7: Detect Joins and Leaves

On every tick, the loop diffs the current user map against its last-known snapshot to detect topology changes:

```rust,ignore
let current_users: HashMap<String, UserInfo> =
    users.iter().collect();

for (id, info) in &current_users {
    if id != &my_id && !known_users.contains_key(id) {
        println!("* {} joined", info.nickname);
    }
}
for (id, info) in &known_users {
    if id != &my_id && !current_users.contains_key(id) {
        println!("* {} left", info.nickname);
    }
}
known_users = current_users;
```

This works because the replicated `Map` converges across all nodes — when a new participant inserts themselves, every other node's local replica eventually reflects the change.

## Step 8: Handle Raft Reconciliation

When two nodes start at nearly the same time, each may briefly form its own single-node Raft group. When they discover each other, Raft reconciles the logs — the "losing" node's log is discarded. This means a node's own user-map insertion might vanish:

```rust,ignore
// Re-register ourselves if our entry was lost
if !known_users.contains_key(&my_id) {
    users
        .insert(my_id.clone(), UserInfo {
            nickname: args.nickname.clone(),
            color: args.color,
        })
        .await
        .ok();
}
```

This is a practical pattern for any mosaik application: after Raft reconciliation, check that your local state is still present and re-apply if necessary.

## Running It

Start multiple instances in separate terminals:

```bash
# Terminal 1
cargo run --example group-chat -- --nickname Alice

# Terminal 2
cargo run --example group-chat -- --nickname Bob --color 33

# Terminal 3
cargo run --example group-chat -- --nickname Charlie --color 34
```

Nodes discover each other automatically via DHT. To speed up initial discovery, pass a peer ID:

```bash
# Terminal 1 prints: local peer id: <alice-peer-id>

# Terminal 2 — connect directly to Alice
cargo run --example group-chat -- \
    --nickname Bob --color 33 \
    --peer <alice-peer-id>
```

You can also use `--room` to create separate chat rooms:

```bash
cargo run --example group-chat -- --nickname Alice --room dev
cargo run --example group-chat -- --nickname Bob --room dev
```

Expected output (Alice's terminal):

```text
Joining room 'lobby' as Alice ...
local peer id: 7bqnlah...
--- 1 participant(s) ---
  Alice
--- start typing, Ctrl-C to quit ---

* Bob joined
* Charlie joined
Bob: hey everyone!
Charlie: what's up?
```

## Key Takeaways

1. **Collections as shared state** — `Map` and `Vec` give you replicated data structures with no custom state machine needed. Insert, remove, and iterate — replication happens automatically.
2. **`unique_id!()` for stable addressing** — compile-time deterministic IDs ensure all participants reference the same collections without coordination.
3. **`NetworkId::derive()` for namespacing** — deriving a network ID from a room name creates isolated networks per room with a single line of code.
4. **`when().online().await`** — reactive conditions let you wait for the system to be ready before proceeding, without polling or timeouts.
5. **Raft reconciliation awareness** — when nodes start concurrently, log reconciliation can discard local writes. Re-checking and re-inserting is the idiomatic pattern.
6. **Zero-server architecture** — gossip and DHT handle all discovery. No central server, no signaling service, no message broker.
