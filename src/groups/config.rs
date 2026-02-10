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

/// Full set of configuration options a group instances.
///
/// This set of values is used to derive the group id and must be identical
/// across all members of the group, to ensure that they all run the same
/// consensus parameters.
#[derive(Debug, Clone)]
pub struct GroupConfig {
	id: GroupId,
	key: GroupKey,
	intervals: IntervalsConfig,
}

impl GroupConfig {
	pub fn new<M: StateMachine>(
		key: GroupKey,
		intervals: IntervalsConfig,
	) -> Self {
		let id = key
			.secret()
			.hashed()
			.derive(intervals.digest())
			.derive(M::ID);

		Self { id, key, intervals }
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
}
