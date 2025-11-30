use {
	super::Streams,
	core::fmt,
	iroh::{
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler},
	},
};

/// Protocol Acceptor
pub(super) struct Listener;

impl fmt::Debug for Listener {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		// Safety: ALPN is valid UTF-8 hardcoded at compile time
		unsafe { write!(f, "{}", str::from_utf8_unchecked(Streams::ALPN)) }
	}
}

impl ProtocolHandler for Listener {
	async fn accept(&self, _connection: Connection) -> Result<(), AcceptError> {
		todo!()
	}
}
