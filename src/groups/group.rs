use {
	iroh::EndpointId,
	serde::{Deserialize, Serialize},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupState {
	public_key: iroh::PublicKey,
	members: Vec<EndpointId>,
}

impl GroupState {
	pub(crate) fn new(
		public_key: iroh::PublicKey,
		members: Vec<EndpointId>,
	) -> Self {
		Self {
			public_key,
			members,
		}
	}

	pub(crate) fn push_member(&mut self, member: EndpointId) {
		self.members.push(member);
	}

	pub(crate) fn remove_member(&mut self, member: &EndpointId) {
		self.members.retain(|m| m != member);
	}
}

// TODO
pub(super) struct Group {
	alpn: &'static [u8],
	state: GroupState,
	// RAFT state
}
