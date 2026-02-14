use {
	crate::{
		Digest,
		GroupKey,
		Groups,
		builtin::groups::{InMemory, NoOp},
		groups::{
			Group,
			StateMachine,
			Storage,
			config::GroupConfig,
			worker::Worker,
		},
	},
	core::time::Duration,
	dashmap::Entry,
	derive_builder::Builder,
};

/// Configures the behavior of a [`Group`] that is being joined.
pub struct GroupBuilder<'g, S = (), M = ()> {
	/// Reference to the parent [`Groups`] instance that tracks all existing
	/// groups.
	groups: &'g Groups,

	/// The group key that carries the authentication credentials for joining the
	/// group and authorizing membership. This value is used when deriving the
	/// group id.
	pub(super) key: GroupKey,

	/// Configures various timing intervals for the group protocol, such as
	/// heartbeat intervals, election timeouts, and consensus tick durations.
	/// Those values are used when deriving the group id, all members of the
	/// group must have identical configuration settings for these values, any
	/// difference will render a different group id.
	pub(super) intervals: IntervalsConfig,

	/// The storage implementation to use for this group. This is used to persist
	/// the current state of the replicated raft log. This value does not affect
	/// the generated group id.
	pub(super) storage: S,

	/// The application-level state machine implementation.
	///
	/// This value is used when deriving the group id. All members of the group
	/// must be running the same state machine implementation.
	pub(super) state_machine: M,
}

/// Setters that are available when neither the state machine nor the storage
/// are set.
impl<'g> GroupBuilder<'g, (), ()> {
	/// Initialize a group builder for the given group key.
	pub fn new(groups: &'g Groups, key: GroupKey) -> Self {
		Self {
			groups,
			key,
			intervals: IntervalsConfig::default(),
			storage: (),
			state_machine: (),
		}
	}

	/// Sets the application-level state machine implementation for the group.
	/// This is used when deriving the group id, all members of the group must be
	/// running the same state machine implementation.
	///
	/// This setting must be set before the storage implementation, since the
	/// storage implementation must be compatible with the command type of the
	/// state machine.
	///
	/// By default `InMemory` storage is set with the command type of the state
	/// machine.
	pub fn with_state_machine<SM: StateMachine>(
		self,
		state_machine: SM,
	) -> GroupBuilder<'g, InMemory<SM::Command>, SM> {
		GroupBuilder {
			groups: self.groups,
			key: self.key,
			intervals: self.intervals,
			storage: InMemory::<SM::Command>::default(),
			state_machine,
		}
	}

	/// Joins a group with default configuration and noop state machine
	/// implementation.
	///
	/// This is useful for cases where the user just wants to join a group for the
	/// sake of knowing who is the leader and keep track of the group members,
	/// without needing to run any application-level logic or persist any state.
	pub fn join(self) -> Group<NoOp> {
		self.with_state_machine(NoOp).join()
	}
}

impl<'g, M> GroupBuilder<'g, InMemory<M::Command>, M>
where
	M: StateMachine,
{
	/// Sets the storage implementation for the state machine's replicated log.
	/// This is used to persist the current state of the log and must be
	/// compatible with the command type of the state machine. This value does
	/// not affect the generated group id.
	pub fn with_storage<S>(self, storage: S) -> GroupBuilder<'g, S, M>
	where
		S: Storage<M::Command>,
	{
		GroupBuilder {
			groups: self.groups,
			key: self.key,
			intervals: self.intervals,
			state_machine: self.state_machine,
			storage,
		}
	}
}

impl<S, M> GroupBuilder<'_, S, M> {
	/// Intervals configuration for the group protocol, such as heartbeat
	/// intervals, election timeouts, and consensus tick durations. This is used
	/// when deriving the group id, all members of the group must have identical
	/// configuration settings for these values, any difference will render a
	/// different group id.
	///
	/// This setting can be set independently of the state machine and storage
	/// settings.
	#[must_use]
	pub const fn with_intervals(mut self, intervals: IntervalsConfig) -> Self {
		self.intervals = intervals;
		self
	}
}

impl<S, M> GroupBuilder<'_, S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	/// Joins a group with the specified configuration.
	///
	/// The group builder values will generate a unique group id that is derived
	/// from the group key, the
	pub fn join(self) -> Group<M> {
		let config = GroupConfig::new::<M>(self.key, self.intervals);

		let group_id = *config.group_id();
		match self.groups.active.entry(group_id) {
			Entry::Occupied(existing) => existing.get().public_handle::<M>(),
			Entry::Vacant(place) => {
				let worker = Worker::<S, M>::spawn(
					self.groups,
					config,
					self.storage,
					self.state_machine,
				);
				place.insert(worker).public_handle::<M>()
			}
		}
	}
}

#[derive(Builder, Debug, Clone, Hash, PartialEq, Eq)]
#[builder(pattern = "owned", setter(prefix = "with"), derive(Debug, Clone))]
#[builder_struct_attr(doc(hidden))]
pub struct IntervalsConfig {
	/// The interval at which heartbeat messages are sent over established
	/// bonds to peers in the group to ensure liveness of the connection.
	#[builder(default = "Duration::from_millis(500)")]
	pub heartbeat_interval: Duration,

	/// The maximum jitter to apply to the heartbeat interval to avoid
	/// an avalanche of heartbeats being sent at the same time.
	///
	/// heartbeats are sent at intervals of
	/// `heartbeat_interval - rand(0, heartbeat_jitter)`.
	#[builder(default = "Duration::from_millis(150)")]
	pub heartbeat_jitter: Duration,

	/// The maximum number of consecutive missed heartbeats before considering
	/// the bond connection to be dead and closing it.
	#[builder(default = "10")]
	pub max_missed_heartbeats: u32,

	/// The election timeout duration for Raft leader elections within the
	/// group. This is the duration that a follower will wait without hearing
	/// from the leader before starting a new election. See the Raft paper
	/// section 5.2 for more details on the role of election timeouts in the Raft
	/// algorithm.
	#[builder(default = "Duration::from_secs(2)")]
	pub election_timeout: Duration,

	/// The maximum jitter to apply to the election timeout to avoid
	/// split votes during leader elections. See the Raft paper section 5.2 for
	/// more details on the role of election timeouts and randomization.
	#[builder(default = "Duration::from_millis(500)")]
	pub election_timeout_jitter: Duration,

	/// The duration to wait during bootstrap before starting elections.
	///
	/// This is the time given to allow nodes to discover other members of the
	/// group on the network before beginning leader elections process and
	/// self-nomination.
	#[builder(default = "Duration::from_secs(3)")]
	pub bootstrap_delay: Duration,

	/// The timeout duration for forwarding a command to the current leader and
	/// receiving an acknowledgment with the assigned log index.
	#[builder(default = "Duration::from_secs(2)")]
	pub forward_timeout: Duration,

	/// The timeout duration for fetching missing log entries during catch-up.
	#[builder(default = "Duration::from_secs(25)")]
	pub catchup_chunk_timeout: Duration,
}

impl Default for IntervalsConfig {
	fn default() -> Self {
		IntervalsConfigBuilder::default().build().unwrap()
	}
}

impl IntervalsConfig {
	/// Creates a new intervals config builder with default values.
	pub fn builder() -> IntervalsConfigBuilder {
		IntervalsConfigBuilder::default()
	}

	/// Returns a randomized election timeout duration.
	///
	/// Randomized timeouts are essential for Raft to minimize the chances of
	/// split votes during leader elections.
	pub(crate) fn election_timeout(&self) -> Duration {
		let base = self.election_timeout;
		let jitter = self.election_timeout_jitter;
		let range_start = base;
		let range_end = base + jitter;
		rand::random_range(range_start..range_end)
	}
}

/// Internal API
impl IntervalsConfig {
	pub(crate) fn digest(&self) -> Digest {
		Digest::from_parts(&[
			self.heartbeat_interval.as_millis().to_le_bytes(),
			self.heartbeat_jitter.as_millis().to_le_bytes(),
			u128::from(self.max_missed_heartbeats).to_le_bytes(),
			self.election_timeout.as_millis().to_le_bytes(),
			self.election_timeout_jitter.as_millis().to_le_bytes(),
			self.bootstrap_delay.as_millis().to_le_bytes(),
			self.forward_timeout.as_millis().to_le_bytes(),
		])
	}
}
