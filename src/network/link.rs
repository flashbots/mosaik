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
	iroh::{EndpointAddr, endpoint::*, protocol::AcceptError as IrohAcceptError},
	n0_error::Meta,
	serde::{Serialize, de::DeserializeOwned},
	std::io,
	tokio::io::{Join, join},
	tokio_util::{
		codec::{Framed, LengthDelimitedCodec},
		sync::CancellationToken,
	},
};

/// Protocol trait for defining ALPN identifiers for network protocols.
///
/// This trait ensures that a given [`Link`] instance is always associated with
/// a specific application-level protocol and uses the correct ALPN identifier.
pub trait Protocol {
	const ALPN: &'static [u8];
}

/// Represents a transport level open bidirectional stream between two peers
/// using a specific application-level protocol.
///
/// Notes:
///
/// - A link can be instantiated either by accepting an incoming connection or
///   by opening an outgoing connection to a remote peer.
///
/// - This is where the framing semantics are defined. We use [`Framed`] with
///   [`LengthDelimitedCodec`] to frame individual messages on the wire.
///
/// - All data sent through the link is serialized and deserialized using
///   [`bincode`] with the standard configuration. Any failure to deserialize
///   incoming data results in closing the link as it is considered a protocol
///   violation.
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
	pub async fn accept_with_cancel(
		connection: Connection,
		cancel: CancellationToken,
	) -> Result<Self, AcceptError> {
		// reject any connection that does not match the expected typed ALPN
		if P::ALPN != connection.alpn() {
			let alpn = connection.alpn().to_vec();

			// close the connection with invalid alpn reason, the remote peer should
			// receive this reason as part of the application close frame.
			if let Some(reason) = close_connection(&connection, InvalidAlpn).await {
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
			close_connection(&connection, Cancelled).await;
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
	#[allow(unused)]
	pub async fn accept(connection: Connection) -> Result<Self, AcceptError> {
		Self::accept_with_cancel(connection, CancellationToken::new()).await
	}

	/// Initiates a new outgoing connection to a remote peer, opens a
	/// bidirectional stream and initializes message framing.
	///
	/// This version of `open` accepts an external cancellation token to allow
	/// cancelling any pending link operations.
	///
	/// Note: the peer that calls `open` must send the first message before the
	/// peer that `accept`s can begin accepting the connection.
	pub async fn open_with_cancel(
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
			close_connection(&connection, Cancelled).await;
			return Err(OpenError::Cancelled);
		};

		let (tx, rx) = match open_result {
			Ok(streams) => streams,
			Err(err) => {
				close_connection(&connection, Cancelled).await;
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
	#[allow(unused)]
	pub async fn open(
		local: &LocalNode,
		remote: impl Into<EndpointAddr>,
	) -> Result<Self, OpenError> {
		Self::open_with_cancel(local, remote, CancellationToken::new()).await
	}

	/// Returns the ALPN identifier for this link.
	#[expect(clippy::unused_self)]
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
	#[expect(unused)]
	pub fn is_cancelled(&self) -> bool {
		self.cancel.is_cancelled()
	}

	/// Receives the next framed message and deserializes it into the given
	/// data type `D` using bincode deserialization.
	pub async fn recv<D: DeserializeOwned>(&mut self) -> Result<D, RecvError> {
		let Some(frame) = self.cancel.run_until_cancelled(self.stream.next()).await
		else {
			close_connection(&self.connection, Cancelled).await;
			return Err(RecvError::Cancelled);
		};

		let Some(read_result) = frame else {
			let Some(reason) =
				close_connection(&self.connection, UnexpectedClose).await
			else {
				return Err(RecvError::closed(UnexpectedClose));
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
				close_connection(&self.connection, ProtocolViolation).await;
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
			close_connection(&self.connection, ProtocolViolation).await;
			return Err(e.into());
		}
		let fut = self.stream.send(writer.into_inner().freeze());
		let Some(send_result) = self.cancel.run_until_cancelled(fut).await else {
			close_connection(&self.connection, Cancelled).await;
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
	) -> Result<(), CloseError> {
		// flush any pending outgoing data before closing and receive any incoming
		// close frames.
		let _ = self.stream.flush().await;
		let _ = self.stream.close().await;

		// link already closed, return the existing reason as sent by the remote
		// peer or generated by the network layer
		if let Some(reason) = self.connection().close_reason() {
			return Err(CloseError::AlreadyClosed(reason));
		}

		// otherwise send the close frame to the remote peer
		let reason: ApplicationClose = reason.into();
		self.connection().close(reason.error_code, &reason.reason);

		// await the link closure confirmation and return any error reason
		// other than locally closed.
		let close_result = self
			.cancel
			.run_until_cancelled(self.connection().closed())
			.await;

		match close_result {
			// We don't know the state of the connection as it was cancelled.
			None => Err(CloseError::Cancelled),
			// the connection was open and it was closed successfully by this peer.
			Some(ConnectionError::LocallyClosed) => Ok(()),
			// the connection closed for some other reason than locally closed.
			Some(reason) => Err(CloseError::UnexpectedReason(reason)),
		}
	}

	/// Awaits the link closure and returns the closure result if the link was
	/// closed for a reason not indicating success.
	pub async fn closed(self) -> Result<(), ConnectionError> {
		match self
			.cancel
			.run_until_cancelled(self.connection.closed())
			.await
		{
			None | Some(ConnectionError::LocallyClosed) => Ok(()),
			Some(ConnectionError::ApplicationClosed(reason)) if reason == Success => {
				Ok(())
			}
			Some(err) => Err(err),
		}
	}
}

/// Closes the given connection with the specified reason and awaits the
/// closure confirmation. If the connection was already closed, then the
/// existing closure error is returned.
async fn close_connection(
	connection: &Connection,
	reason: impl Into<ApplicationClose>,
) -> Option<ConnectionError> {
	let reason = reason.into();
	connection.close(reason.error_code, &reason.reason);
	match connection.closed().await {
		ConnectionError::LocallyClosed => None,
		err => Some(err),
	}
}

/// Errors that can occur when working with a link.
///
/// This error type covers all operations on a link, including opening,
/// accepting, sending, and receiving data. It also includes a `Cancelled`
/// variant to indicate that an operation was cancelled.
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

	#[error("Link closed with unexpected reason: {0}")]
	Close(CloseError),

	#[error("Operation cancelled")]
	Cancelled,
}

/// Errors that occur on the connection initiating side when opening a link
/// to a remote peer.
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

/// Errors that can occur when sending data over a link.
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

/// Errors that can occur when closing a link.
#[derive(Debug, thiserror::Error)]
pub enum CloseError {
	#[error("Connection already closed: {0}")]
	AlreadyClosed(ConnectionError),

	#[error("Connection closed with unexpected reason: {0}")]
	UnexpectedReason(ConnectionError),

	#[error("Operation cancelled")]
	Cancelled,
}

impl AcceptError {
	/// If the connection was closed with an application-level close frame,
	/// returns the associated `ApplicationClose` reason. Otherwise returns
	/// `None`.
	pub fn close_reason(&self) -> Option<&ApplicationClose> {
		match self {
			AcceptError::Io(
				IrohAcceptError::Connecting {
					source:
						ConnectingError::ConnectionError {
							source: ConnectionError::ApplicationClosed(reason),
							..
						},
					..
				}
				| IrohAcceptError::Connection {
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
		matches!(self, AcceptError::Cancelled)
	}
}

impl From<CloseError> for AcceptError {
	fn from(err: CloseError) -> Self {
		match err {
			CloseError::Cancelled => AcceptError::Cancelled,
			error @ CloseError::UnexpectedReason(_) => {
				AcceptError::Io(IrohAcceptError::from_err(error))
			}
			CloseError::AlreadyClosed(_) => {
				AcceptError::Io(IrohAcceptError::from_err(err))
			}
		}
	}
}

impl From<ApplicationClose> for RecvError {
	fn from(val: ApplicationClose) -> Self {
		RecvError::Io(ReadError::ConnectionLost(
			ConnectionError::ApplicationClosed(val),
		))
	}
}

impl RecvError {
	pub fn closed(reason: impl Into<ApplicationClose>) -> Self {
		RecvError::from(reason.into())
	}

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

impl SendError {
	/// If the connection was closed with an application-level close frame,
	/// returns the associated `ApplicationClose` reason. Otherwise returns
	/// `None`.
	pub fn close_reason(&self) -> Option<&ApplicationClose> {
		match self {
			SendError::Io(WriteError::ConnectionLost(
				ConnectionError::ApplicationClosed(reason),
			)) => Some(reason),
			_ => None,
		}
	}

	/// Returns `true` if the error indicates that the operation was locally
	/// cancelled.
	pub fn is_cancelled(&self) -> bool {
		matches!(self, SendError::Cancelled)
	}
}

impl From<AcceptError> for IrohAcceptError {
	fn from(err: AcceptError) -> Self {
		match err {
			AcceptError::Io(e) => e,
			error => IrohAcceptError::from_err(error),
		}
	}
}

impl CloseError {
	pub fn close_reason(&self) -> Option<&ApplicationClose> {
		match self {
			CloseError::UnexpectedReason(ConnectionError::ApplicationClosed(
				reason,
			)) => Some(reason),
			_ => None,
		}
	}

	/// Returns `true` if the error indicates that the operation was locally
	/// cancelled.
	pub fn is_cancelled(&self) -> bool {
		matches!(self, CloseError::Cancelled)
	}
}

impl LinkError {
	/// Returns `true` if the error indicates that the operation was locally
	/// cancelled.
	pub fn is_cancelled(&self) -> bool {
		match self {
			LinkError::Open(err) => err.is_cancelled(),
			LinkError::Accept(err) => err.is_cancelled(),
			LinkError::Recv(err) => err.is_cancelled(),
			LinkError::Write(err) => err.is_cancelled(),
			LinkError::Close(err) => err.is_cancelled(),
			LinkError::Cancelled => true,
		}
	}

	pub fn close_reason(&self) -> Option<&ApplicationClose> {
		match self {
			LinkError::Accept(err) => err.close_reason(),
			LinkError::Open(err) => err.close_reason(),
			LinkError::Recv(err) => err.close_reason(),
			LinkError::Write(err) => err.close_reason(),
			LinkError::Close(err) => err.close_reason(),
			LinkError::Cancelled => None,
		}
	}
}

impl From<OpenError> for LinkError {
	fn from(err: OpenError) -> Self {
		match err {
			OpenError::Cancelled => LinkError::Cancelled,
			error @ OpenError::Io(_) => LinkError::Open(error),
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

macro_rules! make_close_reason {
	($(#[$meta:meta])* $vis:vis struct $name:ident, $code:expr) => {
		$(#[$meta])*
		#[derive(Debug, Clone, Copy)]
		$vis struct $name;

		const _: () = {
			#[automatically_derived]
			impl From<$name> for iroh::endpoint::ApplicationClose {
				fn from(_: $name) -> Self {
					iroh::endpoint::ApplicationClose {
						error_code: iroh::endpoint::VarInt::from($code as u32),
						reason: stringify!($name).into(),
					}
				}
			}

			#[automatically_derived]
			impl PartialEq<iroh::endpoint::ApplicationClose> for $name {
				fn eq(&self, other: &iroh::endpoint::ApplicationClose) -> bool {
					other.error_code == iroh::endpoint::VarInt::from($code as u32)
				}
			}

			#[automatically_derived]
			impl PartialEq<$name> for iroh::endpoint::ApplicationClose {
				fn eq(&self, _: &$name) -> bool {
					self.error_code == iroh::endpoint::VarInt::from($code as u32)
				}
			}
		};
	};
}

pub(crate) use make_close_reason;

// Standardized application-level close reasons for links and protocols in
// mosaik. Close reasons 0-199 are reserved for mosaik internal use.

make_close_reason!(
	/// Protocol ran to completion successfully.
	pub(crate) struct Success, 0);

make_close_reason!(
	/// The accepting socket was expecting a different protocol ALPN.
	pub(crate) struct InvalidAlpn, 100);

make_close_reason!(
	/// Operation was cancelled.
	pub(crate) struct Cancelled, 101);

make_close_reason!(
	/// The connection was closed unexpectedly.
	pub(crate) struct UnexpectedClose, 102);

make_close_reason!(
	/// The remote peer sent a message that violates the protocol.
	pub(crate) struct ProtocolViolation, 103);
