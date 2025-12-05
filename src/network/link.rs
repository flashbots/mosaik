use {
	super::{LocalNode, PeerId},
	crate::primitives::Short,
	bincode::{
		config::standard,
		error::{DecodeError, EncodeError},
		serde::{decode_from_std_read, encode_into_std_write},
	},
	bytes::{Buf, BufMut, BytesMut},
	core::{fmt, marker::PhantomData},
	futures::{SinkExt, StreamExt},
	iroh::{
		EndpointAddr,
		endpoint::{
			ApplicationClose,
			ConnectError,
			ConnectingError,
			Connection,
			ConnectionError,
			ReadError,
			RecvStream,
			SendStream,
			VarInt,
			WriteError,
		},
		protocol::AcceptError as IrohAcceptError,
	},
	n0_error::Meta,
	serde::{Serialize, de::DeserializeOwned},
	std::io,
	strum::{AsRefStr, FromRepr, IntoStaticStr},
	tokio::io::{Join, join},
	tokio_util::{
		codec::{Framed, LengthDelimitedCodec},
		sync::CancellationToken,
	},
};

/// Protocol trait for defining ALPN identifiers for network protocols.
///
/// This trait ensures that a given [`Link`] instance is always associated with
/// a specific ALPN identifier.
pub trait Protocol {
	const ALPN: &'static [u8];
}

/// Represents a transport level open bidirectional stream between two peers
/// using a well-known protocol.
///
/// Notes:
///
/// - This is where the framing semantics are defined. We use [`Framed`] with
///   [`LengthDelimitedCodec`] to frame individual messages.
///
/// - A link can be instantiated either by accepting an incoming connection or
///   by opening an outgoing connection to a remote peer.
///
/// - All operations are cancellable via the attached cancellation token. An
///   external token can be provided when accepting or opening a link, or a
///   default token specific to the link will be created. All errors have a
///   `Cancelled` variant.
///
/// - Links are always bi-directional, meaning that both peers can send and
///   receive messages over the same link.
///
/// - Connections in cancelled links are closed with a `Cancelled` close reason.
pub struct Link<P: Protocol> {
	connection: Connection,
	cancel: CancellationToken,
	stream: Framed<Join<RecvStream, SendStream>, LengthDelimitedCodec>,
	_protocol: PhantomData<P>,
}

// Public API
impl<P: Protocol> Link<P> {
	/// Accepts a new incoming connection, opens a bidirectional stream and
	/// initializes message framing.
	///
	/// Accepts an external cancellation token to allow cancelling any pending
	/// link operations.
	///
	/// Note: the peer that calls `open` must send the first message before the
	/// peer that `accept`s can begin accepting the connection.
	pub async fn accept_with_cancel_token(
		connection: Connection,
		cancel: CancellationToken,
	) -> Result<Self, AcceptError> {
		// reject any connection that does not match the expected typed ALPN
		if P::ALPN != connection.alpn().as_ref() {
			let alpn = connection.alpn().to_vec();

			// close the connection with invalid alpn reason, the remote peer should
			// receive this reason as part of the application close frame.
			if let Some(reason) =
				close_connection(&connection, SystemReason::InvalidAlpn).await
			{
				// the connection actually failed before we could send the close frame
				// return the transport error instead of invalid alpn
				return Err(reason.into());
			}

			return Err(AcceptError::InvalidAlpn {
				expected: P::ALPN,
				received: alpn,
			});
		}

		// all streams are bi-directional
		let Some(accept_result) =
			cancel.run_until_cancelled(connection.accept_bi()).await
		else {
			close_connection(&connection, SystemReason::Cancelled).await;
			return Err(AcceptError::Cancelled);
		};

		let (tx, rx) = accept_result?;
		let stream = Framed::new(join(rx, tx), LengthDelimitedCodec::new());

		Ok(Self {
			connection,
			stream,
			cancel,
			_protocol: PhantomData,
		})
	}

	/// Accepts a new incoming connection, opens a bidirectional stream and
	/// initializes message framing.
	///
	/// This version of `accept` uses a default cancellation token specific to
	/// the link.
	///
	/// Note: the peer that calls `open` must send the first message before the
	/// peer that `accept`s can begin accepting the connection.
	pub async fn accept(connection: Connection) -> Result<Self, AcceptError> {
		Self::accept_with_cancel_token(connection, CancellationToken::new()).await
	}

	/// Initiates a new outgoing connection to a remote peer, opens a
	/// bidirectional stream and initializes message framing.
	///
	/// This version of `open` accepts an external cancellation token to allow
	/// cancelling any pending link operations.
	///
	/// Note: the peer that calls `open` must send the first message before the
	/// peer that `accept`s can begin accepting the connection.
	pub async fn open_with_cancel_token(
		local: &LocalNode,
		remote: impl Into<EndpointAddr>,
		cancel: CancellationToken,
	) -> Result<Self, OpenError> {
		let fut = local.endpoint().connect(remote.into(), P::ALPN);
		let Some(connection) = cancel.run_until_cancelled(fut).await else {
			return Err(OpenError::Cancelled);
		};

		let connection = connection?;

		// all streams are bi-directional
		let Some(open_result) =
			cancel.run_until_cancelled(connection.open_bi()).await
		else {
			close_connection(&connection, SystemReason::Cancelled).await;
			return Err(OpenError::Cancelled);
		};

		let (tx, rx) = match open_result {
			Ok(streams) => streams,
			Err(err) => {
				close_connection(&connection, SystemReason::Cancelled).await;
				return Err(err.into());
			}
		};

		let combined = join(rx, tx);
		let stream = Framed::new(combined, LengthDelimitedCodec::new());

		Ok(Self {
			connection,
			stream,
			cancel,
			_protocol: PhantomData,
		})
	}

	/// Initiates a new outgoing connection to a remote peer, opens a
	/// bidirectional stream and initializes message framing.
	///
	/// This version of `open` uses a default cancellation token specific to
	/// this link.
	///
	/// Note: the peer that calls `open` must send the first message before the
	/// peer that `accept`s can begin accepting the connection.
	pub async fn open(
		local: &LocalNode,
		remote: impl Into<EndpointAddr>,
	) -> Result<Self, OpenError> {
		Self::open_with_cancel_token(local, remote, CancellationToken::new()).await
	}

	/// Returns the ALPN identifier for this link.
	pub fn alpn(&self) -> &[u8] {
		P::ALPN
	}

	/// Returns remote peer id.
	pub fn remote_id(&self) -> PeerId {
		self.connection.remote_id()
	}

	/// Returns the underlying iroh connection.
	pub const fn connection(&self) -> &Connection {
		&self.connection
	}

	/// Returns `true` if the link has been cancelled.
	pub fn is_cancelled(&self) -> bool {
		self.cancel.is_cancelled()
	}

	/// Receives the next framed message and deserializes it into the given
	/// data type `D` using bincode deserialization.
	pub async fn recv<D: DeserializeOwned>(&mut self) -> Result<D, RecvError> {
		let Some(frame) = self.cancel.run_until_cancelled(self.stream.next()).await
		else {
			close_connection(&self.connection, SystemReason::Cancelled).await;
			return Err(RecvError::Cancelled);
		};

		let Some(read_result) = frame else {
			let Some(reason) =
				close_connection(&self.connection, SystemReason::UnexpectedClose).await
			else {
				return Err(SystemReason::UnexpectedClose.into());
			};
			return Err(reason.into());
		};

		let mut reader = match read_result {
			Ok(bytes) => bytes.reader(),
			Err(err) => match err.downcast::<ReadError>() {
				Ok(read_err) => return Err(RecvError::Io(read_err)),
				Err(other_err) => return Err(RecvError::Unknown(other_err)),
			},
		};

		// deserialize the received bytes into the expected data type, if
		// deserialization fails, close the connection with protocol violation
		let decoded = match decode_from_std_read(&mut reader, standard()) {
			Ok(datum) => datum,
			Err(err) => {
				close_connection(&self.connection, SystemReason::ProtocolViolation)
					.await;
				return Err(RecvError::Decode(err));
			}
		};

		Ok(decoded)
	}

	/// Sends a framed message over the link.
	///
	/// The message is serialized using bincode serialization and sent as a
	/// length-delimited frame.
	///
	/// If the serialization fails, the link is closed with a [`UnexpectedClose`].
	pub async fn send<D: Serialize>(
		&mut self,
		datum: &D,
	) -> Result<(), SendError> {
		let mut writer = BytesMut::new().writer();
		if let Err(e) = encode_into_std_write(datum, &mut writer, standard()) {
			close_connection(&self.connection, SystemReason::ProtocolViolation).await;
			return Err(e.into());
		}
		let fut = self.stream.send(writer.into_inner().freeze());
		let Some(send_result) = self.cancel.run_until_cancelled(fut).await else {
			close_connection(&self.connection, SystemReason::Cancelled).await;
			return Err(SendError::Cancelled);
		};

		match send_result {
			Ok(()) => Ok(()),
			Err(err) => match err.downcast::<WriteError>() {
				Ok(io_err) => Err(SendError::Io(io_err)),
				Err(other_err) => Err(SendError::Unknown(other_err)),
			},
		}
	}

	/// Closes the link with a given reason and waits for the closure to complete.
	/// The `CloseReason` is sent as part of the application-level close frame to
	/// the remote peer.
	///
	/// If the link was closed before calling this method (e.g., by the remote
	/// peer), the an error is returned indicating the existing close reason.
	pub async fn close(
		mut self,
		reason: impl Into<ApplicationClose>,
	) -> Result<(), ConnectionError> {
		// flush any pending outgoing data before closing and receive any incoming
		// close frames.
		let _ = self.stream.flush().await;
		let _ = self.stream.close().await;

		// link already closed, return the existing reason as sent by the remote
		// peer or generated by the network layer
		if let Some(reason) = self.connection().close_reason() {
			return Err(reason);
		}

		// otherwise send the close frame to the remote peer
		let reason: ApplicationClose = reason.into();
		self.connection().close(reason.error_code, &reason.reason);

		// await the link closure confirmation and return any error reason
		// other than locally closed.
		let close_result = self.connection().closed().await;
		if close_result != ConnectionError::LocallyClosed {
			return Err(close_result);
		}

		Ok(())
	}

	/// Awaits the link closure and returns the closure result if the link was
	/// closed for a reason not indicating success.
	pub async fn closed(self) -> Result<(), ConnectionError> {
		match self.connection.closed().await {
			ConnectionError::LocallyClosed => Ok(()),
			ConnectionError::ApplicationClosed(reason)
				if reason == SystemReason::Completed =>
			{
				Ok(())
			}
			err => Err(err),
		}
	}
}

/// Closes the given connection with the specified reason and awaits the
/// closure confirmation. If the connection was already closed, then the
/// existing closure error is returned.
async fn close_connection(
	connection: &Connection,
	reason: SystemReason,
) -> Option<ConnectionError> {
	connection.close(reason.error_code(), reason.reason());
	match connection.closed().await {
		ConnectionError::LocallyClosed => None,
		err => Some(err),
	}
}

/// Errors that can occur when receiving data from a link.
#[derive(Debug, thiserror::Error)]
pub enum RecvError {
	#[error("Read error: {0:?}")]
	Io(#[from] ReadError),

	/// This error indicates that the data was read successfully but failed to
	/// deserialize it into a typed structure as set in [`Link::recv_as`].
	#[error("Decode error: {0:?}")]
	Decode(#[from] DecodeError),

	#[error("Unknown error: {0}")]
	Unknown(#[from] io::Error),

	#[error("Operation cancelled")]
	Cancelled,
}

impl RecvError {
	/// If the connection was closed with an application-level close frame,
	/// returns the associated `ApplicationClose` reason. Otherwise returns
	/// `None`.
	pub fn close_reason(&self) -> Option<&ApplicationClose> {
		match self {
			RecvError::Io(ReadError::ConnectionLost(
				ConnectionError::ApplicationClosed(reason),
			)) => Some(reason),
			_ => None,
		}
	}

	/// Returns `true` if the error indicates that the operation was locally
	/// cancelled.
	pub fn is_cancelled(&self) -> bool {
		matches!(self, RecvError::Cancelled)
	}
}

impl From<ConnectionError> for RecvError {
	fn from(err: ConnectionError) -> Self {
		RecvError::Io(ReadError::ConnectionLost(err))
	}
}

impl From<SystemReason> for RecvError {
	fn from(val: SystemReason) -> Self {
		RecvError::Io(ReadError::ConnectionLost(
			ConnectionError::ApplicationClosed(val.into()),
		))
	}
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
	#[error("io error: {0}")]
	Io(#[from] ConnectError),

	#[error("Operation cancelled")]
	Cancelled,
}

/// Errors that occur when accepting a new connection from a remote peer.
#[derive(Debug, thiserror::Error)]
pub enum AcceptError {
	#[error("IO error: {0}")]
	Io(#[from] IrohAcceptError),

	/// The remote peer is trying to connect with a different ALPN than expected.
	#[error("Invalid ALPN: expected {expected:?}, received {received:?}")]
	InvalidAlpn {
		expected: &'static [u8],
		received: Vec<u8>,
	},

	/// The link was cancelled before or during acceptance.
	#[error("Operation cancelled")]
	Cancelled,
}

impl From<AcceptError> for IrohAcceptError {
	fn from(err: AcceptError) -> Self {
		match err {
			AcceptError::Io(e) => e,
			error => IrohAcceptError::from_err(error),
		}
	}
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
	#[error("encoder error: {0}")]
	Encode(#[from] EncodeError),

	#[error("IO error: {0}")]
	Io(#[from] WriteError),

	#[error("Unknown error: {0}")]
	Unknown(#[from] io::Error),

	#[error("Operation cancelled")]
	Cancelled,
}

#[derive(Debug, thiserror::Error)]
pub enum LinkError {
	#[error("Connection error: {0}")]
	Open(OpenError),

	#[error("Connection error: {0}")]
	Accept(AcceptError),

	#[error("Receive error: {0}")]
	Recv(RecvError),

	#[error("Send error: {0}")]
	Write(SendError),

	#[error("Operation cancelled")]
	Cancelled,
}

impl LinkError {
	/// Returns `true` if the error indicates that the operation was locally
	/// cancelled.
	pub fn is_cancelled(&self) -> bool {
		matches!(self, LinkError::Cancelled)
	}

	pub fn close_reason(&self) -> Option<&ApplicationClose> {
		match self {
			LinkError::Open(err) => err.close_reason(),
			// LinkError::Accept(err) => err.close_reason(),
			LinkError::Recv(err) => err.close_reason(),
			// LinkError::Write(err) => err.close_reason(),
			_ => None,
		}
	}
}

impl From<OpenError> for LinkError {
	fn from(err: OpenError) -> Self {
		match err {
			OpenError::Cancelled => LinkError::Cancelled,
			error => LinkError::Open(error),
		}
	}
}

impl From<AcceptError> for LinkError {
	fn from(err: AcceptError) -> Self {
		match err {
			AcceptError::Cancelled => LinkError::Cancelled,
			error => LinkError::Accept(error),
		}
	}
}

impl From<RecvError> for LinkError {
	fn from(err: RecvError) -> Self {
		match err {
			RecvError::Cancelled => LinkError::Cancelled,
			error => LinkError::Recv(error),
		}
	}
}

impl From<SendError> for LinkError {
	fn from(err: SendError) -> Self {
		match err {
			SendError::Cancelled => LinkError::Cancelled,
			error => LinkError::Write(error),
		}
	}
}

#[derive(
	Debug, Clone, Copy, IntoStaticStr, AsRefStr, FromRepr, thiserror::Error,
)]
#[repr(u32)]
pub enum SystemReason {
	/// Protocol ran to completion successfully.
	#[error("success")]
	Completed = 10,

	/// The accepting socket was expecting a different protocol ALPN.
	#[error("invalid alpn")]
	InvalidAlpn = 100,

	#[error("operation cancelled")]
	Cancelled = 200,

	#[error("unexpected close")]
	UnexpectedClose = 300,

	/// The remote peer sent a message that violates the protocol, e.g.
	/// deserialization of its contents by the recipient failed.
	#[error("protocol violation")]
	ProtocolViolation = 400,
}

impl From<ConnectionError> for OpenError {
	fn from(err: ConnectionError) -> Self {
		OpenError::Io(ConnectError::Connection {
			source: err,
			meta: Meta::default(),
		})
	}
}

impl OpenError {
	/// If the connection was closed with an application-level close frame,
	/// returns the associated `ApplicationClose` reason. Otherwise returns
	/// `None`.
	pub fn close_reason(&self) -> Option<&ApplicationClose> {
		match self {
			OpenError::Io(
				ConnectError::Connecting {
					source:
						ConnectingError::ConnectionError {
							source: ConnectionError::ApplicationClosed(reason),
							..
						},
					..
				}
				| ConnectError::Connection {
					source: ConnectionError::ApplicationClosed(reason),
					..
				},
			) => Some(reason),
			_ => None,
		}
	}

	/// Returns `true` if the error indicates that the operation was locally
	/// cancelled.
	pub fn is_cancelled(&self) -> bool {
		matches!(self, OpenError::Cancelled)
	}
}

impl From<ConnectionError> for AcceptError {
	fn from(err: ConnectionError) -> Self {
		AcceptError::Io(IrohAcceptError::from(err))
	}
}

impl<P: Protocol> fmt::Debug for Link<P> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Link")
			.field("alpn", &String::from_utf8_lossy(self.alpn()))
			.field("remote_id", &self.remote_id())
			.finish_non_exhaustive()
	}
}

impl<P: Protocol> fmt::Display for Link<P> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"Link<{}:{}>",
			String::from_utf8_lossy(self.alpn()),
			Short(self.remote_id())
		)
	}
}

impl SystemReason {
	pub fn error_code(self) -> VarInt {
		VarInt::from(self as u32)
	}

	pub fn reason(self) -> &'static [u8] {
		let bytes: &'static str = self.into();
		bytes.as_bytes()
	}
}

impl From<SystemReason> for &'static [u8] {
	fn from(val: SystemReason) -> Self {
		let bytes: &'static str = val.into();
		bytes.as_bytes()
	}
}

impl From<SystemReason> for ApplicationClose {
	fn from(val: SystemReason) -> Self {
		let reason: &'static [u8] = val.into();
		ApplicationClose {
			error_code: VarInt::from(val as u32),
			reason: reason.into(),
		}
	}
}

impl PartialEq<ApplicationClose> for SystemReason {
	fn eq(&self, other: &ApplicationClose) -> bool {
		let this_code = VarInt::from(*self as u32);
		let this_reason = self.as_ref();
		this_code == other.error_code && this_reason == other.reason
	}
}

impl PartialEq<SystemReason> for ApplicationClose {
	fn eq(&self, other: &SystemReason) -> bool {
		other == self
	}
}
