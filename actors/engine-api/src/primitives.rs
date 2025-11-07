use rblib::{
	prelude::*,
	reth::{
		payload::PayloadId,
		providers::CanonStateNotification,
		rpc::types::engine::ForkchoiceState,
	},
};

pub struct PayloadJob<P: Platform> {
	pub attributes: types::PayloadBuilderAttributes<P>,
	pub chain_state: ForkchoiceState,
}

pub struct PayloadProposal<P: Platform> {
	pub job: PayloadId,
	pub payload: types::BuiltPayload<P>,
}

pub type ChainUpdate<P: Platform> =
	CanonStateNotification<types::Primitives<P>>;
