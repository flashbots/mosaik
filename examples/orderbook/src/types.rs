use serde::{Deserialize, Serialize};

/// Trading pair identifier (e.g., "ETH/USDC").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TradingPair {
	pub base: String,
	pub quote: String,
}

impl TradingPair {
	pub fn new(base: &str, quote: &str) -> Self {
		Self {
			base: base.to_string(),
			quote: quote.to_string(),
		}
	}
}

impl core::fmt::Display for TradingPair {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(f, "{}/{}", self.base, self.quote)
	}
}

/// Order side: bid (buy) or ask (sell).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
	Bid,
	Ask,
}

impl core::fmt::Display for Side {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		match self {
			Self::Bid => write!(f, "BID"),
			Self::Ask => write!(f, "ASK"),
		}
	}
}

/// Price in basis points to avoid floating point. 1 unit = 0.01.
/// e.g., price 150000 = $1500.00
pub type Price = u64;

/// Quantity in smallest units of the base asset.
pub type Quantity = u64;

/// A limit order submitted to the orderbook.
/// This type auto-implements `Datum` for stream transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
	pub id: u64,
	pub pair: TradingPair,
	pub side: Side,
	pub price: Price,
	pub quantity: Quantity,
	pub trader: String,
}

/// A fill produced when two orders match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fill {
	pub bid_order_id: u64,
	pub ask_order_id: u64,
	pub pair: TradingPair,
	pub price: Price,
	pub quantity: Quantity,
}

impl core::fmt::Display for Fill {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(
			f,
			"Fill(bid={}, ask={}, {}@{}, qty={})",
			self.bid_order_id, self.ask_order_id, self.pair, self.price, self.quantity
		)
	}
}
