use {core::time::Duration, derive_builder::Builder};

/// Configuration options for the streams subsystem.
#[derive(Builder, Debug, Clone)]
#[builder(pattern = "owned", setter(prefix = "with"), derive(Debug, Clone))]
#[builder_struct_attr(doc(hidden))]
pub struct Config {
	/// The timeout duration for completing the handshake when establishing
	/// a new bond connection to a remote peer in the group.
	#[builder(default = "Duration::from_secs(2)")]
	pub handshake_timeout: Duration,

	/// The interval at which heartbeat messages are sent over established
	/// bonds to peers in the group to ensure liveness of the connection.
	#[builder(default = "Duration::from_millis(150)")]
	pub heartbeat_interval: Duration,

	/// The maximum jitter to apply to the heartbeat interval to avoid
	/// an avalanche of heartbeats being sent at the same time.
	///
	/// heartbeats are sent at intervals of
	/// `heartbeat_interval - rand(0, heartbeat_jitter)`.
	#[builder(default = "Duration::from_millis(50)")]
	pub heartbeat_jitter: Duration,

	/// The maximum number of consecutive missed heartbeats before considering
	/// the bond connection to be dead and closing it.
	#[builder(default = "10")]
	pub max_missed_heartbeats: usize,

	/// The tick duration for driving the Raft consensus protocol within the
	/// group.
	#[builder(default = "Duration::from_millis(150)")]
	pub consensus_tick: Duration,

	/// The election timeout duration for Raft leader elections within the
	/// group. This is the duration that a follower will wait without hearing
	/// from the leader before starting a new election.
	#[builder(default = "Duration::from_secs(1)")]
	pub election_timeout: Duration,

	/// The duration to wait during bootstrap before starting elections.
	///
	/// This is the time given to allow nodes to discover other members of the
	/// group on the network before beginning leader elections process and
	/// self-nomination.
	#[builder(default = "Duration::from_secs(3)")]
	pub bootstrap_delay: Duration,

	/// The maximum jitter to apply to the election timeout to avoid
	/// split votes during leader elections.
	#[builder(default = "Duration::from_millis(500)")]
	pub election_timeout_jitter: Duration,

	/// The backlog size for the consensus events broadcast channel before
	/// dropping unconsumed events.
	#[builder(default = "64")]
	pub consensus_events_backlog: usize,
}

impl Config {
	/// Creates a new config builder with default values.
	pub fn builder() -> ConfigBuilder {
		ConfigBuilder::default()
	}
}
