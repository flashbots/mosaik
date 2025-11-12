use {
	super::{Error, catalog::Catalog},
	crate::local::Local,
	core::fmt,
	iroh::{
		EndpointAddr,
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler},
	},
	tracing::info,
};

#[derive(Clone)]
pub struct Protocol {
	local: Local,
	catalog: Catalog,
}

impl Protocol {
	pub const ALPN: &'static [u8] = b"/mosaik/discovery/1";

	pub fn new(local: Local, catalog: Catalog) -> Self {
		Self { local, catalog }
	}

	pub async fn dial(&self, peer: EndpointAddr) -> Result<(), Error> {
		info!(?peer, "Dialing peer at address for discovery");

		Ok(())
	}
}

impl ProtocolHandler for Protocol {
	async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
		info!(peer = ?connection.remote_id()?,
			"Received discovery connection",
		);
		Ok(())
	}
}

impl fmt::Debug for Protocol {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "Discovery({})", String::from_utf8_lossy(Self::ALPN))
	}
}
