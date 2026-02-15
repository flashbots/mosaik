use {
	super::{
		LocalNode,
		PeerId,
		error::{
			Cancelled,
			InvalidAlpn,
			ProtocolViolation,
			Success,
			UnexpectedClose,
		},
	},
	crate::{
		Digest,
		primitives::{Bytes, Short, deserialize, serialize},
	},
	core::{fmt, marker::PhantomData},
	futures::{FutureExt, SinkExt, StreamExt},
	iroh::{EndpointAddr, endpoint::*, protocol::AcceptError as IrohAcceptError},
	n0_error::Meta,
	serde::{Serialize, de::DeserializeOwned},
	std::io,
	tokio_util::{
		codec::{FramedRead, FramedWrite, LengthDelimitedCodec},
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
///   by opening a new connection to a remote peer.
///
/// - This is where the framing semantics are defined. We use [`Framed`] with
///   [`LengthDelimitedCodec`] to frame individual messages on the wire.
///
/// - All data sent through the link is serialized and deserialized using
///   [`postcard`]. Any failure to deserialize incoming data results in closing
///   the link as it is considered a protocol violation.
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
	sender: FramedWrite<SendStream, LengthDelimitedCodec>,
	receiver: FramedRead<RecvStream, LengthDelimitedCodec>,
	_protocol: PhantomData<P>,
}

// Public API
impl<P: Protocol> Link<P> {
	/// Reconciles a sender and receiver into a bidirectional link.
	pub fn join(sender: LinkSender<P>, receiver: LinkReceiver<P>) -> Self {
		assert_eq!(
			sender.connection.stable_id(),
			receiver.connection.stable_id(),
			"sender and receiver must belong to the same connection",
		);

		Self {
			connection: sender.connection,
			sender: sender.sender,
			receiver: receiver.receiver,
			cancel: sender.cancel,
			_protocol: PhantomData,
		}
	}

	/// Splits the link into a sender and receiver half.
	pub fn split(self) -> (LinkSender<P>, LinkReceiver<P>) {
		let Self {
			connection,
			sender,
			receiver,
			cancel,
			..
		} = self;

		(
			LinkSender {
				connection: connection.clone(),
				sender,
				cancel: cancel.clone(),
				_protocol: PhantomData,
			},
			LinkReceiver {
				connection,
				receiver,
				cancel,
				_protocol: PhantomData,
			},
		)
	}

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
		let sender = FramedWrite::new(tx, LengthDelimitedCodec::new());
		let receiver = FramedRead::new(rx, LengthDelimitedCodec::new());

		Ok(Self {
			connection,
			sender,
			receiver,
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

		let sender = FramedWrite::new(tx, LengthDelimitedCodec::new());
		let receiver = FramedRead::new(rx, LengthDelimitedCodec::new());

		Ok(Self {
			connection,
			sender,
			receiver,
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
	pub const fn alpn(&self) -> &[u8] {
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
	/// data type `D` using postcard deserialization.
	pub async fn recv<D: DeserializeOwned>(&mut self) -> Result<D, RecvError> {
		self.recv_with_size().await.map(|(d, _)| d)
	}

	/// Receives the next framed message and deserializes it into the given data
	/// type `D`, returning a deserialized value along with the size of the
	/// message in bytes.
	pub async fn recv_with_size<D: DeserializeOwned>(
		&mut self,
	) -> Result<(D, usize), RecvError> {
		let Some(frame) =
			self.cancel.run_until_cancelled(self.receiver.next()).await
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

		let bytes = match read_result {
			Ok(bytes) => bytes,
			Err(err) => match err.downcast::<ReadError>() {
				Ok(read_err) => return Err(RecvError::Io(read_err)),
				Err(other_err) => return Err(RecvError::Unknown(other_err)),
			},
		};

		// deserialize the received bytes into the expected data type, if
		// deserialization fails, close the connection with protocol violation
		let decoded = match deserialize(&bytes) {
			Ok(datum) => datum,
			Err(err) => {
				close_connection(&self.connection, ProtocolViolation).await;
				return Err(RecvError::Decode(err));
			}
		};

		Ok((decoded, bytes.len()))
	}

	/// Sends a framed message over the link.
	///
	/// The message is serialized using postcard serialization and sent as a
	/// length-delimited frame.
	///
	/// If the serialization fails, the link is closed with a [`UnexpectedClose`].
	/// Returns the number of bytes sent on success.
	pub async fn send<D: Serialize>(
		&mut self,
		datum: D,
	) -> Result<usize, SendError> {
		// SAFETY: the bytes written into the writer are guaranteed to be
		// well-formed postcard serialized `D`.
		unsafe { self.send_raw(serialize(&datum)).await }
	}

	/// Sends raw bytes over the link without serialization.
	///
	/// It is the caller's responsibility to ensure that the bytes
	/// are properly formatted according to the protocol's expectations.
	///
	/// This variant of `send` is unsafe because it is intended for advanced
	/// use cases where the caller needs to optimize performance by avoiding
	/// serialization overhead. Improper use may lead to protocol violations.
	///
	/// Returns the number of bytes sent on success.
	pub async unsafe fn send_raw(
		&mut self,
		bytes: Bytes,
	) -> Result<usize, SendError> {
		let msg_len = bytes.len();
		let fut = self.sender.send(bytes);
		let Some(send_result) = self.cancel.run_until_cancelled(fut).await else {
			close_connection(&self.connection, Cancelled).await;
			return Err(SendError::Cancelled);
		};

		match send_result {
			Ok(()) => Ok(msg_len),
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
		// link already closed, return the existing reason as sent by the remote
		// peer or generated by the network layer
		if let Some(reason) = self.connection().close_reason() {
			return Err(CloseError::AlreadyClosed(reason));
		}

		// otherwise send the close frame to the remote peer
		let reason: ApplicationClose = reason.into();
		self.connection().close(reason.error_code, &reason.reason);

		// flush any pending outgoing data before closing and receive any incoming
		// close frames.
		let _ = self.cancel.run_until_cancelled(self.sender.flush()).await;
		let _ = self.cancel.run_until_cancelled(self.sender.close()).await;

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
	/// closed for a reason not indicating success. This future can be used to
	/// monitor the link for unexpected closures and its lifetime is detached
	/// from the link itself.
	pub fn closed(
		&self,
	) -> impl Future<Output = Result<(), ConnectionError>> + Send + Sync + 'static
	{
		let cancel = self.cancel.clone();
		let connection = self.connection.clone();
		async move {
			match cancel.run_until_cancelled(connection.closed()).await {
				None | Some(ConnectionError::LocallyClosed) => Ok(()),
				Some(ConnectionError::ApplicationClosed(reason))
					if reason == Success =>
				{
					Ok(())
				}
				Some(err) => Err(err),
			}
		}
		.fuse()
	}

	/// Replaces the existing cancellation token with a new one.
	///
	/// This is useful when the link needs to inherit a more scoped cancellation
	/// token after being created with a general one.
	pub fn replace_cancel_token(&mut self, cancel: CancellationToken) {
		self.cancel = cancel;
	}

	/// Derives keying material from this connection's TLS session secrets.
	///
	/// When both peers call this method with the same `label` and ALPN, they will
	/// get the same [`Digest`]. These bytes are cryptographically
	/// strong and pseudorandom, and are suitable for use as keying material.
	///
	/// See [RFC5705](https://tools.ietf.org/html/rfc5705) for more information.
	pub fn shared_random(&self, label: impl AsRef<[u8]>) -> Digest {
		let mut shared_secret = [0u8; 32];

		self
			.connection()
			.export_keying_material(&mut shared_secret, label.as_ref(), self.alpn())
			.expect("exporting keying material should not fail for this buffer len");

		Digest::from_bytes(shared_secret)
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
	#[error("{0}")]
	Open(OpenError),

	#[error("{0}")]
	Accept(AcceptError),

	#[error("{0}")]
	Recv(RecvError),

	#[error("{0}")]
	Write(SendError),

	#[error("{0}")]
	Close(CloseError),

	#[error("Operation cancelled")]
	Cancelled,
}

/// Errors that occur on the connection initiating side when opening a link
/// to a remote peer.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
	#[error("{0}")]
	Io(#[from] ConnectError),

	#[error("Operation cancelled")]
	Cancelled,
}

/// Errors that occur when accepting a new connection from a remote peer.
#[derive(Debug, thiserror::Error)]
pub enum AcceptError {
	#[error("{0}")]
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
	#[error("{0}")]
	Io(#[from] ReadError),

	/// This error indicates that the data was read successfully but failed to
	/// deserialize it into a typed structure as set in [`Link::recv_as`].
	#[error("{0}")]
	Decode(#[from] postcard::Error),

	#[error("{0}")]
	Unknown(#[from] io::Error),

	#[error("Operation cancelled")]
	Cancelled,
}

/// Errors that can occur when sending data over a link.
#[derive(Debug, thiserror::Error)]
pub enum SendError {
	#[error("{0}")]
	Encode(#[from] postcard::Error),

	#[error("{0}")]
	Io(#[from] WriteError),

	#[error("{0}")]
	Unknown(#[from] io::Error),

	#[error("Operation cancelled")]
	Cancelled,
}

/// Errors that can occur when closing a link.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum CloseError {
	#[error("{0}")]
	AlreadyClosed(ConnectionError),

	#[error("{0}")]
	UnexpectedReason(ConnectionError),

	#[error("Operation cancelled")]
	Cancelled,
}

impl AcceptError {
	/// If the connection was closed with an application-level close frame,
	/// returns the associated `ApplicationClose` reason. Otherwise returns
	/// `None`.
	pub const fn close_reason(&self) -> Option<&ApplicationClose> {
		match self {
			Self::Io(
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
	pub const fn is_cancelled(&self) -> bool {
		matches!(self, Self::Cancelled)
	}
}

impl From<CloseError> for AcceptError {
	fn from(err: CloseError) -> Self {
		match err {
			CloseError::Cancelled => Self::Cancelled,
			error @ CloseError::UnexpectedReason(_) => {
				Self::Io(IrohAcceptError::from_err(error))
			}
			CloseError::AlreadyClosed(_) => Self::Io(IrohAcceptError::from_err(err)),
		}
	}
}

impl From<ApplicationClose> for AcceptError {
	fn from(val: ApplicationClose) -> Self {
		Self::Io(ConnectionError::ApplicationClosed(val).into())
	}
}

impl From<ApplicationClose> for RecvError {
	fn from(val: ApplicationClose) -> Self {
		Self::Io(ReadError::ConnectionLost(
			ConnectionError::ApplicationClosed(val),
		))
	}
}

impl From<ApplicationClose> for OpenError {
	fn from(val: ApplicationClose) -> Self {
		Self::Io(ConnectionError::ApplicationClosed(val).into())
	}
}

impl RecvError {
	pub fn closed(reason: impl Into<ApplicationClose>) -> Self {
		Self::from(reason.into())
	}

	/// If the connection was closed with an application-level close frame,
	/// returns the associated `ApplicationClose` reason. Otherwise returns
	/// `None`.
	pub const fn close_reason(&self) -> Option<&ApplicationClose> {
		match self {
			Self::Io(ReadError::ConnectionLost(
				ConnectionError::ApplicationClosed(reason),
			)) => Some(reason),
			_ => None,
		}
	}

	/// Returns `true` if the error indicates that the operation was locally
	/// cancelled.
	pub const fn is_cancelled(&self) -> bool {
		matches!(self, Self::Cancelled)
	}
}

impl From<ConnectionError> for RecvError {
	fn from(err: ConnectionError) -> Self {
		Self::Io(ReadError::ConnectionLost(err))
	}
}

impl SendError {
	/// If the connection was closed with an application-level close frame,
	/// returns the associated `ApplicationClose` reason. Otherwise returns
	/// `None`.
	pub const fn close_reason(&self) -> Option<&ApplicationClose> {
		match self {
			Self::Io(WriteError::ConnectionLost(
				ConnectionError::ApplicationClosed(reason),
			)) => Some(reason),
			_ => None,
		}
	}

	/// Returns `true` if the error indicates that the operation was locally
	/// cancelled.
	pub const fn is_cancelled(&self) -> bool {
		matches!(self, Self::Cancelled)
	}
}

impl From<AcceptError> for IrohAcceptError {
	fn from(err: AcceptError) -> Self {
		match err {
			AcceptError::Io(e) => e,
			error => Self::from_err(error),
		}
	}
}

impl CloseError {
	pub const fn close_reason(&self) -> Option<&ApplicationClose> {
		match self {
			Self::UnexpectedReason(ConnectionError::ApplicationClosed(reason)) => {
				Some(reason)
			}
			_ => None,
		}
	}

	/// Returns `true` if the error indicates that the operation was locally
	/// cancelled.
	pub const fn is_cancelled(&self) -> bool {
		matches!(self, Self::Cancelled)
	}

	/// Returns `true` if the link was already closed when attempting to close
	/// it.
	pub const fn was_already_closed(&self) -> bool {
		matches!(self, Self::AlreadyClosed(_))
	}
}

impl LinkError {
	/// Returns `true` if the error indicates that the operation was locally
	/// cancelled.
	pub const fn is_cancelled(&self) -> bool {
		match self {
			Self::Open(err) => err.is_cancelled(),
			Self::Accept(err) => err.is_cancelled(),
			Self::Recv(err) => err.is_cancelled(),
			Self::Write(err) => err.is_cancelled(),
			Self::Close(err) => err.is_cancelled(),
			Self::Cancelled => true,
		}
	}

	pub const fn close_reason(&self) -> Option<&ApplicationClose> {
		match self {
			Self::Accept(err) => err.close_reason(),
			Self::Open(err) => err.close_reason(),
			Self::Recv(err) => err.close_reason(),
			Self::Write(err) => err.close_reason(),
			Self::Close(err) => err.close_reason(),
			Self::Cancelled => None,
		}
	}
}

impl From<OpenError> for LinkError {
	fn from(err: OpenError) -> Self {
		match err {
			OpenError::Cancelled => Self::Cancelled,
			error @ OpenError::Io(_) => Self::Open(error),
		}
	}
}

impl From<AcceptError> for LinkError {
	fn from(err: AcceptError) -> Self {
		match err {
			AcceptError::Cancelled => Self::Cancelled,
			error => Self::Accept(error),
		}
	}
}

impl From<RecvError> for LinkError {
	fn from(err: RecvError) -> Self {
		match err {
			RecvError::Cancelled => Self::Cancelled,
			error => Self::Recv(error),
		}
	}
}

impl From<SendError> for LinkError {
	fn from(err: SendError) -> Self {
		match err {
			SendError::Cancelled => Self::Cancelled,
			error => Self::Write(error),
		}
	}
}

impl From<CloseError> for LinkError {
	fn from(err: CloseError) -> Self {
		match err {
			CloseError::Cancelled => Self::Cancelled,
			error => Self::Close(error),
		}
	}
}

impl From<ConnectionError> for OpenError {
	fn from(err: ConnectionError) -> Self {
		Self::Io(ConnectError::Connection {
			source: err,
			meta: Meta::default(),
		})
	}
}

impl OpenError {
	/// If the connection was closed with an application-level close frame,
	/// returns the associated `ApplicationClose` reason. Otherwise returns
	/// `None`.
	pub const fn close_reason(&self) -> Option<&ApplicationClose> {
		match self {
			Self::Io(
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
	pub const fn is_cancelled(&self) -> bool {
		matches!(self, Self::Cancelled)
	}
}

impl From<ConnectionError> for AcceptError {
	fn from(err: ConnectionError) -> Self {
		Self::Io(IrohAcceptError::from(err))
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

pub struct LinkSender<P: Protocol> {
	connection: Connection,
	cancel: CancellationToken,
	sender: FramedWrite<SendStream, LengthDelimitedCodec>,
	_protocol: PhantomData<P>,
}

impl<P: Protocol> LinkSender<P> {
	/// Sends a framed message over the link.
	///
	/// The message is serialized using postcard serialization and sent as a
	/// length-delimited frame.
	///
	/// If the serialization fails, the link is closed with a [`UnexpectedClose`].
	/// Returns the number of bytes sent on success.
	pub async fn send<D: Serialize>(
		&mut self,
		datum: D,
	) -> Result<usize, SendError> {
		// SAFETY: the bytes written into the writer are guaranteed to be
		// well-formed postcard serialized `D`.
		unsafe { self.send_raw(serialize(&datum)).await }
	}

	/// Sends raw bytes over the link without serialization.
	///
	/// It is the caller's responsibility to ensure that the bytes
	/// are properly formatted according to the protocol's expectations.
	///
	/// This variant of `send` is unsafe because it is intended for advanced
	/// use cases where the caller needs to optimize performance by avoiding
	/// serialization overhead. Improper use may lead to protocol violations.
	///
	/// Returns the number of bytes sent on success.
	pub async unsafe fn send_raw(
		&mut self,
		bytes: Bytes,
	) -> Result<usize, SendError> {
		let msg_len = bytes.len();
		let fut = self.sender.send(bytes);
		let Some(send_result) = self.cancel.run_until_cancelled(fut).await else {
			close_connection(&self.connection, Cancelled).await;
			return Err(SendError::Cancelled);
		};

		match send_result {
			Ok(()) => Ok(msg_len),
			Err(err) => match err.downcast::<WriteError>() {
				Ok(io_err) => Err(SendError::Io(io_err)),
				Err(other_err) => Err(SendError::Unknown(other_err)),
			},
		}
	}
}

pub struct LinkReceiver<P: Protocol> {
	connection: Connection,
	cancel: CancellationToken,
	receiver: FramedRead<RecvStream, LengthDelimitedCodec>,
	_protocol: PhantomData<P>,
}

impl<P: Protocol> LinkReceiver<P> {
	/// Receives the next framed message and deserializes it into the given
	/// data type `D` using postcard deserialization.
	pub async fn recv<D: DeserializeOwned>(&mut self) -> Result<D, RecvError> {
		self.recv_with_size().await.map(|(d, _)| d)
	}

	/// Receives the next framed message and deserializes it into the given data
	/// type `D`, returning a deserialized value along with the size of the
	/// message in bytes.
	pub async fn recv_with_size<D: DeserializeOwned>(
		&mut self,
	) -> Result<(D, usize), RecvError> {
		let Some(frame) =
			self.cancel.run_until_cancelled(self.receiver.next()).await
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

		let bytes = match read_result {
			Ok(bytes) => bytes,
			Err(err) => match err.downcast::<ReadError>() {
				Ok(read_err) => return Err(RecvError::Io(read_err)),
				Err(other_err) => return Err(RecvError::Unknown(other_err)),
			},
		};

		// deserialize the received bytes into the expected data type, if
		// deserialization fails, close the connection with protocol violation
		let decoded = match deserialize(&bytes) {
			Ok(datum) => datum,
			Err(err) => {
				close_connection(&self.connection, ProtocolViolation).await;
				return Err(RecvError::Decode(err));
			}
		};

		Ok((decoded, bytes.len()))
	}
}
