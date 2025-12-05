use {
	super::{Criteria, Datum, StreamId, Streams, producer::Sinks},
	crate::{
		discovery::Discovery,
		network::link::{Link, Protocol},
		primitives::Short,
	},
	core::fmt,
	iroh::{
		endpoint::{ApplicationClose, Connection},
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
		let mut link = Link::accept(connection).await?;
		let remote_peer_id = link.remote_id();
		let catalog = self.discovery.catalog();
		let Some(info) = catalog.get(&remote_peer_id) else {
			tracing::debug!(
				consumer_id = %Short(&remote_peer_id),
				"unknown consumer peer",
			);

			// Close the link with a reason before returning error
			let reason: ApplicationClose = UnknownPeer.into();
			link.close(reason.clone()).await?;
			return Err(AcceptError::NotAllowed {
				meta: Meta::default(),
			});
		};

		tracing::info!(
			consumer_id = %Short(&remote_peer_id),
			consumer_info = ?info,
			"Accepted new consumer connection",
		);

		// Receive the consumer's handshake message
		let handshake: ConsumerHandshake = link
			.recv()
			.await
			.inspect_err(|e| {
				tracing::debug!(
					consumer_id = %Short(&remote_peer_id),
					error = %e,
					"Failed to receive consumer handshake",
				)
			})
			.map_err(AcceptError::from_err)?;

		// Lookup the fanout sink for the requested stream id
		let Some(sink) = self.sinks.open(handshake.stream_id) else {
			tracing::debug!(
				consumer_id = %Short(&remote_peer_id),
				stream_id = %handshake.stream_id,
				"Consumer requesting unavailable stream",
			);

			// Close the link with a reason before returning error
			let reason: ApplicationClose = StreamNotFound.into();
			link.close(reason.clone()).await?;
			return Err(AcceptError::NotAllowed {
				meta: Meta::default(),
			});
		};

		// pass the connection to the stream-specific [`SinkHandle`]
		// to handle the initiation of the data stream with this remote consumer.
		sink.accept(link, handshake.criteria);
		Ok(())
	}
}

/// Handshake sent by consumers when connecting to a producer's stream sink.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ConsumerHandshake {
	/// The stream id the consumer wishes to subscribe to.
	stream_id: StreamId,

	/// The criteria the consumer is using to filter the stream data.
	criteria: Criteria,
}

impl ConsumerHandshake {
	/// Creates a new [`ConsumerHandshake`] instance.
	pub fn new<D: Datum>(criteria: Criteria) -> Self {
		let stream_id = D::stream_id();
		Self {
			stream_id,
			criteria,
		}
	}
}

/// Handshake response sent by producers to consumers upon successful
/// initiation of a stream.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct StartStream(pub StreamId);

macro_rules! make_close_reason {
	($code:expr, $name:ident) => {
		#[derive(Debug, Clone, Copy)]
		pub(super) struct $name;
		const _: () = {
			impl From<$name> for iroh::endpoint::ApplicationClose {
				fn from(_: $name) -> Self {
					iroh::endpoint::ApplicationClose {
						error_code: iroh::endpoint::VarInt::from($code as u32),
						reason: stringify!($name).into(),
					}
				}
			}

			impl PartialEq<iroh::endpoint::ApplicationClose> for $name {
				fn eq(&self, other: &iroh::endpoint::ApplicationClose) -> bool {
					other.error_code == iroh::endpoint::VarInt::from($code as u32)
				}
			}

			impl PartialEq<$name> for iroh::endpoint::ApplicationClose {
				fn eq(&self, _: &$name) -> bool {
					self.error_code == iroh::endpoint::VarInt::from($code as u32)
				}
			}
		};
	};
}

make_close_reason!(0x1001, UnknownPeer);
make_close_reason!(0x1002, StreamNotFound);
