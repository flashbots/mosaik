use {
	crate::peer::PeerInfo,
	core::pin::pin,
	futures::StreamExt,
	iroh::{Endpoint, EndpointAddr, Watcher},
	tokio::sync::watch,
	tokio_util::sync::{
		CancellationToken,
		DropGuard,
		WaitForCancellationFutureOwned,
	},
	tracing::{error, info},
};

/// This type maintains information about the local peer.
///
/// It is responsible for:
pub struct Local {
	endpoint: Endpoint,
	latest: watch::Receiver<PeerInfo>,
	_cancel_on_drop: DropGuard,
}

impl Local {
	pub fn new(endpoint: Endpoint) -> Self {
		let initial = PeerInfo {
			address: endpoint.addr(),
			version: Default::default(),
		};

		let cancellation = CancellationToken::new();
		let cancel_signal = cancellation.clone().cancelled_owned();
		let (info_tx, info_rx) = watch::channel(initial.clone());

		tokio::spawn(Self::runloop(
			endpoint.clone(),
			initial,
			info_tx,
			cancel_signal,
		));

		Self {
			endpoint,
			latest: info_rx,
			_cancel_on_drop: cancellation.drop_guard(),
		}
	}

	/// Returns the transport layer endpoint of the local peer.
	pub const fn endpoint(&self) -> &Endpoint {
		&self.endpoint
	}

	/// Returns all known addresses and relay nodes of the local peer.
	pub fn addr(&self) -> EndpointAddr {
		self.endpoint.addr()
	}

	/// Returns the public key of the local peer.
	///
	/// This is a unique global identifier for this peer that should be enough to
	/// identify, discover and connect to it.
	pub fn id(&self) -> iroh::EndpointId {
		self.endpoint.id()
	}

	/// Awaits until the local peer is online.
	///
	/// When it is online it means that it is registered with discovery and can
	/// accept incoming connections.
	pub async fn online(&self) {
		self.endpoint.online().await
	}

	/// Returns a watch receiver that yields updates to the local peer's info.
	pub fn changes(&self) -> watch::Receiver<PeerInfo> {
		self.latest.clone()
	}

	/// Returns the latest known PeerInfo for the local peer.
	pub fn info(&self) -> PeerInfo {
		self.latest.borrow().clone()
	}

	async fn runloop(
		endpoint: Endpoint,
		initial: PeerInfo,
		sender: watch::Sender<PeerInfo>,
		cancel: WaitForCancellationFutureOwned,
	) {
		let mut current = initial;
		let mut cancelled = pin!(cancel);
		let mut addrs_stream = endpoint.watch_addr().stream_updates_only();

		loop {
			tokio::select! {
				_ = &mut cancelled => {
					info!("Local peer runloop terminated");
					break;
				}

				Some(addr) = addrs_stream.next() => {
					info!("Discovered new own address: {:?}", addr);
					if current.address != addr {
						current.address = addr;
						if let Err(e) = sender.send(current.clone()) {
							error!("Failed to send updated PeerInfo: {:?}", e);
							break;
						}
					}
				}
			}
		}
	}
}
