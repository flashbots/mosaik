//! This library defines unified data models used by Mosaik actors.

use {
	rblib::alloy::primitives::{Address, BlockHash, U256},
	serde::{Deserialize, Serialize},
	std::collections::BTreeMap,
};

pub type Nonce = u64;
pub type Balance = U256;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct NoncesUpdate {
	pub block: BlockHash,
	pub nonces: BTreeMap<Address, Nonce>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct BalancesUpdate {
	pub block: BlockHash,
	pub balances: BTreeMap<Address, Balance>,
}
