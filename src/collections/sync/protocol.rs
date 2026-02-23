//! Defines the wire-level messages exchanged between peers during snapshot
//! sync.

use {
	super::SnapshotItem,
	crate::{PeerId, groups::Cursor},
	chrono::{DateTime, Utc},
	core::ops::Range,
	derive_more::From,
	serde::{Deserialize, Serialize},
};

/// A message sent by a lagging follower to the current leader to request the
/// preparation of a snapshot for state synchronization.
///
/// This will trigger the insertion of a snapshot marker command in the log that
/// will be seen by all peers at the same log position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRequest {
	pub requested_by: PeerId,
	pub requested_at: DateTime<Utc>,
}

/// Message exchanged between `SnapshotSyncSession` and `SnapshotSyncProvider`
/// to coordinate the snapshot sync process.
#[derive(Clone, Serialize, Deserialize, From)]
#[serde(bound = "T: SnapshotItem")]
pub enum SnapshotSyncMessage<T> {
	/// A follower is starting a state catch-up session because it is lagging
	/// behind the state of the group.
	///
	/// This message is sent to the current leader and it triggers the insertion
	/// of a snapshot marker command in the log that will be seen by all peers and
	/// will trigger the creation of a snapshot at the same log position for all
	/// peers.
	RequestSnapshot,

	/// Sent by all peers to the lagging follower in response to the snapshot
	/// marker command being applied and committed in the log.
	///
	/// This message is the second step in the snapshot sync process after a
	/// follower sends a `RequestSnapshot` and it indicates that the snapshot is
	/// ready to be fetched. It contains the position of the snapshot in the log,
	/// which is the same for all peers since they all create the snapshot at the
	/// same log position, and the number of items in the snapshot.
	SnapshotOffer(SnapshotInfo),

	/// Sent by the lagging follower to all peers that have offered a snapshot
	/// through a `SnapshotOffer` message, requesting a specific range of data
	/// items in the snapshot anchored at a specific position.
	FetchDataRequest(FetchDataRequest),

	/// Sent by peers that have a snapshot ready in response to a
	/// `FetchDataRequest` message, containing the requested batch of snapshot
	/// data items.
	FetchDataResponse(FetchDataResponse<T>),
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
	/// The log position at which the snapshot was taken. This is the same for
	/// all peers since they all take the snapshot at the same log position when
	/// they apply the snapshot marker command.
	///
	/// The anchor position is used as an identifier of the snapshot and it is
	/// included in all messages related to that snapshot during the sync
	/// process.
	pub anchor: Cursor,

	/// The total number of items contained in the snapshot. Lagging followers
	/// will use this information to partition the snapshot download into
	/// batches and distribute the batches across multiple peers for parallel
	/// fetching.
	pub items_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchDataRequest {
	/// Specifies the log position at which the snapshot was taken.
	///
	/// This is the identifier of the snapshot that the follower wants to fetch
	/// data from.
	pub anchor: Cursor,

	/// The range of items in the snapshot that the follower wants to fetch. This
	/// is going to be a value between 0 and the `items_count` value included in
	/// the `SnapshotOffer` message that the peer received before.
	///
	/// The order of items on all nodes offering the snapshot should be the same
	/// and they should return the same items for the same range/anchor
	/// combination.
	pub range: Range<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(bound = "T: SnapshotItem")]
pub struct FetchDataResponse<T> {
	/// The log position at which the snapshot was taken.
	///
	/// This is the identifier of the snapshot that the follower is fetching data
	/// from.
	pub anchor: Cursor,

	/// The offset of the first item in this batch within the snapshot. This is
	/// the same as the `range.start` value in the corresponding
	/// `FetchDataRequest`
	pub offset: u64,

	/// The batch of items in the snapshot that the peer is sending in response
	/// to the `FetchDataRequest`. The items in the batch should be ordered
	/// according to their offset in the snapshot, so the first item in the
	/// batch should have the offset specified by the `offset` field and the
	/// last item should have the offset `offset + items.len() - 1`.
	pub items: Vec<T>,
}
