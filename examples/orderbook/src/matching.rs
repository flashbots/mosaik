use {
	crate::types::{Fill, Order, Price, Quantity, Side, TradingPair},
	mosaik::{
		groups::{LogReplaySync, StateMachine},
		primitives::UniqueId,
	},
	serde::{Deserialize, Serialize},
	std::collections::BTreeMap,
};

/// Commands that mutate the orderbook state machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderBookCommand {
	/// Submit a new limit order.
	PlaceOrder(Order),
	/// Cancel an existing order by ID.
	CancelOrder(u64),
}

/// Queries against the orderbook state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderBookQuery {
	/// Get the top N levels of the book for a pair.
	TopOfBook { pair: TradingPair, depth: usize },
	/// Get all fills produced so far.
	Fills,
	/// Get the total number of resting orders.
	OrderCount,
}

/// Results returned by orderbook queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderBookQueryResult {
	TopOfBook {
		bids: Vec<(Price, Quantity)>,
		asks: Vec<(Price, Quantity)>,
	},
	Fills(Vec<Fill>),
	OrderCount(usize),
}

/// A price-time priority orderbook replicated via Raft consensus.
///
/// Bids are stored in descending price order (highest first).
/// Asks are stored in ascending price order (lowest first).
/// Orders at the same price level are matched FIFO.
#[derive(Debug)]
pub struct OrderBook {
	bids: BTreeMap<Price, Vec<(u64, Quantity, String)>>,
	asks: BTreeMap<Price, Vec<(u64, Quantity, String)>>,
	/// All fills produced by matching.
	fills: Vec<Fill>,
	/// The trading pair this book handles.
	pair: TradingPair,
}

impl OrderBook {
	pub const fn new(pair: TradingPair) -> Self {
		Self {
			bids: BTreeMap::new(),
			asks: BTreeMap::new(),
			fills: Vec::new(),
			pair,
		}
	}

	/// Match an incoming order against the resting book.
	/// Returns any unfilled remainder quantity.
	fn match_order(&mut self, order: &Order) -> Quantity {
		let mut remaining = order.quantity;

		match order.side {
			Side::Bid => {
				// A bid matches against asks at or below the bid price.
				// Iterate asks from lowest to highest.
				while remaining > 0 {
					let Some((&ask_price, _)) = self.asks.first_key_value() else {
						break;
					};
					if ask_price > order.price {
						break;
					}

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
						if *ask_qty == 0 {
							ask_level.remove(0);
						}
					}

					if ask_level.is_empty() {
						self.asks.remove(&ask_price);
					}
				}
			}
			Side::Ask => {
				// An ask matches against bids at or above the ask price.
				// Iterate bids from highest to lowest.
				while remaining > 0 {
					let Some((&bid_price, _)) = self.bids.last_key_value() else {
						break;
					};
					if bid_price < order.price {
						break;
					}

					let bid_level = self.bids.get_mut(&bid_price).unwrap();
					while remaining > 0 && !bid_level.is_empty() {
						let (bid_id, bid_qty, _) = &mut bid_level[0];
						let fill_qty = remaining.min(*bid_qty);

						self.fills.push(Fill {
							bid_order_id: *bid_id,
							ask_order_id: order.id,
							pair: self.pair.clone(),
							price: bid_price,
							quantity: fill_qty,
						});

						remaining -= fill_qty;
						*bid_qty -= fill_qty;
						if *bid_qty == 0 {
							bid_level.remove(0);
						}
					}

					if bid_level.is_empty() {
						self.bids.remove(&bid_price);
					}
				}
			}
		}

		remaining
	}

	fn place_order(&mut self, order: Order) {
		let remaining = self.match_order(&order);

		// If there's unfilled quantity, add it as a resting order
		if remaining > 0 {
			let book = match order.side {
				Side::Bid => &mut self.bids,
				Side::Ask => &mut self.asks,
			};
			book.entry(order.price).or_default().push((
				order.id,
				remaining,
				order.trader,
			));
		}
	}

	fn cancel_order(&mut self, order_id: u64) {
		for (_, level) in self.bids.iter_mut().chain(self.asks.iter_mut()) {
			level.retain(|(id, _, _)| *id != order_id);
		}
		// Clean up empty price levels
		self.bids.retain(|_, level| !level.is_empty());
		self.asks.retain(|_, level| !level.is_empty());
	}
}

impl StateMachine for OrderBook {
	type Command = OrderBookCommand;
	type Query = OrderBookQuery;
	type QueryResult = OrderBookQueryResult;
	type StateSync = LogReplaySync<Self>;

	fn signature(&self) -> UniqueId {
		UniqueId::from("orderbook_state_machine")
	}

	fn apply(&mut self, command: Self::Command) {
		match command {
			OrderBookCommand::PlaceOrder(order) => self.place_order(order),
			OrderBookCommand::CancelOrder(id) => self.cancel_order(id),
		}
	}

	fn query(&self, query: Self::Query) -> Self::QueryResult {
		match query {
			OrderBookQuery::TopOfBook { pair: _, depth } => {
				// Bids: highest prices first
				let bids: Vec<(Price, Quantity)> = self
					.bids
					.iter()
					.rev()
					.take(depth)
					.map(|(&price, orders)| {
						let total_qty: Quantity =
							orders.iter().map(|(_, qty, _)| qty).sum();
						(price, total_qty)
					})
					.collect();

				// Asks: lowest prices first
				let asks: Vec<(Price, Quantity)> = self
					.asks
					.iter()
					.take(depth)
					.map(|(&price, orders)| {
						let total_qty: Quantity =
							orders.iter().map(|(_, qty, _)| qty).sum();
						(price, total_qty)
					})
					.collect();

				OrderBookQueryResult::TopOfBook { bids, asks }
			}
			OrderBookQuery::Fills => OrderBookQueryResult::Fills(self.fills.clone()),
			OrderBookQuery::OrderCount => {
				let count: usize = self
					.bids
					.values()
					.chain(self.asks.values())
					.map(Vec::len)
					.sum();
				OrderBookQueryResult::OrderCount(count)
			}
		}
	}

	fn state_sync(&self) -> Self::StateSync {
		LogReplaySync::default()
	}
}
