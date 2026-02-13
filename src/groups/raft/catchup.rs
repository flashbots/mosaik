use {
	crate::{
		NetworkId,
		PeerId,
		groups::{
			Cursor,
			GroupId,
			Index,
			StateMachine,
			Storage,
			raft::{
				Message,
				protocol::{LogEntry, Sync},
				shared::Shared,
			},
		},
		primitives::Short,
	},
	core::ops::RangeInclusive,
	std::collections::HashMap,
};

/// Implements the catch-up mechanism for lagging followers in the Raft
/// consensus algorithm.
///
/// Notes:
///
/// - Peers only offer committed log entries to followers catching up with the
///   leader.
#[derive(Debug)]
pub struct Catchup<M: StateMachine> {
	/// The range of log indices that the follower is missing compared to the
	/// leader.
	gap: RangeInclusive<Index>,

	/// Keeps track of which peers have responded to our `DiscoveryRequest`
	/// message and the range of log entries they have available for replay.
	available: HashMap<PeerId, RangeInclusive<Index>>,

	/// A buffer of log entries received from the leader while the follower is
	/// offline and catching up. These entries will be applied to the state
	/// machine once the follower has fetched any missing entries from peers and
	/// is in sync with the leader.
	buffered_entries: Vec<LogEntry<M::Command>>,

	/// Used in logging when we don't have access to the shared state.
	ids: (GroupId, NetworkId),
}

impl<M: StateMachine> Catchup<M> {
	/// Creates a new `Catchup` instance that manages the catch-up process for a
	/// lagging follower.
	///
	/// This constructor is called only once when the follower first detects that
	/// it is lagging behind the leader. The `position` parameter indicates the
	/// log position of the leader's `AppendEntries` message that triggered the
	/// catch-up process.
	pub fn new<S: Storage<M::Command>>(
		position: Cursor,
		shared: &Shared<S, M>,
	) -> Self {
		let gap = shared.log.last().index() + 1..=position.index();

		// Broadcast a `DiscoveryRequest` message to all bonded peers to discover
		// which peers have the log entries in the missing range so that we can
		// coordinate the catch-up process and distribute the fetching of among
		// them.
		shared
			.bonds()
			.broadcast_raft_message::<M>(Message::Sync(Sync::DiscoveryRequest));

		Self {
			gap,
			available: HashMap::new(),
			buffered_entries: Vec::new(),
			ids: (*shared.group_id(), *shared.network_id()),
		}
	}

	/// Called when we receive a `DiscoveryResponse` message from a peer in
	/// response to our `DiscoveryRequest` message. This method records the range
	/// of log entries that group peers have available for replay.
	pub fn record_availability(
		&mut self,
		available: RangeInclusive<Index>,
		peer: PeerId,
	) {
		tracing::trace!(
			peer = %Short(peer),
			range = ?available,
			group = %Short(self.ids.0),
			network = %Short(self.ids.1),
			"log availability confirmed from"
		);

		self.available.insert(peer, available);
	}

	/// Buffers the incoming log entries received from the leader while the
	/// follower is offline and catching up. These entries will be applied to the
	/// state machine once the gap is filled.
	pub fn buffer_entries(
		&mut self,
		_position: Cursor,
		entries: Vec<LogEntry<M::Command>>,
	) {
		self.buffered_entries.extend(entries);
	}
}
