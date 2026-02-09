use {
	rblib::alloy::primitives::{
		Address,
		BlockHash,
		BlockNumber,
		U256,
		keccak256,
	},
	serde::{Deserialize, Serialize},
	std::collections::BTreeMap,
};

mod discovery;
mod groups;
// mod store;
mod streams;
mod utils;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Data1(pub String);

impl Data1 {
	#[allow(dead_code)]
	pub fn new(s: &str) -> Self {
		Self(s.to_string())
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Data2(pub String);

#[allow(dead_code)]
impl Data2 {
	pub fn new(s: &str) -> Self {
		Self(s.to_string())
	}
}

pub type Nonce = u64;

#[allow(dead_code)]
pub type Balance = U256;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoncesUpdate {
	pub block: (BlockNumber, BlockHash),
	pub nonces: BTreeMap<Address, Nonce>,
}

#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalancesUpdate {
	pub block: (BlockNumber, BlockHash),
	pub balances: BTreeMap<Address, Balance>,
}

impl NoncesUpdate {
	pub fn new(
		block: BlockNumber,
		nonces: impl IntoIterator<Item = (Address, Nonce)>,
	) -> Self {
		Self {
			block: (block, keccak256(block.to_be_bytes())),
			nonces: nonces.into_iter().collect(),
		}
	}
}
