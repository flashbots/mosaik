use {
	crate::store::Store,
	mosaik::prelude::*,
	rblib::{
		alloy::primitives::{Address, BlockHash, U256},
		prelude::*,
	},
	serde::{Deserialize, Serialize},
	std::{collections::BTreeMap, sync::Arc},
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

pub struct StateSync<P: Platform> {
	store: Arc<Store>,
	nonces: Consumer<NoncesUpdate>,
	balances: Consumer<BalancesUpdate>,
	headers: Consumer<types::Header<P>>,
}

impl<P: Platform> StateSync<P> {
	pub fn new(network: &Network, store: Arc<Store>) -> Self {
		Self {
			store,
			nonces: network.consume::<NoncesUpdate>(),
			balances: network.consume::<BalancesUpdate>(),
			headers: network.consume::<types::Header<P>>(),
		}
	}
}
