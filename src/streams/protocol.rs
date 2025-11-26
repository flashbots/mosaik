use {
	super::link::{CloseReason, Link},
	crate::{
		datum::{Criteria, StreamId},
		local::Local,
		prelude::NetworkId,
	},
	core::fmt,
	iroh::{
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler},
	},
	serde::{Deserialize, Serialize},
};

/// This type represents the `/mosaik/streams/1` protocol handler used by
/// Mosaik nodes to accept incoming stream subscription requests from remote
/// peers.
pub(crate) struct Protocol {
	local: Local,
}

impl Protocol {
	pub(crate) const ALPN: &'static [u8] = b"/mosaik/streams/1";

	pub(crate) fn new(local: Local) -> Self {
		Self { local }
	}
}

impl ProtocolHandler for Protocol {
	/// This code is called on the stream producer node when a remote peer
	/// connects and wants to subscribe to a stream.
	async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
		let mut link = Link::accept(connection).await?;

		// The accepting node expects a subscription request message as the first
		// message from the dialing node.
		let request = link.recv_as::<SubscriptionRequest>().await?;

		// Verify that the network ID matches.
		if request.network_id != *self.local.network_id() {
			link.close_with_reason(CloseReason::NetworkMismatch).await?;
			return Err(AcceptError::from_err(CloseReason::NetworkMismatch));
		}

		// Look up the requested stream ID in the local registry.
		let Some(sink) = self.local.open_sink(&request.stream_id) else {
			link.close_with_reason(CloseReason::StreamNotFound).await?;
			return Err(AcceptError::from_err(CloseReason::StreamNotFound));
		};

		// delegate the link to the stream specific sink
		sink.accept(link, request.criteria);

		Ok(())
	}
}

impl fmt::Debug for Protocol {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "Streams({})", String::from_utf8_lossy(Self::ALPN))
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SubscriptionRequest {
	network_id: NetworkId,
	stream_id: StreamId,
	criteria: Criteria,
}

impl SubscriptionRequest {
	pub(crate) fn new(
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
