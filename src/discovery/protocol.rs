use {
	super::{Error, catalog::Catalog},
	crate::{
		discovery::message::{
			self,
			CatalogHashCompareRequest,
			CatalogHashCompareResponse,
			DiscoveryMessage,
		},
		local::Local,
		prelude::PeerInfo,
	},
	core::fmt,
	iroh::{
		Endpoint,
		EndpointAddr,
		endpoint::Connection,
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

		// 1. receive peer's [`CatalogHashCompareRequest`]
		let msg = recv.read_to_end(MAX_MESSAGE_SIZE).await.map_err(|e| {
			AcceptError::from_err(AnyError::from_string(format!(
				"failed to read DiscoveryMessage from {endpoint_id}: {e}"
			)))
		})?;
		let DiscoveryMessage::CatalogHashCompareRequest(peer_catalog_hash) =
			DiscoveryMessage::from_bytes(&msg).map_err(|e| {
				AcceptError::from_err(AnyError::from_string(format!(
					"failed to deserialize DiscoveryMessage: {e}"
				)))
			})?
		else {
			return Err(AcceptError::from_err(AnyError::from_string(
				"expected CatalogHashCompareRequest message".to_string(),
			)));
		};

		// 2. compare with our catalog hash
		let our_catalog_hash = self.catalog.hash().await;
		if peer_catalog_hash.hash() != our_catalog_hash {
			// 3. if catalogs differ, send our full catalog
			// TODO: this is unoptimized, implement full catalog sync algo later
			let catalog: message::Catalog =
				self.catalog.peers().collect::<Vec<PeerInfo>>().into();
			let message: DiscoveryMessage =
				CatalogHashCompareResponse::Mismatches(catalog).into();
			send.write_all(&message.into_bytes()).await.map_err(|e| {
				AcceptError::from_err(AnyError::from_string(format!(
					"failed to send CatalogHashCompareResponse to {endpoint_id}: {e}"
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
	let our_catalog_hash: CatalogHashCompareRequest = catalog.hash().await.into();
	let message: DiscoveryMessage = our_catalog_hash.into();
	send
		.write_all(&message.into_bytes())
		.await
		.map_err(Error::write)?;
	let _ = send.finish();

	// if stream closes, then our hashes are the same (?)
	let resp = recv
		.read_to_end(MAX_MESSAGE_SIZE)
		.await
		.map_err(Error::read_to_end)?;
	if resp.is_empty() {
		return Err(Error::EmptyCatalogHashCompareResponse);
	}

	let DiscoveryMessage::CatalogHashCompareResponse(resp) =
		DiscoveryMessage::from_bytes(&resp).map_err(Error::invalid_message)?
	else {
		return Err(Error::InvalidCatalogHashCompareResponse);
	};

	if let CatalogHashCompareResponse::Mismatches(peer_catalog) = resp {
		catalog.merge(peer_catalog.peers());
	}

	conn.close(0u32.into(), b"done");
	Ok(())
}
