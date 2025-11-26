use {
	crate::local::Local,
	channel::Channel,
	futures::StreamExt,
	iroh::{EndpointAddr, protocol::RouterBuilder},
	iroh_gossip::{
		Gossip,
		api::{Event as GossipEvent, GossipSender},
	},
	protocol::Protocol,
	std::sync::Arc,
	tokio::{sync::mpsc, task::JoinHandle},
	tokio_util::sync::{CancellationToken, DropGuard},
	tracing::info,
};

mod catalog;
mod channel;
mod error;
mod peer;
mod protocol;

pub use {
	catalog::{Catalog, Event, Events},
	error::Error,
	peer::{PeerId, PeerInfo, SignedPeerInfo},
};

pub struct Discovery(Arc<Inner>);

impl Clone for Discovery {
	fn clone(&self) -> Self {
		Self(Arc::clone(&self.0))
	}
}

struct Inner {
	gossip: Gossip,
	catalog: Catalog,
	protocol: Protocol,
	cmd_tx: mpsc::Sender<Command>,
	_eventloop: JoinHandle<Result<(), Error>>,
	_abort: DropGuard,
}

/// Public API
impl Discovery {
	pub fn catalog(&self) -> &Catalog {
		&self.0.catalog
	}

	pub async fn dial(&self, peer: EndpointAddr) -> Result<(), Error> {
		self
			.0
			.cmd_tx
			.send(Command::Dial(peer.clone()))
			.await
			.unwrap();

		self.0.protocol.dial(peer).await
	}

	pub async fn dial_all(
		&self,
		peers: impl Iterator<Item = EndpointAddr>,
	) -> Result<(), Error> {
		for peer in peers {
			self.dial(peer).await?;
		}
		Ok(())
	}

	#[cfg(feature = "test-utils")]
	pub async fn insert(&self, info: PeerInfo) -> Result<(), Error> {
		info!("Inserting peer info into discovery catalog: {info:?}");
		self.0.cmd_tx.send(Command::Insert(info)).await.unwrap();
		Ok(())
	}
}

/// Internal API
impl Discovery {
	const ALPN_GOSSIP: &'static [u8] = b"/mosaik/gossip/1";

	pub(crate) fn new(local: Local) -> Self {
		let catalog = Catalog::default();
		let protocol = Protocol::new(local.clone(), catalog.clone());
		let cancel = CancellationToken::new();
		let gossip = Gossip::builder()
			.alpn(Self::ALPN_GOSSIP)
			.spawn(local.endpoint().clone());

		let eventloop = EventLoop {
			local: local.clone(),
			gossip: gossip.clone(),
			catalog: catalog.clone(),
			commands: Channel::default(),
			cancel: cancel.clone(),
		};

		let cmd_tx = eventloop.commands.sender().clone();
		let eventloop = tokio::spawn(eventloop.run());

		Self(Arc::new(Inner {
			gossip,
			catalog,
			protocol,
			cmd_tx,
			_eventloop: eventloop,
			_abort: cancel.drop_guard(),
		}))
	}

	pub(crate) fn attach(&mut self, router: RouterBuilder) -> RouterBuilder {
		router
			.accept(Self::ALPN_GOSSIP, self.0.gossip.clone())
			.accept(Protocol::ALPN, self.0.protocol.clone())
	}
}

struct EventLoop {
	local: Local,
	gossip: Gossip,
	catalog: Catalog,
	commands: Channel<Command>,
	cancel: CancellationToken,
}

impl EventLoop {
	pub async fn run(mut self) -> Result<(), Error> {
		let (topic_tx, mut topic_rx) = self
			.gossip
			.subscribe(self.local.network_id().topic_id(), vec![])
			.await?
			.split();

		let mut local_info = self.local.changes();

		loop {
			tokio::select! {
				_ = self.cancel.cancelled() => {
					self.on_terminated().await;
					return Ok(());
				}
				Ok(_) = local_info.changed() => {
					let info = local_info.borrow().clone();
					self.on_local_info_changed(info).await;
				}
				Some(Ok(event)) = topic_rx.next() => self.on_gossip_event(event).await,
				Some(command) = self.commands.recv() => self.on_command(command, &topic_tx).await,
			}
		}
	}

	async fn on_terminated(&mut self) {
		info!("Discovery event loop terminated");
	}

	async fn on_gossip_event(&mut self, event: GossipEvent) {
		info!("Received gossip event in discovery event loop: {event:?}");
	}

	async fn on_local_info_changed(&mut self, info: SignedPeerInfo) {
		info!("Local peer info updated: {info:?}");
	}

	async fn on_command(&mut self, command: Command, topic_tx: &GossipSender) {
		match command {
			Command::Dial(peer) => {
				info!("Dialing peer via discovery event loop: {peer:?}");
				topic_tx.join_peers(vec![peer.id]).await.unwrap();
			}

			#[cfg(feature = "test-utils")]
			Command::Insert(info) => {
				info!("Inserting peer info into discovery catalog: {info:?}");
			}
		}
	}
}

enum Command {
	Dial(EndpointAddr),

	#[cfg(feature = "test-utils")]
	Insert(PeerInfo),
}
