use {
	crate::{
		GroupKey,
		groups::{GroupId, IntervalsConfig, StateMachine},
	},
	core::time::Duration,
	derive_builder::Builder,
};

/// Configuration options for the streams subsystem.
#[derive(Builder, Debug, Clone)]
#[builder(pattern = "owned", setter(prefix = "with"), derive(Debug, Clone))]
#[builder_struct_attr(doc(hidden))]
pub struct Config {
	/// The timeout duration for completing the handshake when establishing
	/// a new bond connection to a remote peer in the group.
	#[builder(default = "Duration::from_secs(2)")]
	pub handshake_timeout: Duration,
}

impl Config {
	/// Creates a new config builder with default values.
	pub fn builder() -> ConfigBuilder {
		ConfigBuilder::default()
	}
}

#[derive(Debug, Clone)]
pub struct GroupConfig {
	id: GroupId,
	key: GroupKey,
	intervals: IntervalsConfig,
	consensus_events_backlog: usize,
}

impl GroupConfig {
	pub fn new<M: StateMachine>(
		key: GroupKey,
		intervals: IntervalsConfig,
		consensus_events_backlog: usize,
	) -> Self {
		let id = key
			.secret()
			.hashed()
			.derive(intervals.digest())
			.derive(M::ID);

		Self {
			id,
			key,
			intervals,
			consensus_events_backlog,
		}
	}

	pub const fn group_id(&self) -> &GroupId {
		&self.id
	}

	pub const fn key(&self) -> &GroupKey {
		&self.key
	}

	pub const fn intervals(&self) -> &IntervalsConfig {
		&self.intervals
	}

	pub const fn consensus_events_backlog(&self) -> usize {
		self.consensus_events_backlog
	}
}
