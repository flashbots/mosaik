use {
	core::time::Duration,
	futures::{StreamExt, stream::SelectAll},
	mosaik::*,
	tokio::time::interval,
	tokio_stream::wrappers::BroadcastStream,
	tracing::info,
};

#[tokio::test]
async fn catalogs_are_consistent() -> anyhow::Result<()> {
	const PEERS_COUNT: usize = 10;

	let network_id = NetworkId::random();
	let n0 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_tags("beacon-node"),
		)
		.build()
		.await?;

	info!(
		"Node0 (bootstrap) is known as {:?} on network {}",
		n0.local().id(),
		n0.network_id()
	);

	let mut nodes = vec![];

	for _ in 0..PEERS_COUNT {
		let node = Network::builder(network_id)
			.with_discovery(
				discovery::Config::builder() //
					.with_bootstrap(n0.local().id()),
			)
			.build()
			.await?;
		nodes.push(node);
	}

	let mut updates: SelectAll<_> = nodes
		.iter()
		.map(|node| BroadcastStream::new(node.discovery().events()))
		.collect();

	let mut interval = interval(Duration::from_secs(3));

	loop {
		tokio::select! {
			Some(Ok(event)) = updates.next() => {
				info!("Discovery event: {:?}", event);
			}

			_ = interval.tick() => {
				for node in &nodes {
					let catalog = node.discovery().catalog();
					info!(
						"Peer {} knows about {} peers",
						node.local().id(),
						catalog.peers_count()
					);
				}
				info!("---");
			}
		}
	}
}
