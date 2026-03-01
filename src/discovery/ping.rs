use {
	super::{Catalog, Handle},
	crate::{
		network::{
			LocalNode,
			link::{Link, Protocol},
		},
		primitives::Short,
	},
	core::fmt::{self, Debug},
	iroh::{
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler},
	},
	tokio::sync::watch,
};

/// Node Status Ping Protocol
///
/// This protocol is responsible for testing the liveness of a peer and
/// retrieving its current discovery information.
pub struct Ping {
	local: LocalNode,
	catalog: watch::Sender<Catalog>,
}

impl Ping {
	pub(super) fn new(handle: &Handle) -> Self {
		Self {
			local: handle.local.clone(),
			catalog: handle.catalog.clone(),
		}
	}

	/// Returns the protocol listener instance for accepting incoming ping
	/// requests from remote peers.
	pub const fn protocol(&self) -> &impl ProtocolHandler {
		self
	}
}

impl Protocol for Ping {
	/// ALPN identifier for the ping protocol.
	const ALPN: &'static [u8] = b"/mosaik/discovery/ping/1.0";
}

impl ProtocolHandler for Ping {
	fn accept(
		&self,
		connection: Connection,
	) -> impl Future<Output = Result<(), AcceptError>> + Send {
		let cancel = self.local.termination().child_token();
		let catalog = self.catalog.clone();

		async move {
			let remote_id = connection.remote_id();
			let mut link = Link::<Self>::accept_with_cancel(connection, cancel)
				.await
				.inspect_err(|e| {
					tracing::trace!(
						error = %e,
						peer = %Short(remote_id),
						"failed to accept incoming ping query"
					);
				})?;

			let _: () = link
				.recv()
				.await
				.inspect_err(|e| {
					tracing::trace!(
						error = %e,
						peer = %Short(remote_id),
						"failed to receive ping query"
					);
				})
				.map_err(AcceptError::from_err)?;

			let me = catalog.borrow().local().clone();

			link
				.send(&me)
				.await
				.inspect_err(|e| {
					tracing::trace!(
						error = %e,
						peer = %Short(remote_id),
						"failed to respond to ping"
					);
				})
				.map_err(AcceptError::from_err)?;

			if let Err(e) = link.closed().await {
				tracing::trace!(
					error = %e,
					peer = %Short(remote_id),
					"ping link closed with error"
				);
			}

			Ok(())
		}
	}
}

impl Debug for Ping {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "Ping(network_id={})", Short(self.local.network_id()))
	}
}
