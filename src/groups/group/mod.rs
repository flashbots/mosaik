use {
	crate::{
		GroupKey,
		Groups,
		NetworkId,
		groups::{GroupId, accept::HandshakeStart},
		network::link::Link,
	},
	iroh::protocol::AcceptError,
	std::sync::Arc,
	worker::{Handle, WorkerLoop},
};

mod bond;
// mod conn;
mod worker;

/// Represents a joined group instance that the local node is a member of.
///
/// Notes:
///
/// - This type is cheap to clone as it uses an Arc internally and all clones
///   refer to the same underlying group instance.
///
/// - A node can be a member of multiple groups simultaneously, but it can only
///   have one active group instance per unique group id. Attempting to join the
///   same group id multiple times will return the existing instance.
#[derive(Clone)]
pub struct Group(Arc<Handle>);

impl Group {
	/// Returns the unique identifier for this group that is derived from the
	/// group key.
	pub fn id(&self) -> &GroupId {
		self.key().id()
	}

	/// Returns the group key associated with this group.
	///
	/// The group key defines the authentication parameters for the group
	/// and is used to derive the group id.
	pub fn key(&self) -> &GroupKey {
		self.0.key()
	}

	/// Returns the network id this group belongs to.
	pub fn network_id(&self) -> &NetworkId {
		self.0.network_id()
	}
}

/// Internal API
impl Group {
	/// Called by the public Groups API when the local node is joining a new
	/// group. If this is the first call to join this group id, a new group
	/// instance is created and a background worker loop is spawned to manage it.
	/// Otherwise, a handle to the existing group instance is returned.
	pub(super) fn new(groups: &Groups, key: GroupKey) -> Self {
		let handle = WorkerLoop::spawn(
			key,
			&groups.local,
			groups.discovery.clone(),
			Arc::clone(&groups.config),
		);

		Self(handle)
	}

	/// Accepts an incoming bond connection for this group.
	///
	/// This is called by the group's protocol handler when a new connection
	/// is established  in [`Listener::accept`].
	///
	/// By the time this method is called:
	/// - The network id has already been verified to match the local node's
	///   network id.
	/// - The group id has already been verified to match this group's id.
	/// - The authentication proof has not been verified yet.
	/// - The presence of the remote peer in the local discovery catalog is not
	///   guaranteed.
	pub(super) async fn accept(
		&self,
		link: Link<Groups>,
		handshake: HandshakeStart,
	) -> Result<(), AcceptError> {
		self.0.accept(link, handshake).await
	}
}
