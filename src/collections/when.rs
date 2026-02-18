use super::Version;

/// Observer of a collection's state, which can be used to wait for the
/// collection to reach a certain state version before performing an action or
/// knowing when it is online or offline.
pub struct When {
	group: crate::groups::When,
}

impl When {
	pub(super) const fn new(group: crate::groups::When) -> Self {
		Self { group }
	}
}

impl When {
	/// Resolves collection is online, meaning it has successfully joined
	/// the network and the collection-specific group, identified the current
	/// leader and is in sync with the committed state of the group.
	///
	/// Mutable operations on the collection are only possible when it is online.
	pub async fn online(&self) {
		self.group.online().await;
	}

	/// Resolves when the collection is offline, meaning it has not yet joined the
	/// network or is not yet in sync with the committed state of the group.
	///
	/// A collection enters the offline state also when the current leader of the
	/// group becomes unavailable, and remains offline until a new leader is
	/// elected and the collection is in sync with the committed state of the
	/// group again.
	pub async fn offline(&self) {
		self.group.offline().await;
	}

	/// Resolves when new state versions are committed to the group and available
	/// to the collection.
	pub async fn updated(&self) {
		self.group.committed().changed().await;
	}

	/// Resolves when the collection has reached at least the given state version,
	/// meaning that the collection is in sync with the committed state of the
	/// group up to the specified version.
	///
	/// If the collection has already reached the specified version, this method
	/// will resolve immediately. Otherwise, it will wait until the collection has
	/// caught up to the specified version.
	///
	/// All mutable operations on all mosaik collections return the version of the
	/// state when the mutation is expected to be applied and committed to the
	/// group state.
	pub async fn reaches(&self, pos: Version) {
		self.group.committed().reaches(pos.0).await;
	}
}
