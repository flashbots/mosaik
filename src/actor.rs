use {
	crate::id::NetworkId,
	core::sync::atomic::{AtomicU32, Ordering},
	futures::{FutureExt, StreamExt},
	iroh::{Endpoint, EndpointAddr, protocol::Router},
	iroh_gossip::{
		Gossip,
		api::{Event, Message},
	},
	serde::{Deserialize, Serialize},
	std::{collections::HashMap, sync::Arc},
	tokio::{
		sync::{mpsc, oneshot},
		task::JoinHandle,
	},
	tracing::{error, info},
};

const ALPN_GOSSIP: &[u8] = b"/mosaik/gossip/1";

pub struct Actor {
	handle: JoinHandle<()>,
	sender: mpsc::Sender<ActorCommand>,
	peer_count: Arc<AtomicU32>,
}

impl Actor {
	pub async fn start(me: Endpoint, id: NetworkId) -> Self {
		let (cmd_sender, mut cmd_receiver) = mpsc::channel(1);
		let (ready_tx, ready_rx) = oneshot::channel::<()>();
		let peer_count = Arc::new(AtomicU32::new(0));
		let peer_count_ref = Arc::clone(&peer_count);

		let handle = tokio::spawn(async move {
			let gossip = Gossip::builder().alpn(ALPN_GOSSIP).spawn(me.clone());
			// let netview = NetworkView::new(me.clone(), id.clone()).await.unwrap();

			let _router = Router::builder(me.clone())
				.accept(ALPN_GOSSIP, gossip.clone())
				//.accept(NetworkView::ALPN, netview)
				.spawn();

			let mut neighbors = HashMap::new();
			let Ok(topic) = gossip.subscribe(id.topic_id(), vec![]).await else {
				error!("Failed to subscribe to topic {}", id.topic_id());
				return;
			};

			let (topic_tx, mut topic_rx) = topic.split();
			ready_tx.send(()).ok();

			loop {
				tokio::select! {
					Some(cmd) = cmd_receiver.recv() => {
						match cmd {
							ActorCommand::Dial(addr) => {
								info!("Dialing peer at address {addr:?}");
								topic_tx.join_peers(vec![addr.id]).await.ok();
							}
							ActorCommand::Ping => {
								let message = rmp_serde::to_vec(&ProtocolMessage::Hello(rand::random(), me.addr())).unwrap();
								topic_tx.broadcast(message.into()).await.ok();
							}
						}
					}
					Some(Ok(msg)) = topic_rx.next() => {
						//info!("Received message on topic {}: {:?}", id.topic_id(), msg);
						match msg {
							Event::NeighborUp(peer_id) => {
								let was = neighbors.len();
								neighbors.insert(peer_id, ());
								peer_count_ref.fetch_add(1, Ordering::AcqRel);
								//info!("Neighbor up: {}. Total neighbors: {}", peer_id, neighbors.len());

								if was == 0 {
									let message = rmp_serde::to_vec(&ProtocolMessage::Hello(rand::random(), me.addr())).unwrap();
									topic_tx.broadcast(message.into()).await.ok();
								}
							}
							Event::NeighborDown(peer_id) => {
								neighbors.remove(&peer_id);
								peer_count_ref.fetch_sub(1, Ordering::AcqRel);
								//info!("Neighbor down: {}. Total neighbors: {}", peer_id, neighbors.len());
							}
							Event::Received(Message { content, delivered_from, .. }) => {
								let msg: ProtocolMessage = rmp_serde::from_slice(&content).unwrap();
								info!("Message from {delivered_from}: {msg:?}");

								let ProtocolMessage::Hello(_, peer) = msg;
								neighbors.insert(peer.id, ());
								peer_count_ref.store(neighbors.len() as u32, Ordering::Relaxed);

							}
							Event::Lagged => {}
						}
					}
				}
			}
		});

		ready_rx.await.ok();

		Self {
			handle,
			sender: cmd_sender,
			peer_count,
		}
	}

	pub fn peer_count(&self) -> u32 {
		self.peer_count.load(Ordering::Acquire)
	}

	pub const fn sender(&self) -> &mpsc::Sender<ActorCommand> {
		&self.sender
	}
}

impl Future for Actor {
	type Output = Result<(), tokio::task::JoinError>;

	fn poll(
		self: core::pin::Pin<&mut Self>,
		cx: &mut core::task::Context<'_>,
	) -> core::task::Poll<Self::Output> {
		self.get_mut().handle.poll_unpin(cx)
	}
}

pub enum ActorCommand {
	Dial(EndpointAddr),
	Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ProtocolMessage {
	Hello(u32, EndpointAddr),
}
