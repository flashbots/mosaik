use {
	super::protocol::Protocol,
	crate::prelude::PeerId,
	bytes::{Bytes, BytesMut},
	core::{
		ops::{Deref, DerefMut},
		pin::Pin,
		task::{Context, Poll},
	},
	futures::{SinkExt, Stream, StreamExt},
	iroh::{
		Endpoint,
		EndpointAddr,
		endpoint::{ConnectError, Connection, RecvStream, SendStream},
		protocol::AcceptError,
	},
	serde::{Serialize, de::DeserializeOwned},
	std::io,
	strum::{AsRefStr, FromRepr, IntoStaticStr},
	tokio::io::Join,
	tokio_util::codec::{Framed, LengthDelimitedCodec},
};

/// A type alias for a framed wire stream used by the `Link` type.
///
/// This type combines a `RecvStream` and `SendStream` into a single
/// bidirectional stream using `tokio::io::Join`, and applies length-delimited
/// framing to it using `LengthDelimitedCodec`.
pub(crate) type WireStream =
	Framed<Join<RecvStream, SendStream>, LengthDelimitedCodec>;

/// This type represents a physical link between two peers that can be used to
/// send and receive raw bytes of data on the `/mosaik/streams/1` protocol.
///
/// This type implements transport level framing it data is sent and received in
/// discrete packets of arbitrary length byte buffers.
pub(crate) struct Link {
	peer_id: PeerId,
	connection: Connection,
	wire: WireStream,
}

// Construction
impl Link {
	/// Creates a new Link on the accepting node.
	///
	/// There must be a corresponding `connect` call on the dialing node.
	pub(crate) async fn accept(
		connection: Connection,
	) -> Result<Self, AcceptError> {
		let peer_id = connection.remote_id();
		let (tx, rx) = connection.accept_bi().await?;
		let combined = tokio::io::join(rx, tx);
		let wire = WireStream::new(combined, LengthDelimitedCodec::new());

		Ok(Self {
			peer_id,
			wire,
			connection,
		})
	}

	/// Creates a new Link on the dialing node.
	///
	/// There must be a corresponding `accept` call on the accepting node.
	pub(crate) async fn connect(
		local: Endpoint,
		peer: EndpointAddr,
	) -> Result<Self, ConnectError> {
		let peer_id = peer.id;
		let connection = local.connect(peer, Protocol::ALPN).await?;
		let (tx, rx) = connection.open_bi().await?;
		let combined = tokio::io::join(rx, tx);
		let wire = Framed::new(combined, LengthDelimitedCodec::new());

		Ok(Self {
			wire,
			connection,
			peer_id,
		})
	}
}

impl Link {
	/// Unique identifier of the remote peer on the other end of this link.
	pub(crate) const fn peer_id(&self) -> &PeerId {
		&self.peer_id
	}

	/// Underlying iroh connection for this link.
	#[allow(dead_code)]
	pub(crate) const fn connection(&self) -> &Connection {
		&self.connection
	}

	/// Sends and flush a packet of data over this link.
	pub(crate) async fn send(
		&mut self,
		packet: impl Into<Bytes>,
	) -> Result<(), std::io::Error> {
		self.wire.send(packet.into()).await
	}

	/// Feeds a packet of data over this link without flushing.
	#[allow(dead_code)]
	pub(crate) async fn feed(
		&mut self,
		packet: impl Into<Bytes>,
	) -> Result<(), std::io::Error> {
		self.wire.feed(packet.into()).await
	}

	/// Receives a packet of data from this link.
	pub(crate) async fn recv(&mut self) -> Result<Bytes, std::io::Error> {
		let Some(packet) = self.wire.next().await.transpose()? else {
			return Err(std::io::Error::new(
				std::io::ErrorKind::UnexpectedEof,
				"Link closed",
			));
		};

		Ok(packet.into())
	}

	/// Reads and deserializes a packet of data from this link.
	pub(crate) async fn recv_as<T: DeserializeOwned>(
		&mut self,
	) -> Result<T, io::Error> {
		let packet = self.recv().await?;
		let data = rmp_serde::from_slice(&packet)
			.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

		Ok(data)
	}

	/// Serializes and sends a packet of data over this link.
	pub(crate) async fn send_as<T: Serialize>(
		&mut self,
		data: T,
	) -> Result<(), io::Error> {
		let packet = rmp_serde::to_vec(&data)
			.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

		self.send(packet).await
	}

	/// Closes this link without specifying a reason.
	pub(crate) async fn close(self) -> Result<(), std::io::Error> {
		self.close_with_reason(CloseReason::Unspecified).await
	}

	/// Closes this link with a specific reason.
	///
	/// This should be used to indicate why the link is being closed abnormally.
	pub(crate) async fn close_with_reason(
		mut self,
		reason: CloseReason,
	) -> Result<(), std::io::Error> {
		self.wire.close().await?;
		self
			.connection
			.close((reason as u8).into(), reason.as_ref().as_bytes());

		self.connection.closed().await;

		Ok(())
	}
}

#[derive(
	Debug, Clone, Copy, IntoStaticStr, AsRefStr, FromRepr, thiserror::Error,
)]
#[repr(u8)]
pub enum CloseReason {
	#[error("Unspecified")]
	Unspecified = 1,

	#[error("Stream not found")]
	StreamNotFound = 2,

	#[error("Data send error")]
	DataSendError = 3,

	#[error("Stream error")]
	StreamError = 4,

	#[error("Protocol error")]
	ProtocolError = 5,

	#[error("Network mismatch")]
	NetworkMismatch = 6,

	#[error("Remote closed the connection unexpectedly")]
	RemoteLinkClosed = 7,
}

/// Deref implementations to access the underlying WireStream
impl Deref for Link {
	type Target = WireStream;

	fn deref(&self) -> &Self::Target {
		&self.wire
	}
}

/// DerefMut implementations to access the underlying WireStream
impl DerefMut for Link {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.wire
	}
}

impl core::fmt::Debug for Link {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(f, "Link(to={})", self.peer_id)
	}
}

impl Stream for Link {
	type Item = Result<BytesMut, io::Error>;

	fn poll_next(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		self.get_mut().wire.poll_next_unpin(cx)
	}
}
