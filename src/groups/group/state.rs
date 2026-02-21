use {
	crate::{
		Consistency,
		NetworkId,
		PeerId,
		discovery::{Discovery, SignedPeerEntry},
		groups::{
			Bonds,
			CommandError,
			CommittedQueryResult,
			Config,
			Group,
			GroupId,
			Groups,
			IndexRange,
			QueryError,
			StateMachine,
			When,
			bond::{BondEvent, HandshakeStart},
			config::GroupConfig,
			key::SecretProof,
		},
		network::{LocalNode, link::Link},
	},
	core::any::{Any, TypeId},
	iroh::protocol::AcceptError,
	std::sync::Arc,
	tokio::sync::{
		mpsc::{UnboundedReceiver, UnboundedSender},
		oneshot,
	},
	tokio_util::sync::CancellationToken,
};

/// Manages an instance of a joined group worker loop.
#[derive(Debug)]
pub struct WorkerState {
	/// Configuration settings for this group, such as the group key and the
	/// consensus configuration. Those values must be identical across all
	/// members of the group, any difference will render a different group id and
	/// will prevent the members from forming a bond connection with each other.
	pub config: GroupConfig,

	/// Global configuration settings for the groups subsystem. This includes
	/// settings that are not specific to this group but affect the behavior of
	/// the subsystem as a whole, such as handshake timeouts.
	pub global_config: Arc<Config>,

	/// Reference to the local node networking stack instance.
	///
	/// This is needed to initiate outgoing connections to other peers in the
	/// group when they are discovered and bonds need to be created.
	pub local: LocalNode,

	/// A reference to the discovery service for peer discovery and peers
	/// catalog.
	pub discovery: Discovery,

	/// List of all active bonds in this group. Each bond represents a direct
	/// connection to another peer in the group. Bonds that are in the process of
	/// being established are also tracked here.
	///
	/// We always want to have this list in sync with `members`, so that we
	/// maintain bonds to all current members of the group. Any divergence
	/// between these two structures should be temporary and resolved quickly.
	pub bonds: Bonds,

	/// Cancellation token that terminates the worker loop for this group and all
	/// active bonds associated with it.
	pub cancel: CancellationToken,

	/// Used to signal changes to the group's state, such as leadership changes.
	pub when: When,

	/// Channel for sending external commands to the worker loop, such as
	/// forwarding incoming bond connection attempts that are routed to this
	/// worker by the acceptor.
	pub bonds_cmd_tx: UnboundedSender<WorkerBondCommand>,

	/// A type-erased channel for sending raft protocol commands to the worker
	/// loop. It's type-erased to keep the `WorkerState` struct generic over the
	/// state machine and storage types so we can store it in homogenous
	/// registers of groups alongside other groups with different state machine
	/// and storage implementations.
	pub raft_cmd_tx: Box<dyn Any + Send + Sync>,

	/// The type ids of the state machine and storage implementations used by
	/// this group.
	pub types: (TypeId, TypeId),
}

impl WorkerState {
	/// `PeerId` of the local node.
	pub fn local_id(&self) -> PeerId {
		self.local.id()
	}

	/// Returns the unique identifier of this group, which is derived from the
	/// group key and the hash of its configuration.
	pub const fn group_id(&self) -> &GroupId {
		self.config.group_id()
	}

	/// Returns the network id of this group, which is the same as the network id
	/// of the local node.
	pub fn network_id(&self) -> &NetworkId {
		self.local.network_id()
	}

	/// Returns the type id of the state machine implementation used by this
	/// group.
	pub const fn state_machine_type(&self) -> TypeId {
		self.types.0
	}

	/// Generates a proof of knowledge of the group secret key for the given link.
	///
	/// This is used during the bond establishment process to prove to the remote
	/// peer that we are indeed a member of the group and that we know the group
	/// secret key without revealing the key itself.
	pub fn generate_key_proof(&self, link: &Link<Groups>) -> SecretProof {
		self
			.config
			.key()
			.generate_proof(link, self.local.id(), *self.group_id())
	}

	/// Validates a proof of knowledge of the group secret key received from a
	/// remote peer during the bond establishment process.
	pub fn validate_key_proof(
		&self,
		link: &Link<Groups>,
		proof: SecretProof,
	) -> bool {
		self
			.config
			.key()
			.validate_proof(link, proof, *self.group_id())
	}

	/// Initiates the process of forming a bond connection with the specified
	/// peer.
	pub fn bond_with(&self, peer: SignedPeerEntry) {
		let _ = self.bonds_cmd_tx.send(WorkerBondCommand::Connect(peer));
	}

	/// Returns a public-api handle to this group.
	///
	/// Panics if the provided state machine type does not match the one used by
	/// this group.
	pub fn public_handle<M: StateMachine>(self: &Arc<Self>) -> Group<M> {
		assert_eq!(self.state_machine_type(), TypeId::of::<M>());
		Group::new(Arc::clone(self))
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
	pub async fn accept(
		&self,
		link: Link<Groups>,
		peer: SignedPeerEntry,
		handshake: HandshakeStart,
	) -> Result<(), AcceptError> {
		let (result_tx, result_rx) = oneshot::channel();
		let command =
			WorkerBondCommand::Accept(link.into(), peer, handshake, result_tx);

		// handoff the accept process to the background worker loop
		self
			.bonds_cmd_tx
			.send(command)
			.map_err(AcceptError::from_err)?;

		// wait for the worker loop to process the accept request
		result_rx.await.map_err(AcceptError::from_err)?
	}
}

/// Bond-related commands sent to the group worker loop.
pub enum WorkerBondCommand {
	/// Accepts an incoming connection for this group.
	/// Connections that are routed here have already passed preliminary
	/// validation such as network id and group id checks.
	Accept(
		Box<Link<Groups>>,
		SignedPeerEntry,
		HandshakeStart,
		oneshot::Sender<Result<(), AcceptError>>,
	),

	/// Attempts to create a new bond connection to the specified peer.
	Connect(SignedPeerEntry),

	/// When a bond is created, its event receiver is sent to the worker loop to
	/// be added to the aggregated stream of bond events that the worker loop
	/// listens to.
	///
	/// It is an explicit command to allow scheduling concurrent bonding processes
	/// on the worker loop without blocking the main group worker loop.
	SubscribeToBond(UnboundedReceiver<BondEvent>, PeerId),
}

/// Commands sent to the group worker that are raft-specific and carry state
/// machine impl types.
pub(in crate::groups) enum WorkerRaftCommand<M: StateMachine> {
	Feed(
		Vec<M::Command>,
		oneshot::Sender<Result<IndexRange, CommandError<M>>>,
	),

	Query(
		M::Query,
		Consistency,
		oneshot::Sender<Result<CommittedQueryResult<M>, QueryError<M>>>,
	),
}
