use {
	super::{Error, catalog::Catalog},
	crate::{
		discovery::message::{self, CatalogHash},
		local::Local,
		prelude::PeerInfo,
	},
	core::fmt,
	iroh::{
		Endpoint,
		EndpointAddr,
		endpoint::{Connection, ReadError, ReadToEndError},
		protocol::{AcceptError, ProtocolHandler},
	},
	n0_error::AnyError,
	tracing::info,
};

// TODO: arbitrary right now, decide on a proper limit based on expected message
// type
const MAX_MESSAGE_SIZE: usize = 1024 * 16; // 16 KiB

#[derive(Clone)]
pub(crate) struct Protocol {
	local: Local,
	catalog: Catalog,
}

impl Protocol {
	pub(crate) const ALPN: &'static [u8] = b"/mosaik/discovery/1";

	pub(crate) fn new(local: Local, catalog: Catalog) -> Self {
		Self { local, catalog }
	}
}

impl ProtocolHandler for Protocol {
	async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
		info!(peer = ?connection.remote_id(),
			"Received discovery connection",
		);
		let endpoint_id = connection.remote_id();
		let (mut send, mut recv) = connection.accept_bi().await?;

		// 1. receive peer's CatalogHash
		let msg = recv.read_to_end(MAX_MESSAGE_SIZE).await.map_err(|e| {
			AcceptError::from_err(AnyError::from_string(format!(
				"failed to read CatalogHash from {endpoint_id}: {e}"
			)))
		})?;
		let peer_catalog_hash = CatalogHash::from_bytes(&msg).map_err(|e| {
			AcceptError::from_err(AnyError::from_string(format!(
				"failed to deserialize CatalogHash: {e}"
			)))
		})?;

		// 2. compare with our catalog hash
		let our_catalog_hash = self.catalog.hash().await;
		if peer_catalog_hash.hash() != our_catalog_hash {
			// 3. if catalogs differ, send our full catalog
			// TODO: this is unoptimized, implement full catalog sync algo later
			let catalog: message::Catalog =
				self.catalog.peers().collect::<Vec<PeerInfo>>().into();
			send.write_all(&catalog.into_bytes()).await.map_err(|e| {
				AcceptError::from_err(AnyError::from_string(format!(
					"failed to send Catalog to {endpoint_id}: {e}"
				)))
			})?;
		}

		send.finish().map_err(|e| {
			AcceptError::from_err(AnyError::from_string(format!(
				"failed to finish sending to {endpoint_id}: {e}"
			)))
		})?;

		connection.closed().await;
		Ok(())
	}
}

impl fmt::Debug for Protocol {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "Discovery({})", String::from_utf8_lossy(Self::ALPN))
	}
}

pub(crate) async fn do_catalog_sync(
	peer: EndpointAddr,
	endpoint: &Endpoint,
	catalog: &mut Catalog,
) -> Result<(), Error> {
	info!(?peer, "Dialing peer at address for catalog sync");
	let conn = endpoint
		.connect(peer, Protocol::ALPN)
		.await
		.map_err(Error::dial)?;
	let (mut send, mut recv) = conn.open_bi().await.map_err(Error::connection)?;
	let our_catalog_hash: CatalogHash = catalog.hash().await.into();
	send
		.write_all(&our_catalog_hash.into_bytes())
		.await
		.map_err(Error::write)?;
	let _ = send.finish();

	// if stream closes, then our hashes are the same (?)
	let resp = match recv.read_to_end(MAX_MESSAGE_SIZE).await {
		Ok(data) => data,
		Err(ReadToEndError::Read(ReadError::ClosedStream)) => {
			// peer closed the stream, assume catalogs are the same
			return Ok(());
		}
		Err(e) => {
			return Err(Error::read_to_end(e));
		}
	};

	if resp.is_empty() {
		// peer sent no data, assume catalogs are the same
		return Ok(());
	}

	let peer_catalog =
		message::Catalog::from_bytes(&resp).map_err(Error::invalid_message)?;
	catalog.merge(peer_catalog.peers()).await;
	conn.close(0u32.into(), b"done");
	Ok(())
}
