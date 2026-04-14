//! Replicated State Machine (RSM) module for consensus groups.
//!
//! This module defines the [`StateMachine`] trait — the main integration
//! point for application-specific logic in a mosaik consensus group. Every
//! group runs exactly one state machine instance per node, and the Raft
//! protocol ensures that all instances process the same commands in the same
//! order, converging on identical state.
//!
//! The module also defines the [`StateSync`] trait, which handles catch-up
//! for followers that fall behind the leader's log. The built-in
//! [`LogReplaySync`](super::LogReplaySync) implementation works with any
//! state machine by replaying missed log entries.

use {
	crate::{
		Datum,
		groups::{Cursor, Term},
		primitives::UniqueId,
	},
	serde::{Serialize, de::DeserializeOwned},
};

mod noop;
mod sync;

#[doc(hidden)]
pub use noop::NoOp;
// Public API traits for user-provided state machine implementations.
pub use sync::*;

/// The core trait for application-specific replicated state in a consensus
/// group.
///
/// Every mosaik consensus group is backed by a single `StateMachine`
/// implementation. The Raft protocol replicates [`Command`]s across all
/// group members, and each member applies them **in the same order** to
/// its local `StateMachine` instance, guaranteeing that all nodes converge
/// on identical state.
///
/// # Lifecycle
///
/// 1. **Create** — you instantiate your state machine and pass it to the group
///    builder via
///    [`with_state_machine`](super::GroupBuilder::with_state_machine).
/// 2. **Join** — calling [`join()`](super::GroupBuilder::join) on the builder
///    starts the Raft protocol and returns a [`Group<M>`](super::Group) handle.
/// 3. **Commands** — clients submit commands through
///    [`Group::execute`](super::Group::execute). The leader replicates each
///    command to a quorum of followers; once committed, every node calls
///    [`apply`](StateMachine::apply) with that command.
/// 4. **Queries** — clients call [`Group::query`](super::Group::query) with
///    either `Weak` (local, possibly stale) or `Strong` (forwarded to leader)
///    consistency. The group calls [`query`](StateMachine::query) on the local
///    state machine and returns the result.
/// 5. **Catch-up** — when a follower falls behind, the [`StateSync`]
///    implementation returned by [`state_sync`](StateMachine::state_sync) fills
///    the gap (replaying missed log entries or transferring a snapshot).
///
/// # Determinism
///
/// **`apply` must be deterministic.** Given the same sequence of commands,
/// every node must produce the exact same state. Do not read wall-clock
/// time, random values, or any external I/O inside `apply`. If you need
/// non-deterministic input, inject it *into* a command before submitting
/// it.
///
/// # Example
///
/// A simple counter replicated across a group:
///
/// ```rust
/// use {
/// 	mosaik::{
/// 		UniqueId,
/// 		groups::{
/// 			ApplyContext,
/// 			LeadershipPreference,
/// 			LogReplaySync,
/// 			StateMachine,
/// 		},
/// 	},
/// 	serde::{Deserialize, Serialize},
/// };
///
/// /// Commands that mutate the counter.
/// #[derive(Clone, Serialize, Deserialize)]
/// enum CounterCmd {
/// 	Increment(u32),
/// 	Decrement(u32),
/// }
///
/// /// Queries that read the counter.
/// #[derive(Clone, Serialize, Deserialize)]
/// struct GetValue;
///
/// struct Counter {
/// 	value: i64,
/// }
///
/// impl StateMachine for Counter {
/// 	type Command = CounterCmd;
/// 	type Query = GetValue;
/// 	type QueryResult = i64;
/// 	type StateSync = LogReplaySync<Self>;
///
/// 	fn signature(&self) -> UniqueId {
/// 		UniqueId::from("my_counter_v1")
/// 	}
///
/// 	fn apply(&mut self, cmd: CounterCmd, _ctx: &dyn ApplyContext) {
/// 		match cmd {
/// 			CounterCmd::Increment(n) => {
/// 				self.value += i64::from(n);
/// 			}
/// 			CounterCmd::Decrement(n) => {
/// 				self.value -= i64::from(n);
/// 			}
/// 		}
/// 	}
///
/// 	fn query(&self, _: GetValue) -> i64 {
/// 		self.value
/// 	}
///
/// 	fn state_sync(&self) -> LogReplaySync<Self> {
/// 		LogReplaySync::default()
/// 	}
/// }
/// ```
///
/// Then, to use it in a group:
///
/// ```rust,ignore
/// let group = network
///     .groups()
///     .with_key("my-counter-group")
///     .with_state_machine(Counter { value: 0 })
///     .join();
///
/// // Wait for a leader to be elected.
/// group.when().online().await;
///
/// // Submit a command (replicated to all members).
/// group.execute(CounterCmd::Increment(1)).await?;
///
/// // Read the current value (strong consistency).
/// let result = group.query(GetValue, Consistency::Strong).await?;
/// assert_eq!(*result, 1);
/// ```
pub trait StateMachine: Sized + Send + Sync + 'static {
	/// The type of commands that mutate the state machine.
	///
	/// Commands are the unit of replication: the leader appends each
	/// command to the Raft log, replicates it to a quorum of followers,
	/// and then every node applies it via [`apply`](Self::apply).
	///
	/// Commands must be serializable ([`Datum`]) because they are
	/// transmitted over the network and persisted in the log. They must
	/// also be [`Clone`] so the runtime can copy them when needed (e.g.
	/// during state sync or forwarding).
	type Command: Command;

	/// The type of read-only queries against the state machine.
	///
	/// Queries do **not** go through the Raft log. They are evaluated
	/// directly against the local state machine on whichever node
	/// handles the request:
	///
	/// - **Weak consistency** — the query runs on the local node immediately,
	///   which may return stale data.
	/// - **Strong consistency** — the query is forwarded to the current leader,
	///   guaranteeing a consistent read.
	type Query: Query;

	/// The result type returned by [`query`](Self::query).
	///
	/// Must be serializable so it can be sent over the network when a
	/// follower forwards a strong-consistency query to the leader.
	type QueryResult: QueryResult;

	/// The [`StateSync`] implementation used to synchronize followers
	/// that fall behind the leader's log.
	///
	/// Use [`LogReplaySync`](super::LogReplaySync) as a sensible
	/// default — it works with any state machine by replaying missed
	/// log entries from peers. For higher-throughput workloads, you can
	/// provide a snapshot-based implementation instead.
	type StateSync: StateSync<Machine = Self>;

	/// Returns a unique fingerprint of this state machine type and its
	/// configuration.
	///
	/// This value is **part of the [`GroupId`](super::GroupId)
	/// derivation**. All members of the same group must return an
	/// identical signature; any mismatch produces a different group id,
	/// preventing nodes from bonding.
	///
	/// # What to include
	///
	/// - The state machine's **type name or version tag** (so incompatible
	///   implementations never collide).
	/// - Any **configuration parameters** that affect command semantics (e.g.
	///   capacity limits, feature flags). If two nodes interpret the same command
	///   differently because of a config difference, the signature must differ.
	///
	/// # Example
	///
	/// ```rust,ignore
	/// fn signature(&self) -> UniqueId {
	///     UniqueId::from("my_orderbook_v2")
	///         .derive(self.max_depth.to_le_bytes())
	/// }
	/// ```
	fn signature(&self) -> UniqueId;

	/// Applies a committed command to the state machine.
	///
	/// This method is called **exactly once per committed log entry**,
	/// in log order, on every node in the group. Because all nodes
	/// apply the same commands in the same order, the state machine
	/// must remain **deterministic**: the same input must always produce
	/// the same state mutation.
	///
	/// The [`ApplyContext`] provides metadata about the current log
	/// position and term, which can be useful for versioning,
	/// compaction, or progress tracking.
	///
	/// # Panics
	///
	/// Panicking inside `apply` is treated as a fatal divergence — the
	/// node will shut down rather than risk inconsistent state.
	fn apply(&mut self, command: Self::Command, ctx: &dyn ApplyContext);

	/// Applies a batch of commands in one call.
	///
	/// The default implementation calls [`apply`](Self::apply) for
	/// each command sequentially. Override this if your state machine
	/// can amortize per-command overhead (e.g. by deferring index
	/// rebuilds until the end of the batch).
	///
	/// The batch is received in log order and all commands belong to
	/// the same term (available via [`ApplyContext::current_term`]).
	#[inline]
	fn apply_batch(
		&mut self,
		commands: impl IntoIterator<Item = Self::Command>,
		ctx: &dyn ApplyContext,
	) {
		for command in commands {
			self.apply(command, ctx);
		}
	}

	/// Evaluates a read-only query against the current state.
	///
	/// This is called on the node that handles the query (the local
	/// node for weak consistency, the leader for strong consistency).
	/// It must **not** mutate the state machine.
	fn query(&self, query: Self::Query) -> Self::QueryResult;

	/// Returns a new [`StateSync`] instance for this state machine.
	///
	/// Called once when the group is initialized. The returned value
	/// provides the session/provider factories that the Raft protocol
	/// uses during follower catch-up.
	///
	/// For most use cases, returning
	/// [`LogReplaySync::default()`](super::LogReplaySync::default) is
	/// sufficient:
	///
	/// ```rust,ignore
	/// fn state_sync(&self) -> LogReplaySync<Self> {
	///     LogReplaySync::default()
	/// }
	/// ```
	fn state_sync(&self) -> Self::StateSync;

	/// Returns the leadership preference for this node.
	///
	/// This is a **per-node** setting that does not affect the group
	/// id — different nodes in the same group can have different
	/// preferences. The Raft election logic enforces the preference:
	///
	/// - [`LeadershipPreference::Normal`] — standard candidate behavior (the
	///   default).
	/// - [`LeadershipPreference::Reluctant`] — longer election timeouts, reducing
	///   the chance of becoming leader but not preventing it entirely.
	/// - [`LeadershipPreference::Observer`] — never self-nominates, abstains from
	///   all votes, and does not count toward quorum. The node still replicates
	///   the log and applies commands.
	///
	/// # Example
	///
	/// A read-replica that should never lead:
	///
	/// ```rust,ignore
	/// fn leadership_preference(&self) -> LeadershipPreference {
	///     LeadershipPreference::Observer
	/// }
	/// ```
	#[inline]
	fn leadership_preference(&self) -> LeadershipPreference {
		LeadershipPreference::Normal
	}
}

/// Contextual information provided to the state machine when applying commands.
///
/// This trait does not offer any information that is non-deterministic or that
/// can diverge between different nodes in the group, so it is safe to use for
/// deterministic state machines.
pub trait ApplyContext {
	/// The index and term of the last committed log entry that has been applied
	/// by the state machine before applying the current batch of commands.
	///
	/// This can be used by the state machine to determine the current position of
	/// the log when it is applying new commands.
	///
	/// If the state machine implements its own `apply_batch` method, it needs to
	/// compute the index of each command in the batch by itself using the number
	/// of commands in the batch and the index of the last committed command
	/// returned by this method.
	fn committed(&self) -> Cursor;

	/// The index and term of the last log entry in the log.
	fn log_position(&self) -> Cursor;

	/// The term of the commands being applied.
	fn current_term(&self) -> Term;
}

pub trait StateMachineMessage:
	Clone + Send + Sync + Serialize + DeserializeOwned + 'static
{
}

impl<T> StateMachineMessage for T where
	T: Clone + Send + Sync + Serialize + DeserializeOwned + 'static
{
}

pub trait Command: Datum + Sync + Clone {}
impl<T> Command for T where T: Datum + Sync + Clone {}

pub trait Query: Datum + Sync + Clone {}
impl<T> Query for T where T: Datum + Sync + Clone {}

pub trait QueryResult: Datum + Sync + Clone {}
impl<T> QueryResult for T where T: Datum + Sync + Clone {}

/// Describes a node's preference for assuming leadership within a group.
///
/// This preference is per-node and does not affect the group identity.
/// Different nodes in the same group can have different leadership preferences.
/// The preference is enforced by the Raft election logic at the follower level:
/// observers never transition to candidate state, and reluctant nodes use
/// longer election timeouts to reduce their likelihood of winning elections.
///
/// `Normal` and `Reluctant` nodes participate fully in voting and log
/// replication. `Observer` nodes replicate the log but abstain from both
/// election votes and commit-advancement votes, so they never inflate the
/// quorum.
///
/// # Safety Considerations
///
/// - **Observer nodes do not count toward quorum.** They replicate the log and
///   advance their committed state, but abstain from election and commit votes.
///   This prevents slow or offline observers from stalling writes.
/// - **Liveness.** If all nodes in a group are observers, no leader can ever be
///   elected and the group will be unable to make progress. Ensure that at
///   least one node in every group has `Normal` or `Reluctant` preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LeadershipPreference {
	/// Default Raft behavior. The node participates in elections as a
	/// candidate with standard election timeouts.
	#[default]
	Normal,

	/// The node uses longer election timeouts, reducing its likelihood of
	/// becoming leader. It can still be elected if no other candidate is
	/// available (e.g., during network partitions or when all preferred
	/// leaders are down). The factor controls the timeout multiplier
	/// (e.g., a factor of 3 triples the election timeout).
	Reluctant {
		/// The multiplier applied to the election timeout and bootstrap
		/// delay. Must be greater than 1 for the deprioritization to have
		/// any effect.
		factor: u32,
	},

	/// The node never self-nominates as a candidate and abstains from all
	/// votes (both elections and commit advancement), so it never counts
	/// toward quorum. It still replicates the log and advances its
	/// committed state from the leader's heartbeats.
	Observer,
}

impl LeadershipPreference {
	/// Creates a reluctant preference with the default factor of 3.
	pub const fn reluctant() -> Self {
		Self::Reluctant { factor: 3 }
	}
}
