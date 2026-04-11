use {
	crate::{
		GroupKey,
		discovery::SignedPeerEntry,
		groups::{ConsensusConfig, GroupId, StateMachine, StateSync},
		primitives::{Expiration, InvalidTicket, TicketValidator},
	},
	core::time::Duration,
	derive_builder::Builder,
};

/// Configuration options for the streams subsystem.
#[derive(Builder, Debug, Clone)]
#[builder(pattern = "owned", setter(prefix = "with"), derive(Debug, Clone))]
#[builder_struct_attr(doc(hidden))]
pub struct Config {
	/// The timeout duration for completing the handshake when establishing
	/// a new bond connection to a remote peer in the group.
	#[builder(default = "Duration::from_secs(2)")]
	pub handshake_timeout: Duration,
}

impl Config {
	/// Creates a new config builder with default values.
	pub fn builder() -> ConfigBuilder {
		ConfigBuilder::default()
	}
}

/// Full set of configuration options a group instances.
///
/// This set of values is used to derive the group id and must be identical
/// across all members of the group, to ensure that they all run the same
/// consensus parameters.
pub struct GroupConfig {
	id: GroupId,
	key: GroupKey,
	consensus: ConsensusConfig,
	auth: Vec<Box<dyn TicketValidator>>,
}

impl GroupConfig {
	pub fn new(
		key: GroupKey,
		consensus: ConsensusConfig,
		state_machine: &impl StateMachine,
		auth: Vec<Box<dyn TicketValidator>>,
	) -> Self {
		let mut id = key
			.secret()
			.hashed()
			.derive(consensus.digest())
			.derive(state_machine.signature())
			.derive(state_machine.state_sync().signature());

		for validator in &auth {
			id = id.derive(validator.signature());
		}

		Self {
			id,
			key,
			consensus,
			auth,
		}
	}

	pub const fn group_id(&self) -> &GroupId {
		&self.id
	}

	pub const fn key(&self) -> &GroupKey {
		&self.key
	}

	pub const fn consensus(&self) -> &ConsensusConfig {
		&self.consensus
	}

	pub fn auth(&self) -> &[Box<dyn TicketValidator>] {
		&self.auth
	}

	/// Authorizes a peer's tickets against all configured validators.
	///
	/// Returns `Ok(None)` if no validators are configured.
	/// Returns `Ok(Some(expiration))` with the earliest expiration if all
	/// validators accept the peer.
	/// Returns `Err(InvalidTicket)` if any validator rejects the peer.
	pub fn authorize_peer(
		&self,
		peer: &SignedPeerEntry,
	) -> Result<Option<Expiration>, InvalidTicket> {
		peer.validate_tickets(&self.auth)
	}
}
