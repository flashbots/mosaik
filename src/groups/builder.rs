use {
	crate::{
		Digest,
		GroupKey,
		groups::{
			Group,
			Groups,
			NoOp,
			StateMachine,
			Storage,
			config::GroupConfig,
			log::InMemoryLogStore,
			worker::Worker,
		},
	},
	core::time::Duration,
	dashmap::Entry,
	derive_builder::Builder,
	serde::{Deserialize, Serialize},
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

	/// Configures consensus parameters for the group protocol, such as
	/// heartbeat intervals or election timeouts.
	///
	/// Those values are used when deriving the group id, all members of the
	/// group must have identical configuration settings for these values, any
	/// difference will render a different group id.
	///
	/// If the state machine implementation provides a default consensus config,
	/// that will be used unless the value is explicitly set in the builder.
	pub(super) consensus: Option<ConsensusConfig>,

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
	pub(super) const fn new(groups: &'g Groups, key: GroupKey) -> Self {
		Self {
			groups,
			key,
			consensus: None,
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
	pub fn with_state_machine<SM: StateMachine>(
		self,
		state_machine: SM,
	) -> GroupBuilder<'g, InMemoryLogStore<SM::Command>, SM> {
		GroupBuilder {
			groups: self.groups,
			key: self.key,
			consensus: state_machine.consensus_config(),
			storage: InMemoryLogStore::<SM::Command>::default(),
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

impl<'g, M> GroupBuilder<'g, InMemoryLogStore<M::Command>, M>
where
	M: StateMachine,
{
	/// Sets the storage implementation for the state machine's replicated log.
	///
	/// This is used to persist the current state of the log and must be
	/// compatible with the command type of the state machine. This value does
	/// not affect the generated group id.
	///
	/// Defaults to an in-memory log store if not set explicitly.
	pub fn with_log_storage<S>(self, storage: S) -> GroupBuilder<'g, S, M>
	where
		S: Storage<M::Command>,
	{
		GroupBuilder {
			groups: self.groups,
			key: self.key,
			consensus: self.consensus,
			state_machine: self.state_machine,
			storage,
		}
	}
}

impl<S, M> GroupBuilder<'_, S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	/// Consensus configuration for the group protocol, such as heartbeat
	/// intervals, election timeouts. This is used
	///
	/// when deriving the group id, all members of the group must have identical
	/// configuration settings for these values, any difference will render a
	/// different group id.
	///
	/// If not set explicitly, the value defaults to either the value hinted by
	/// the state machine implementation via [`StateMachine::consensus_config`]
	/// method, or to the default consensus config if the state machine does not
	/// provide a hint.
	///
	/// Setting this value overrides the default consensus config provided by the
	/// state machine, if any.
	#[must_use]
	pub const fn with_consensus_config(
		mut self,
		consensus: ConsensusConfig,
	) -> Self {
		self.consensus = Some(consensus);
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
	/// from the group key, the state machine, and the consensus configuration.
	pub fn join(self) -> Group<M> {
		let config = GroupConfig::new(
			self.key, //
			self.consensus.unwrap_or_default(),
			&self.state_machine,
		);

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

#[derive(
	Builder, Debug, Clone, Hash, PartialEq, Serialize, Deserialize, Eq,
)]
#[builder(pattern = "owned", setter(prefix = "with"), derive(Debug, Clone))]
#[builder_struct_attr(doc(hidden))]
pub struct ConsensusConfig {
	/// The interval at which heartbeat messages are sent over established
	/// bonds to peers in the group to ensure liveness of the connection.
	///
	/// This value is used when deriving the group id and must be identical
	/// across all members of the group.
	#[builder(default = "Duration::from_millis(500)")]
	pub heartbeat_interval: Duration,

	/// The maximum jitter to apply to the heartbeat interval to avoid
	/// an avalanche of heartbeats being sent at the same time.
	///
	/// This value is used when deriving the group id and must be identical
	/// across all members of the group.
	///
	/// heartbeats are sent at intervals of
	/// `heartbeat_interval - rand(0, heartbeat_jitter)`.
	#[builder(default = "Duration::from_millis(150)")]
	pub heartbeat_jitter: Duration,

	/// The maximum number of consecutive missed heartbeats before considering
	/// the bond connection to be dead and closing it.
	///
	/// This value is used when deriving the group id and must be identical
	/// across all members of the group.
	#[builder(default = "10")]
	pub max_missed_heartbeats: u32,

	/// The election timeout duration for Raft leader elections within the
	/// group. This is the duration that a follower will wait without hearing
	/// from the leader before starting a new election. See the Raft paper
	/// section 5.2 for more details on the role of election timeouts in the Raft
	/// algorithm.
	///
	/// Nodes in the same group might have different preferences for the election
	/// timeout duration based on their role in the system. This affects the
	/// preference of the node to be a leader or a follower, and can be used to
	/// optimize the behavior of the group. For example, a node that prefers to
	/// be a follower can set a longer election timeout to reduce the chances of
	/// it becoming a leader, while a node that prefers to be a leader can set a
	/// shorter election timeout to increase the chances of it becoming a
	/// leader.
	///
	/// This value must be larger than the heartbeat interval.
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
	/// potentially self-nomination.
	#[builder(default = "Duration::from_secs(3)")]
	pub bootstrap_delay: Duration,

	/// The timeout duration for forwarding a command to the current leader and
	/// receiving an acknowledgment with the assigned log index.
	#[builder(default = "Duration::from_secs(2)")]
	pub forward_timeout: Duration,

	/// The timeout duration for the leader to respond to state machine queries
	/// to a follower querying the state with strong consistency.
	#[builder(default = "Duration::from_secs(2)")]
	pub query_timeout: Duration,
}

impl Default for ConsensusConfig {
	fn default() -> Self {
		ConsensusConfigBuilder::default().build().unwrap()
	}
}

impl ConsensusConfig {
	/// Creates a new intervals config builder with default values.
	pub fn builder() -> ConsensusConfigBuilder {
		ConsensusConfigBuilder::default()
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
impl ConsensusConfig {
	pub(crate) fn digest(&self) -> Digest {
		Digest::from_parts(&[
			self.heartbeat_interval.as_millis().to_le_bytes(),
			self.heartbeat_jitter.as_millis().to_le_bytes(),
			u128::from(self.max_missed_heartbeats).to_le_bytes(),
		])
	}
}
