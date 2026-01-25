use {
	crate::{
		GroupKey,
		Groups,
		groups::{Config, InvalidAuth, accept},
		network::{
			LocalNode,
			link::{Link, LinkError},
		},
		primitives::Short,
	},
	backoff::backoff::Backoff,
	core::future::{pending, ready},
	futures::FutureExt,
	iroh::{EndpointAddr, endpoint::ApplicationClose},
	std::sync::Arc,
	tokio::sync::watch,
	tokio_util::sync::{CancellationToken, ReusableBoxFuture},
};

/// This type represents the state of a connection between two members of a
/// group. Within one group each peer maintains a persistent connection to each
/// other peer in the group.
pub struct Connection {
	state: watch::Sender<State>,
}

impl Connection {
	pub fn connect(
		config: Arc<Config>,
		peer: impl Into<EndpointAddr>,
		local: LocalNode,
		key: GroupKey,
		cancel: CancellationToken,
	) -> Self {
		let state = watch::Sender::new(State::Connecting);
		let peer_addr: EndpointAddr = peer.into();

		let worker = WorkerLoop {
			config,
			key,
			local,
			peer_addr,
			next_connect: ReusableBoxFuture::new(pending()),
			backoff: None,
			state: state.clone(),
			cancel,
		};

		tokio::spawn(worker.spawn());

		Self { state }
	}

	pub fn accept(
		&self,
		config: Arc<Config>,
		link: Link<Groups>,
		local: LocalNode,
		key: GroupKey,
		cancel: CancellationToken,
	) -> Self {
		let state = watch::Sender::new(State::Connected);
		let peer_addr = link.remote_id().into();

		let worker = WorkerLoop {
			config,
			key,
			local,
			peer_addr,
			next_connect: ReusableBoxFuture::new(ready(Ok(link))),
			backoff: None,
			state: state.clone(),
			cancel,
		};

		tokio::spawn(worker.spawn());

		Self { state }
	}

	pub fn state_watch(&self) -> watch::Receiver<State> {
		self.state.subscribe()
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
	Connecting,
	Connected,
	Disconnected,
}

struct WorkerLoop {
	/// Configuration options for the groups subsystem.
	config: Arc<Config>,

	/// The group key used for authenticating this peer within
	/// the group when connecting to other peers.
	key: GroupKey,

	/// Local socket, used to establish connections to remote peers.
	local: LocalNode,

	/// The address of the remote peer in the group we are connecting to.
	peer_addr: EndpointAddr,

	/// Reusable future for connecting to the remote peer.
	next_connect: ReusableBoxFuture<'static, Result<Link<Groups>, LinkError>>,

	/// Backoff policy for reconnecting to the remote peer on recoverable
	/// errors. This is reset upon each successful heartbeat exchange.
	backoff: Option<Box<dyn Backoff + Send + Sync + 'static>>,

	/// Channel for sending updates about the current connection state with the
	/// remote peer.
	state: watch::Sender<State>,

	/// Triggered when the network is shutting down or this connection is
	/// being dropped, or goes into an unrecoverable error state.
	cancel: CancellationToken,
}

impl WorkerLoop {
	async fn spawn(mut self) {
		let connect_fut = self.connect();
		self.next_connect.set(connect_fut);

		loop {
			tokio::select! {
				() = self.cancel.cancelled() => {
					break;
				}

				result = &mut self.next_connect => {
					self.handle_connect_result(result);
				}
			}
		}
	}

	fn handle_connect_result(&mut self, result: Result<Link<Groups>, LinkError>) {
		tracing::info!(
			network_id = %self.local.network_id(),
			group_id = %self.key.id(),
			result = ?result,
			"connection attempt result",
		);

		match result {
			Ok(link) => {
				let _ = self.state.send_replace(State::Connected);
				self.next_connect.set(pending());
			}
			Err(err) => {
				let _ = self.state.send_replace(State::Disconnected);
			}
		}
	}

	fn connect(
		&mut self,
	) -> impl Future<Output = Result<Link<Groups>, LinkError>> + 'static {
		self.state.send_replace(State::Connecting);

		let cancel = self.cancel.clone();
		let network_id = *self.local.network_id();
		let group_key = self.key.clone();
		let peer_id = self.peer_addr.id;
		let own_id = self.local.id();
		let group_id = group_key.id();

		let connect_fut = self
			.local
			.connect_with_cancel::<Groups>(self.peer_addr.clone(), cancel);

		async move {
			tracing::debug!(
				network = %network_id,
				peer = %peer_id,
				group = %group_key.id(),
				"connecting to group member",
			);

			// attempt to establish a new connection to the remote peer
			let mut link = connect_fut.await?;

			// generate a proof of knowledge of the group id by hashing the known
			// group key secret with the tls-derived shared secret and the local peer
			// id
			let auth = group_key
				.secret()
				.derive(link.shared_random(group_id))
				.derive(own_id.as_bytes());

			// send the handshake initiation message to the accepting peer
			link
				.send(&accept::HandshakeStart {
					network_id,
					group_id,
					auth,
				})
				.await?;

			// wait for the accepting peer's response to our challenge
			let end: accept::HandshakeEnd = link.recv().await.inspect_err(|e| {
				tracing::debug!(
					network = %network_id,
					peer = %Short(peer_id),
					error = %e,
					"Failed to receive consumer handshake",
				);
			})?;

			// verify the response's proof of knowledge of the group secret
			// challenge by the accepting peer
			let expected_auth = group_key
				.secret()
				.derive(link.shared_random(group_id))
				.derive(peer_id.as_bytes());

			if end.auth != expected_auth {
				tracing::debug!(
					network = %network_id,
					peer = %Short(peer_id),
					"Invalid authentication proof from group member",
				);

				let close_reason: ApplicationClose = InvalidAuth.into();
				link
					.close(close_reason.clone())
					.await
					.map_err(LinkError::Close)?;
				return Err(LinkError::Open(close_reason.into()));
			}

			// successful connection and authentication
			tracing::info!(
				network = %network_id,
				peer = %Short(peer_id),
				group = %group_key.id(),
				"connected and authenticated group member",
			);

			tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

			Ok(link)
		}
		.fuse()
	}
}
