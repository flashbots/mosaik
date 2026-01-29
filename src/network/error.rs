use crate::{discovery, groups, store, streams};

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("Missing network ID")]
	MissingNetworkId,

	#[error("Bind error: {0}")]
	Bind(#[from] iroh::endpoint::BindError),

	#[error("Discovery config error: {0}")]
	DiscoveryConfig(#[from] discovery::ConfigBuilderError),

	#[error("Streams config error: {0}")]
	StreamsConfig(#[from] streams::ConfigBuilderError),

	#[error("Groups config error: {0}")]
	GroupsConfig(#[from] groups::ConfigBuilderError),

	#[error("Stores config error: {0}")]
	StoreConfig(#[from] store::ConfigBuilderError),
}

pub trait CloseReason:
	std::error::Error
	+ Into<iroh::endpoint::ApplicationClose>
	+ PartialEq<iroh::endpoint::ApplicationClose>
	+ Clone
	+ Sized
	+ Send
	+ Sync
	+ 'static
{
}

macro_rules! make_close_reason {
	($(#[$meta:meta])* $vis:vis struct $name:ident, $code:expr) => {
		$(#[$meta])*
		#[derive(Debug, Clone, Copy, thiserror::Error)]
		#[error("{}", stringify!($name))]
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

			#[automatically_derived]
			impl PartialEq<$name> for &iroh::endpoint::ApplicationClose {
				fn eq(&self, _: &$name) -> bool {
					self.error_code == iroh::endpoint::VarInt::from($code as u32)
				}
			}

			#[automatically_derived]
			impl crate::network::error::CloseReason for $name { }
		};
	};
}

pub(crate) use make_close_reason;

// Standardized application-level close reasons for links and protocols in
// mosaik. Close reasons 0-199 are reserved for mosaik internal use.

make_close_reason!(
	/// Protocol ran to completion successfully.
	pub(crate) struct Success, 200);

make_close_reason!(
	/// Graceful shutdown initiated.
	pub(crate) struct GracefulShutdown, 204);

make_close_reason!(
	/// The accepting socket was expecting a different protocol ALPN.
	pub(crate) struct InvalidAlpn, 100);

make_close_reason!(
	/// The remote peer is on a different network.
	pub(crate) struct DifferentNetwork, 101);

make_close_reason!(
	/// Operation was cancelled.
	pub(crate) struct Cancelled, 102);

make_close_reason!(
	/// The connection was closed unexpectedly.
	pub(crate) struct UnexpectedClose, 103);

make_close_reason!(
	/// The remote peer sent a message that violates the protocol.
	pub(crate) struct ProtocolViolation, 400);

make_close_reason!(
	/// A remote peer is trying to connect but is not known in the discovery catalog and the
	/// protocol requires knowledge of the peer. Indicates that the connecting peer should re-sync
	/// its catalog and retry the connection.
	pub(crate) struct UnknownPeer, 401);
