use {
	crate::local::Local,
	channel::Channel,
	futures::StreamExt,
	iroh::{
		Endpoint,
		EndpointAddr,
		discovery::static_provider::StaticProvider,
		protocol::RouterBuilder,
	},
	iroh_gossip::{
		Gossip,
		api::{Event as GossipEvent, GossipSender},
	},
	protocol::{Protocol, do_catalog_sync},
	std::sync::Arc,
	tokio::{
		sync::mpsc,
		task::{JoinHandle, JoinSet},
	},
	tokio_util::sync::{CancellationToken, DropGuard},
	tracing::info,
};

mod catalog;
mod channel;
mod error;
mod message;
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
			.send(Command::Dial(peer))
			.await
			.map_err(Error::send_command)?;
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

	pub(crate) fn new(local: &Local) -> Self {
		let catalog = Catalog::default();
		// add ourselves to the catalog, as other nodes will have us in their
		// catalog, so comparing hashes for catalog sync will fail if we don't
		// have ourselves in our catalog.
		catalog.insert(PeerInfo::new(local.endpoint().addr()));
		let protocol = Protocol::new(catalog.clone());
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
	async fn run(self) -> Result<(), Error> {
		let Self {
			local,
			gossip,
			catalog,
			mut commands,
			cancel,
		} = self;

		// TODO: topic_id should be defined in the discovery module as it's
		// discovery-specific
		let (topic_tx, mut topic_rx) = gossip
			.subscribe(local.network_id().topic_id(), vec![])
			.await?
			.split();

		let mut local_info = local.changes();
		let mut on_local_info_changed_tasks = JoinSet::new();
		let mut on_gossip_event_tasks = JoinSet::new();
		let mut on_command_tasks = JoinSet::new();

		loop {
			// TODO: implement regular broadcast of `SignedPeerInfo` to network
			tokio::select! {
				() = cancel.cancelled() => {
					on_terminated();
					return Ok(());
				}
				Ok(()) = local_info.changed() => {
					let info = local_info.borrow().clone();
					on_local_info_changed_tasks
						.spawn(on_local_info_changed(catalog.clone(), info, topic_tx.clone()));
				}
				Some(Ok(event)) = topic_rx.next() => {
					on_gossip_event_tasks
						.spawn(on_gossip_event(catalog.clone(), local.endpoint().clone(), event));
				}
				Some(command) = commands.recv() => {
					on_command_tasks
						.spawn(on_command(command, topic_tx.clone(), local.static_provider().clone()));
				}
				Some(res) = on_local_info_changed_tasks.join_next() => {
					match res {
						Ok(Ok(())) => {},
						Ok(Err(e)) => {
							tracing::warn!("failed to handle local info change: {e}");
						},
						Err(e) => {
							tracing::warn!("local info changed task failed: {e}");
						}
					}
				}
				Some(res) = on_gossip_event_tasks.join_next() => {
					match res {
						Ok(Ok(())) => {},
						Ok(Err(e)) => {
							tracing::warn!("failed to handle gossip event: {e}");
						},
						Err(e) => {
							tracing::warn!("gossip event task failed: {e}");
						}
					}
				}
				Some(res) = on_command_tasks.join_next() => {
					if let Err(e) = res {
						tracing::warn!("command task failed: {e}");
					}
				}
			}
		}
	}
}

fn on_terminated() {
	info!("Discovery event loop terminated");
}

async fn on_local_info_changed(
	catalog: Catalog,
	info: SignedPeerInfo,
	topic_tx: GossipSender,
) -> Result<(), Error> {
	info!("Local peer info updated: {info:?}");
	catalog.insert(info.clone());
	topic_tx
		.broadcast(info.into_bytes().map_err(Error::encode)?)
		.await
		.map_err(Error::gossip)?;
	Ok(())
}

async fn on_gossip_event(
	mut catalog: Catalog,
	endpoint: Endpoint,
	event: GossipEvent,
) -> Result<(), Error> {
	info!("Received gossip event in discovery event loop: {event:?}");
	match event {
		GossipEvent::Received(message) => {
			let signed_peer_info = SignedPeerInfo::from_bytes(&message.content)
				.map_err(Error::invalid_message)?;
			if !signed_peer_info.verify() {
				return Err(Error::InvalidSignedPeerInfo);
			}
			catalog.insert(signed_peer_info.into_peer_info());
		}
		GossipEvent::NeighborUp(peer_id) => {
			do_catalog_sync(peer_id.into(), &endpoint, &mut catalog).await?;
		}
		_ => {}
	}
	Ok(())
}

async fn on_command(
	command: Command,
	topic_tx: GossipSender,
	static_provider: StaticProvider,
) {
	match command {
		Command::Dial(peer) => {
			info!("Dialing peer via discovery event loop: {peer:?}");
			static_provider.add_endpoint_info(peer.clone());
			topic_tx.join_peers(vec![peer.id]).await.unwrap();
		}

		#[cfg(feature = "test-utils")]
		Command::Insert(info) => {
			info!("Inserting peer info into discovery catalog: {info:?}");
		}
	}
}

pub enum Command {
	Dial(EndpointAddr),

	#[cfg(feature = "test-utils")]
	Insert(PeerInfo),
}
