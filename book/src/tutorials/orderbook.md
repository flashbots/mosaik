# Building a Distributed Orderbook

This tutorial walks through the [orderbook example](https://github.com/flashbots/mosaik/tree/main/examples/orderbook) — a distributed order-matching engine that combines Streams, Groups, and Raft consensus.

## Architecture Overview

The orderbook example demonstrates how multiple mosaik subsystems compose to build a realistic distributed system:

```text
         Traders                  Matchers                 Observers
    ┌────────────┐          ┌──────────────────┐      ┌────────────┐
    │  Trader A  │          │    Matcher 0     │      │  Observer  │
    │  Producer  │─[Order]─►│    Consumer      │      │  Consumer  │
    │  <Order>   │          │    <Order>       │      │  <Fill>    │
    └────────────┘          │                  │      └────────────┘
                            │  ┌────────────┐  │           ▲
    ┌────────────┐          │  │  OrderBook │  │           │
    │  Trader B  │          │  │  (Raft SM) │  │──[Fill]───┘
    │  Producer  │─[Order]─►│  │            │  │
    │  <Order>   │          │  └────────────┘  │
    └────────────┘          │    Producer      │
                            │    <Fill>        │
                            └──────────────────┘
                            ┌──────────────────┐
                            │    Matcher 1     │
                            │    (replica)     │
                            └──────────────────┘
                            ┌──────────────────┐
                            │    Matcher 2     │
                            │    (replica)     │
                            └──────────────────┘
```

1. **Traders** produce `Order` objects via Streams
2. **Matchers** (a 3-node Raft group) consume orders, replicate them through consensus, and run a price-time priority matching engine
3. Fill events are published back as a stream for downstream consumers

## Step 1: Define the Domain Types

First, define the types that flow through the system. These types auto-implement the `Datum` trait for streaming:

```rust,ignore
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TradingPair {
    pub base: String,
    pub quote: String,
}

impl TradingPair {
    pub fn new(base: &str, quote: &str) -> Self {
        Self { base: base.to_string(), quote: quote.to_string() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side { Bid, Ask }

/// Price in basis points (1 unit = 0.01). e.g., 300_000 = $3000.00
pub type Price = u64;
pub type Quantity = u64;

/// A limit order — streamable via mosaik Streams
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: u64,
    pub pair: TradingPair,
    pub side: Side,
    pub price: Price,
    pub quantity: Quantity,
    pub trader: String,
}

/// A fill produced when orders match
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fill {
    pub bid_order_id: u64,
    pub ask_order_id: u64,
    pub pair: TradingPair,
    pub price: Price,
    pub quantity: Quantity,
}
```

Because `Order` and `Fill` derive `Serialize + Deserialize`, they automatically implement `Datum` and can be used with mosaik Streams.

## Step 2: Implement the State Machine

The heart of the example is the `OrderBook` state machine. It implements the `StateMachine` trait so it can be replicated across all group members via Raft:

```rust,ignore
use mosaik::groups::{ApplyContext, LogReplaySync, StateMachine};
use mosaik::primitives::UniqueId;
use std::collections::BTreeMap;

/// Commands that mutate the orderbook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderBookCommand {
    PlaceOrder(Order),
    CancelOrder(u64),
}

/// Read-only queries against the orderbook state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderBookQuery {
    TopOfBook { pair: TradingPair, depth: usize },
    Fills,
    OrderCount,
}

/// Query results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderBookQueryResult {
    TopOfBook { bids: Vec<(Price, Quantity)>, asks: Vec<(Price, Quantity)> },
    Fills(Vec<Fill>),
    OrderCount(usize),
}
```

The `StateMachine` trait requires four associated types:

- **`Command`** — mutations that get replicated through Raft (must be serializable)
- **`Query`** — read requests (not replicated)
- **`QueryResult`** — responses to queries
- **`StateSync`** — mechanism for catching up new nodes

And three core methods:

```rust,ignore
impl StateMachine for OrderBook {
    type Command = OrderBookCommand;
    type Query = OrderBookQuery;
    type QueryResult = OrderBookQueryResult;
    type StateSync = LogReplaySync<Self>;

    fn signature(&self) -> UniqueId {
        // Unique identifier for this state machine type.
        // Contributes to GroupId derivation — different state machines
        // produce different GroupIds even with the same key.
        UniqueId::from("orderbook_state_machine")
    }

    fn apply(&mut self, command: Self::Command, _ctx: &dyn ApplyContext) {
        match command {
            OrderBookCommand::PlaceOrder(order) => self.place_order(order),
            OrderBookCommand::CancelOrder(id) => self.cancel_order(id),
        }
    }

    fn query(&self, query: Self::Query) -> Self::QueryResult {
        match query {
            OrderBookQuery::TopOfBook { pair: _, depth } => {
                // Return top N price levels
                // ...
            }
            OrderBookQuery::Fills => {
                OrderBookQueryResult::Fills(self.fills.clone())
            }
            OrderBookQuery::OrderCount => {
                let count = self.bids.values()
                    .chain(self.asks.values())
                    .map(Vec::len).sum();
                OrderBookQueryResult::OrderCount(count)
            }
        }
    }

    fn state_sync(&self) -> Self::StateSync {
        LogReplaySync::default()
    }
}
```

Key points:
- **`apply()`** is called on every node in the same order — this is what Raft guarantees. The matching logic must be deterministic.
- **`query()`** reads local state without going through Raft. With `Consistency::Strong`, the query is forwarded to the leader.
- **`LogReplaySync::default()`** means new nodes catch up by replaying the entire command log from the beginning.

## Step 3: The Matching Engine

The `OrderBook` implements price-time priority matching. When a new order arrives, it crosses against the opposite side:

```rust,ignore
impl OrderBook {
    fn match_order(&mut self, order: &Order) -> Quantity {
        let mut remaining = order.quantity;

        match order.side {
            Side::Bid => {
                // Bids match against asks at or below the bid price
                while remaining > 0 {
                    let Some((&ask_price, _)) = self.asks.first_key_value()
                        else { break };
                    if ask_price > order.price { break; }

                    let ask_level = self.asks.get_mut(&ask_price).unwrap();
                    while remaining > 0 && !ask_level.is_empty() {
                        let (ask_id, ask_qty, _) = &mut ask_level[0];
                        let fill_qty = remaining.min(*ask_qty);

                        self.fills.push(Fill {
                            bid_order_id: order.id,
                            ask_order_id: *ask_id,
                            pair: self.pair.clone(),
                            price: ask_price,
                            quantity: fill_qty,
                        });

                        remaining -= fill_qty;
                        *ask_qty -= fill_qty;
                        if *ask_qty == 0 { ask_level.remove(0); }
                    }
                    if ask_level.is_empty() {
                        self.asks.remove(&ask_price);
                    }
                }
            }
            Side::Ask => {
                // Asks match against bids at or above the ask price
                // (mirror logic, iterating bids from highest)
                // ...
            }
        }
        remaining
    }

    fn place_order(&mut self, order: Order) {
        let remaining = self.match_order(&order);
        // Any unfilled quantity rests on the book
        if remaining > 0 {
            let book = match order.side {
                Side::Bid => &mut self.bids,
                Side::Ask => &mut self.asks,
            };
            book.entry(order.price)
                .or_default()
                .push((order.id, remaining, order.trader));
        }
    }
}
```

Because `apply()` is called in the same order on every replica (guaranteed by Raft), the matching results are identical across all nodes.

## Step 4: Wire It All Together

The `main` function creates the network, forms the Raft group, and connects streams:

```rust,ignore
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let network_id = NetworkId::random();
    let group_key = GroupKey::random();
    let pair = TradingPair::new("ETH", "USDC");

    // --- 3 matcher nodes forming a Raft group ---
    let matcher0 = Network::new(network_id).await?;
    let matcher1 = Network::new(network_id).await?;
    let matcher2 = Network::new(network_id).await?;

    // Cross-discover all matchers
    discover_all([&matcher0, &matcher1, &matcher2]).await?;

    // Each joins the same group with an OrderBook state machine
    let g0 = matcher0.groups().with_key(group_key)
        .with_state_machine(OrderBook::new(pair.clone()))
        .join();
    let g1 = matcher1.groups().with_key(group_key)
        .with_state_machine(OrderBook::new(pair.clone()))
        .join();
    let g2 = matcher2.groups().with_key(group_key)
        .with_state_machine(OrderBook::new(pair.clone()))
        .join();

    // Wait for consensus to elect a leader
    g0.when().online().await;
    g1.when().online().await;
    g2.when().online().await;
```

### Connecting Traders via Streams

```rust,ignore
    // --- 2 trader nodes producing orders ---
    let trader_a = Network::new(network_id).await?;
    let trader_b = Network::new(network_id).await?;
    discover_all([&trader_a, &trader_b, &matcher0, &matcher1, &matcher2]).await?;

    // Traders produce Order streams
    let mut orders_a = trader_a.streams().produce::<Order>();
    let mut orders_b = trader_b.streams().produce::<Order>();

    // Matcher consumes orders
    let mut order_consumer = matcher0.streams().consume::<Order>();
    order_consumer.when().subscribed().minimum_of(2).await;
```

### Submitting and Matching Orders

```rust,ignore
    // Trader A: asks (selling ETH)
    orders_a.send(Order {
        id: 1, pair: pair.clone(), side: Side::Ask,
        price: 300_000, quantity: 10, trader: "alice".into(),
    }).await?;

    // Trader B: bids (buying ETH)
    orders_b.send(Order {
        id: 4, pair: pair.clone(), side: Side::Bid,
        price: 300_500, quantity: 15, trader: "bob".into(),
    }).await?;

    // Consume orders from stream and execute through Raft
    for _ in 0..4 {
        let order = order_consumer.next().await
            .expect("expected order from stream");
        g0.execute(OrderBookCommand::PlaceOrder(order)).await?;
    }
```

### Querying Results

```rust,ignore
    // Query fills (strong consistency — goes through leader)
    let result = g0.query(OrderBookQuery::Fills, Consistency::Strong).await?;
    if let OrderBookQueryResult::Fills(fills) = result.result() {
        for fill in fills {
            println!("{fill}");
        }
    }

    // Verify replication to followers
    g1.when().committed().reaches(g0.committed()).await;
    let follower_result = g1.query(
        OrderBookQuery::Fills, Consistency::Weak
    ).await?;
```

Key patterns demonstrated:

- **`execute()`** — sends a command through Raft and waits for it to be committed on a quorum
- **`query(..., Strong)`** — reads through the leader for linearizable results
- **`query(..., Weak)`** — reads local state (faster, but may be stale)
- **`when().committed().reaches(n)`** — waits for a follower to catch up to a specific commit index

## The Helper: Cross-Discovery

The example includes a utility to fully connect all nodes:

```rust,ignore
async fn discover_all(
    networks: impl IntoIterator<Item = &Network>,
) -> anyhow::Result<()> {
    let networks = networks.into_iter().collect::<Vec<_>>();
    for (i, net_i) in networks.iter().enumerate() {
        for (j, net_j) in networks.iter().enumerate() {
            if i != j {
                net_i.discovery().sync_with(net_j.local().addr()).await?;
            }
        }
    }
    Ok(())
}
```

In production, this pairwise sync is unnecessary — a single bootstrap peer handles discovery via gossip. This utility is for testing where all nodes start simultaneously.

## Running the Example

```bash
cd examples/orderbook
cargo run
```

Expected output:
```text
waiting for matching engine group to come online...
matching engine online, leader: <peer-id>
matcher subscribed to both trader order streams
submitting orders...
received order: alice ASK ETH/USDC@300000 qty=10
received order: alice ASK ETH/USDC@301000 qty=5
received order: bob BID ETH/USDC@299000 qty=8
received order: bob BID ETH/USDC@300500 qty=15
1 fills produced:
  Fill(bid=4, ask=1, ETH/USDC@300000, qty=10)
follower g1 sees 1 fills (consistent with leader)
orderbook example complete
```

## Key Takeaways

1. **Streams for data ingestion** — orders flow from traders to the matching engine via typed pub/sub
2. **Raft for consensus** — the `OrderBook` state machine runs on all replicas with identical results because Raft guarantees the same command order
3. **`execute()` for writes, `query()` for reads** — clean separation between mutations (replicated) and reads (local or forwarded)
4. **Automatic catch-up** — new replicas replay the command log to reach the current state
5. **Composability** — Streams + Groups + StateMachine combine naturally for real distributed applications
