use {
	super::{Config, FanoutSink, Streams},
	crate::StreamId,
	core::fmt,
	dashmap::DashMap,
	iroh::{
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler},
	},
	std::sync::Arc,
};

/// Streams protocol acceptor
///
/// This type is responsible for accepting incoming connections from consumers
/// and routing them to the appropriate fanout sink based on the stream id.
pub(super) struct Listener {
	config: Config,
	sinks: Arc<DashMap<StreamId, FanoutSink>>,
}

impl Listener {
	pub(super) fn new(streams: &Streams) -> Self {
		let config = streams.config.clone();
		let sinks = Arc::clone(&streams.sinks);
		Self { config, sinks }
	}
}

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
