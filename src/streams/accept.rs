use {
	super::{Criteria, StreamId, Streams, producer::Sinks},
	crate::{
		NetworkId,
		discovery::Discovery,
		network::{
			error::DifferentNetwork,
			link::{Link, Protocol},
		},
		primitives::Short,
		streams::{StreamNotFound, UnknownPeer},
	},
	core::fmt,
	iroh::{
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler},
	},
	n0_error::Meta,
	serde::{Deserialize, Serialize},
	std::sync::Arc,
};

/// Streams protocol acceptor
///
/// This type is responsible for accepting incoming connections from remote
/// consumers and routing them to the appropriate [`SinkHandle`] based on the
/// [`StreamId`].
///
/// The stream subscription protocol works as follows:
///
/// - A remote consumer connects to a producer's stream sink using the streams
///   protocol ALPN identifier.
///
/// - The consumer sends a handshake message ([`ConsumerHandshake`]) containing
///   the desired [`StreamId`] and any filtering [`Criteria`].
///
/// - The acceptor looks up the corresponding [`SinkHandle`] for the requested
///   [`StreamId`].
///
/// - The connection is passed to the [`SinkHandle`] to handle the data stream
///   initiation with the consumer.
///
/// - If the connecting consumer peer is not known in the discovery catalog, the
///   subscription request is rejected. When the consumer gets a
///   [`CloseReason::UnknownPeer`], it should re-sync its catalog and retry the
///   subscription again.
pub(super) struct Acceptor {
	sinks: Arc<Sinks>,
	discovery: Discovery,
}

impl Acceptor {
	/// Creates a new [`Acceptor`] instance for the [`Streams`] subsystem.
	pub(super) fn new(streams: &Streams) -> Self {
		let sinks = Arc::clone(&streams.sinks);
		let discovery = streams.discovery.clone();

		Self { sinks, discovery }
	}
}

impl fmt::Debug for Acceptor {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		// Safety: ALPN is valid UTF-8 hardcoded at compile time
		unsafe { write!(f, "Streams({})", str::from_utf8_unchecked(Streams::ALPN)) }
	}
}

impl ProtocolHandler for Acceptor {
	async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
		let cancel = self.sinks.local.termination().clone();
		let mut link = Link::accept_with_cancel(connection, cancel).await?;
		let remote_peer_id = link.remote_id();
		let catalog = self.discovery.catalog();
		let Some(peer) = catalog.get(&remote_peer_id) else {
			tracing::trace!(
				peer_id = %Short(&remote_peer_id),
				"rejecting unidentified consumer",
			);

			// Close the link with a reason before returning error
			link
				.close(UnknownPeer)
				.await
				.map_err(|e| AcceptError::from_err(e))?;

			return Err(AcceptError::NotAllowed {
				meta: Meta::default(),
			});
		};

		tracing::trace!(
			consumer_id = %Short(peer.id()),
			consumer_info = %Short(peer),
			"new consumer connection",
		);

		// Receive the consumer's handshake message
		let handshake: ConsumerHandshake = link
			.recv()
			.await
			.inspect_err(|e| {
				tracing::debug!(
					consumer_id = %Short(peer.id()),
					error = %e,
					"Failed to receive consumer handshake",
				);
			})
			.map_err(AcceptError::from_err)?;

		// ensure that the consumer is connecting to the correct network
		if handshake.network_id != self.sinks.local.network_id() {
			tracing::debug!(
				consumer_id = %Short(peer.id()),
				stream_id = %Short(handshake.stream_id),
				expected_network = %Short(self.sinks.local.network_id()),
				received_network = %Short(handshake.network_id),
				"Consumer connected to wrong network",
			);

			// Close the link with a reason before returning error
			link
				.close(DifferentNetwork)
				.await
				.map_err(AcceptError::from_err)?;

			return Err(AcceptError::NotAllowed {
				meta: Meta::default(),
			});
		}

		// Lookup the fanout sink for the requested stream id
		let Some(sink) = self.sinks.open(handshake.stream_id) else {
			tracing::debug!(
				consumer_id = %Short(peer.id()),
				stream_id = %handshake.stream_id,
				"Consumer requesting unavailable stream",
			);

			// Close the link with a reason before returning error
			link
				.close(StreamNotFound)
				.await
				.map_err(AcceptError::from_err)?;

			return Err(AcceptError::NotAllowed {
				meta: Meta::default(),
			});
		};

		// pass the connection to the stream-specific [`SinkHandle`]
		// to handle the initiation of the data stream with this remote consumer.
		if let Err(link) = sink.accept(link, handshake.criteria, peer.clone()) {
			// the sink was terminated or unable to accept new consumers
			tracing::debug!(
				consumer_id = %Short(peer.id()),
				stream_id = %handshake.stream_id,
				"Sink terminated before accepting new consumer",
			);

			// Close the link before returning error, since the sink for this
			// stream id is no longer available, that is equivalent to stream not
			// found.
			return link
				.close(StreamNotFound)
				.await
				.map_err(AcceptError::from_err);
		}

		Ok(())
	}
}

/// Handshake sent by consumers when connecting to a producer's stream sink.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ConsumerHandshake {
	/// The network id the consumer is connecting to.
	network_id: NetworkId,

	/// The stream id the consumer wishes to subscribe to.
	stream_id: StreamId,

	/// The criteria the consumer is using to filter the stream data.
	criteria: Criteria,
}

impl ConsumerHandshake {
	/// Creates a new [`ConsumerHandshake`] instance.
	pub fn new(
		network_id: NetworkId,
		stream_id: StreamId,
		criteria: Criteria,
	) -> Self {
		Self {
			network_id,
			stream_id,
			criteria,
		}
	}
}

/// Handshake response sent by producers to consumers upon successful
/// initiation of a stream.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct StartStream(pub NetworkId, pub StreamId);

impl StartStream {
	pub const fn stream_id(&self) -> &StreamId {
		&self.1
	}

	pub const fn network_id(&self) -> &NetworkId {
		&self.0
	}
}
