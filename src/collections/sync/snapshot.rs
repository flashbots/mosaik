use {
	crate::{
		collections::sync::SnapshotStateMachine,
		groups::{Index, IndexRange, Term},
	},
	serde::{Serialize, de::DeserializeOwned},
};

/// Represents a snapshot of the state of the state machine at a specific point
/// in time. This is used to quickly restore the state of a lagging follower
/// without having to replay the entire log.
pub trait Snapshot: Clone + Send + 'static {
	/// Type of the individual data items contained in this snapshot.
	type Item: SnapshotItem;

	/// Returns the log index at which this snapshot was taken. This is always
	/// going to be a committed index.
	fn position(&self) -> Index;

	/// The number of individual data items contained in this snapshot.
	///
	/// E.g. in `Vec` it will be the number of elements, in `Map` it will be the
	/// number of key-value pairs, etc. This is used to chunk the snapshot into
	/// smaller pieces and distributing requests for those pieces across multiple
	/// peers during the sync process.
	fn items_count(&self) -> u64;

	/// Returns an iterator over the individual data items contained in this
	/// snapshot.
	///
	/// This iterator must always return the same items in the same order for the
	/// same snapshot on all peers for a given range.
	///
	/// Implementation should either return the full range of items or `None`.
	fn items(
		&self,
		range: IndexRange,
	) -> Option<impl Iterator<Item = Self::Item>>;
}

pub trait SnapshotItem:
	Clone + Send + Serialize + DeserializeOwned + 'static
{
}

impl<T> SnapshotItem for T where
	T: Clone + Send + Serialize + DeserializeOwned + 'static
{
}
