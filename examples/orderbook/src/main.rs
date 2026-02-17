//! Orderbook example: a distributed order-matching engine on Mosaik.
//!
//! Demonstrates:
//! - Streams for typed order dissemination (Order -> matching group)
//! - A Raft group running an `OrderBook` state machine (price-time priority)
//! - Stream output of Fill events from the matching group
//!
//! Topology:
//!   Traders (stream producers) -> `OrderBook` Group (Raft RSM) -> Fill stream

#![allow(clippy::too_many_lines)]

mod matching;
mod types;

use {
	futures::{SinkExt, StreamExt},
	matching::{OrderBook, OrderBookCommand, OrderBookQuery, OrderBookQueryResult},
	mosaik::*,
	types::{Order, Side, TradingPair},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	tracing_subscriber::fmt()
		.with_env_filter(
			tracing_subscriber::EnvFilter::try_from_default_env()
				.unwrap_or_else(|_| "info,mosaik=debug".parse().unwrap()),
		)
		.init();

	let network_id = NetworkId::random();
	let group_key = GroupKey::random();
	let pair = TradingPair::new("ETH", "USDC");

	// --- Spin up 3 nodes that form the matching-engine Raft group ---
	let matcher0 = Network::new(network_id).await?;
	let matcher1 = Network::new(network_id).await?;
	let matcher2 = Network::new(network_id).await?;

	// Cross-discover all matchers
	discover_all([&matcher0, &matcher1, &matcher2]).await?;

	// Each matcher joins the same group with an OrderBook state machine
	let g0 = matcher0
		.groups()
		.with_key(group_key)
		.with_state_machine(OrderBook::new(pair.clone()))
		.join();

	let g1 = matcher1
		.groups()
		.with_key(group_key)
		.with_state_machine(OrderBook::new(pair.clone()))
		.join();

	let g2 = matcher2
		.groups()
		.with_key(group_key)
		.with_state_machine(OrderBook::new(pair.clone()))
		.join();

	// Wait for the group to elect a leader and come online
	tracing::info!("waiting for matching engine group to come online...");
	g0.when().online().await;
	g1.when().online().await;
	g2.when().online().await;

	let leader = g0
		.leader()
		.expect("leader should be elected after online");
	tracing::info!("matching engine online, leader: {leader}");

	// --- Spin up 2 trader nodes that submit orders via streams ---
	let trader_a = Network::new(network_id).await?;
	let trader_b = Network::new(network_id).await?;

	// Traders discover the matcher network
	discover_all([&trader_a, &trader_b, &matcher0, &matcher1, &matcher2])
		.await?;

	// Traders produce Order streams
	let mut orders_a = trader_a.streams().produce::<Order>();
	let mut orders_b = trader_b.streams().produce::<Order>();

	// Matcher0 (or whichever is leader) consumes the Order stream
	// and feeds orders into the Raft group
	let mut order_consumer = matcher0.streams().consume::<Order>();

	// Wait for consumer to subscribe to both trader producers
	order_consumer.when().subscribed().minimum_of(2).await;
	tracing::info!("matcher subscribed to both trader order streams");

	// Matcher0 produces Fill events for downstream consumers
	let mut fill_producer = matcher0.streams().produce::<types::Fill>();

	// Trader A consumes fills to see execution reports
	let mut fill_consumer = trader_a.streams().consume::<types::Fill>();
	fill_consumer.when().subscribed().await;
	tracing::info!("trader_a subscribed to fill stream");

	// --- Submit orders ---
	tracing::info!("submitting orders...");

	// Trader A posts asks (selling ETH)
	orders_a
		.send(Order {
			id: 1,
			pair: pair.clone(),
			side: Side::Ask,
			price: 300_000, // $3000.00
			quantity: 10,
			trader: "alice".into(),
		})
		.await?;

	orders_a
		.send(Order {
			id: 2,
			pair: pair.clone(),
			side: Side::Ask,
			price: 301_000, // $3010.00
			quantity: 5,
			trader: "alice".into(),
		})
		.await?;

	// Trader B posts bids (buying ETH)
	orders_b
		.send(Order {
			id: 3,
			pair: pair.clone(),
			side: Side::Bid,
			price: 299_000, // $2990.00 -- won't match
			quantity: 8,
			trader: "bob".into(),
		})
		.await?;

	orders_b
		.send(Order {
			id: 4,
			pair: pair.clone(),
			side: Side::Bid,
			price: 300_500, // $3005.00 -- crosses ask at $3000
			quantity: 15,
			trader: "bob".into(),
		})
		.await?;

	// --- Consume orders from the stream and feed them into the Raft group ---
	// In production this would be a long-running loop; here we process 4 orders.
	for _ in 0..4 {
		let order = order_consumer
			.next()
			.await
			.expect("expected order from stream");

		tracing::info!(
			"received order: {} {} {}@{} qty={}",
			order.trader,
			order.side,
			order.pair,
			order.price,
			order.quantity
		);

		g0.execute(OrderBookCommand::PlaceOrder(order)).await?;
	}

	// --- Query the state machine for fills ---
	let result = g0.query(OrderBookQuery::Fills, Consistency::Strong).await?;

	if let OrderBookQueryResult::Fills(fills) = &result {
		tracing::info!("{} fills produced:", fills.len());
		for fill in fills {
			tracing::info!("  {fill}");

			// Publish fill to the stream for downstream consumers
			fill_producer.send(fill.clone()).await?;
		}
	}

	// --- Query top of book ---
	let result = g0
		.query(
			OrderBookQuery::TopOfBook {
				pair: pair.clone(),
				depth: 5,
			},
			Consistency::Strong,
		)
		.await?;

	if let OrderBookQueryResult::TopOfBook { bids, asks } = &result {
		tracing::info!("top of book for {pair}:");
		for (price, qty) in asks.iter().rev() {
			tracing::info!("  ASK {price} x {qty}");
		}
		tracing::info!("  ---");
		for (price, qty) in bids {
			tracing::info!("  BID {price} x {qty}");
		}
	}

	// --- Verify fills replicated to followers ---
	g1.when().committed().reaches(g0.committed()).await;
	let follower_result =
		g1.query(OrderBookQuery::Fills, Consistency::Weak).await?;

	if let OrderBookQueryResult::Fills(fills) = &follower_result {
		tracing::info!(
			"follower g1 sees {} fills (consistent with leader)",
			fills.len()
		);
	}

	// Receive fills on trader_a's fill consumer
	if let Some(fill) = fill_consumer.next().await {
		tracing::info!("trader_a received fill notification: {fill}");
	}

	tracing::info!("orderbook example complete");
	Ok(())
}

/// Utility: cross-discover all networks with each other.
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
