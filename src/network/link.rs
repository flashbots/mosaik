use {
	crate::network::{LocalNode, PeerId},
	bincode::{
		config::standard,
		serde::{decode_from_std_read, encode_into_std_write},
	},
	bytes::{Buf, BufMut, Bytes, BytesMut},
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	futures::{Sink, SinkExt, Stream, StreamExt},
	iroh::{
		EndpointAddr,
		endpoint::{
			ApplicationClose,
			ConnectError,
			Connection,
			ConnectionError,
			RecvStream,
			SendStream,
			VarInt,
		},
		protocol::AcceptError,
	},
	serde::{Serialize, de::DeserializeOwned},
	std::io,
	strum::{AsRefStr, FromRepr, IntoStaticStr},
	tokio::io::{Join, join},
	tokio_util::codec::{Framed, LengthDelimitedCodec},
};

/// Represents a transport level open socket between two peers for a stream.
/// This type is shared between producers and consumers.
///
/// Notes:
///
/// - This is where the framing semantics are defined. We use [`Framed`] with
///   [`LengthDelimitedCodec`] to frame individual messages.
///
/// - Implements [`Stream`] and [`Sink`] for reading and writing framed
///   messages.
///
/// - The unit of transfer is [`Bytes`].
pub struct Link {
	alpn: Vec<u8>,
	connection: Connection,
	stream: Framed<Join<RecvStream, SendStream>, LengthDelimitedCodec>,
}

impl Link {
	/// Accepts a new incoming connection and initializes the framed stream.
	///
	/// This is used by the stream acceptor to handle incoming connections from
	/// remote consumer peers.
	pub async fn accept(connection: Connection) -> Result<Self, AcceptError> {
		let alpn = connection.alpn().to_owned();
		let (tx, rx) = connection.accept_bi().await?;
		let stream = Framed::new(join(rx, tx), LengthDelimitedCodec::new());

		Ok(Self {
			alpn,
			connection,
			stream,
		})
	}

	/// Opens a new outgoing connection to a remote peer on the streams protocol
	/// and initializes the framed stream. This is used by consumers to connect
	/// to remote producers.
	pub async fn connect(
		local: &LocalNode,
		remote: impl Into<EndpointAddr>,
		alpn: &[u8],
	) -> Result<Self, ConnectError> {
		let alpn = alpn.to_owned();
		let connection = local.endpoint().connect(remote.into(), &alpn).await?;
		let (tx, rx) = connection.open_bi().await?;
		let stream = Framed::new(join(rx, tx), LengthDelimitedCodec::new());

		Ok(Self {
			alpn,
			connection,
			stream,
		})
	}

	/// Returns the ALPN identifier for this link.
	pub fn alpn(&self) -> &[u8] {
		&self.alpn
	}

	/// Returns remote peer id.
	pub fn remote_id(&self) -> PeerId {
		self.connection.remote_id()
	}

	/// Returns the underlying iroh connection.
	pub const fn connection(&self) -> &Connection {
		&self.connection
	}

	/// Receives the next framed message from the link.
	pub async fn recv(&mut self) -> Result<BytesMut, io::Error> {
		self.stream.next().await.transpose()?.ok_or_else(|| {
			io::Error::new(io::ErrorKind::UnexpectedEof, "Link closed")
		})
	}

	/// Receives the next framed message and deserializes it into the given
	/// data type `D` using bincode deserialization.
	pub async fn recv_as<D: DeserializeOwned>(&mut self) -> Result<D, io::Error> {
		let bytes = self.recv().await?;
		let datum = decode_from_std_read(&mut bytes.reader(), standard())
			.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
		Ok(datum)
	}

	/// Sends a framed message over the link.
	///
	/// The message is serialized using bincode serialization and sent as a
	/// length-delimited frame.
	pub async fn send_as<D: Serialize>(
		&mut self,
		datum: &D,
	) -> Result<(), io::Error> {
		let mut writer = BytesMut::new().writer();
		encode_into_std_write(datum, &mut writer, standard())
			.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
		self.stream.send(writer.into_inner().freeze()).await
	}

	/// Closes the link with a given reason and waits for the closure to complete.
	pub async fn close_with_reason(
		mut self,
		reason: CloseReason,
	) -> Result<(), io::Error> {
		self.stream.flush().await?;
		self.stream.close().await?;

		self
			.connection()
			.close(VarInt::from(reason as u8), reason.into());

		let close_result = self.connection().closed().await;
		if close_result != ConnectionError::LocallyClosed {
			return Err(close_result.into());
		}

		Ok(())
	}

	/// Awaits the link closure and returns the closure result if the link was
	/// closed for a reason not indicating success.
	pub async fn closed(self) -> Result<(), ConnectionError> {
		match self.connection.closed().await {
			ConnectionError::LocallyClosed => Ok(()),
			ConnectionError::ApplicationClosed(ApplicationClose {
				error_code,
				..
			}) if error_code == VarInt::from(CloseReason::Success as u8) => Ok(()),
			err => Err(err),
		}
	}
}

impl core::fmt::Debug for Link {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		f.debug_struct("Link")
			.field("alpn", &String::from_utf8_lossy(self.alpn()))
			.field("remote_id", &self.remote_id())
			.finish_non_exhaustive()
	}
}

impl Stream for Link {
	type Item = Result<BytesMut, io::Error>;

	fn poll_next(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		self.get_mut().stream.poll_next_unpin(cx)
	}
}

impl Sink<Bytes> for Link {
	type Error = io::Error;

	fn poll_ready(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		self.get_mut().stream.poll_ready_unpin(cx)
	}

	fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
		self.get_mut().stream.start_send_unpin(item)
	}

	fn poll_flush(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		self.get_mut().stream.poll_flush_unpin(cx)
	}

	fn poll_close(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		self.get_mut().stream.poll_close_unpin(cx)
	}
}

#[derive(
	Debug, Clone, Copy, IntoStaticStr, AsRefStr, FromRepr, thiserror::Error,
)]
#[repr(u8)]
pub enum CloseReason {
	#[error("Protocol ran to completion successfully")]
	Success = 0,

	#[error("Unspecified")]
	Unspecified = 1,

	#[error("Stream not found")]
	StreamNotFound = 2,

	#[error("Invalid message")]
	InvalidMessage = 3,

	#[error("Error sending data")]
	SendError = 4,
}

impl From<CloseReason> for &'static [u8] {
	fn from(val: CloseReason) -> Self {
		let bytes: &'static str = val.into();
		bytes.as_bytes()
	}
}
