use {
	crate::{
		error::Error,
		id::NetworkId,
		local::Local,
		streams::{self, Consumer, Datum, Fanout, Producer, StreamId},
	},
	dashmap::{DashMap, Entry},
	iroh::{Endpoint, EndpointAddr, protocol::Router},
	std::sync::Arc,
};

/// Represents a connection to the Mosaik network.
pub struct Network {
	me: Local,
	network_id: NetworkId,
	producers: Arc<DashMap<StreamId, Fanout>>,
	_protocols: Router,
}

/// Public network management API
impl Network {
	/// Creates a new Network instance with the given NetworkId.
	///
	/// This will generate a random peer identity and bind to a random port.
	pub async fn new(network_id: NetworkId) -> Result<Self, Error> {
		Self::with_endpoint(network_id, Endpoint::bind().await?).await
	}

	/// Creates a new Network instance with the given NetworkId and Endpoint.
	pub async fn with_endpoint(
		network_id: NetworkId,
		endpoint: Endpoint,
	) -> Result<Self, Error> {
		let me = Local::new(endpoint);

		// wait for the local peer to be online
		me.online().await;

		let producers = Arc::new(DashMap::new());

		let protocols = Router::builder(me.endpoint().clone())
			.accept(
				streams::Protocol::ALPN,
				streams::Protocol::new(me.endpoint().clone(), producers.clone()),
			)
			.spawn();

		Ok(Self {
			me,
			network_id,
			producers,
			_protocols: protocols,
		})
	}

	/// Dials into a peer at a given address.
	///
	/// This will attempt to connect to the given peer and perform initial network
	/// view sync with that peer. This will also automatically join the
	/// announcements gossip topic.
	pub async fn dial(&self, _peer: EndpointAddr) -> Result<(), Error> {
		todo!()
	}

	/// Returns the endpoint of the local node.
	pub const fn local(&self) -> &Local {
		&self.me
	}

	/// Returns the NetworkId of this network.
	pub const fn network_id(&self) -> &NetworkId {
		&self.network_id
	}
}

/// Public pubsub API
impl Network {
	pub fn produce<D: Datum>(&self) -> Producer<D> {
		let stream_id = StreamId::of::<D>();
		match self.producers.entry(stream_id) {
			Entry::Occupied(occupied) => occupied.get().producer::<D>(),
			Entry::Vacant(vacant) => {
				let fanout = Fanout::new::<D>();
				let producer = fanout.producer();
				vacant.insert(fanout);
				producer
			}
		}
	}

	pub fn consume<D: Datum>(&self) -> Consumer<D> {
		todo!()
	}
}
