use rblib::alloy::primitives::{Address, BlockHash, BlockNumber};

#[derive(Debug, Clone)]
pub struct SignerNonces {
	pub block_number: BlockNumber,
	pub block_hash: BlockHash,
	pub updates: Vec<(Address, u64)>,
}
