use {
	core::time::Duration,
	futures::{StreamExt, stream::SelectAll},
	mosaik::*,
	tokio::time::interval,
	tokio_stream::wrappers::BroadcastStream,
	tracing::info,
};

#[tokio::test]
async fn peers_have_consistent_maps() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::builder(network_id)
		.with_discovery(
			DiscoveryConfig::builder()
				.with_tags("beacon-node")
				.build()?,
		)
		.build()
		.await?;

	info!(
		"Node0 is known as {:?} on network {}",
		n0.local().id(),
		n0.network_id()
	);

	n0.online().await;

	info!("Bootstrap addresses: {:?}", n0.local().addr());

	let mut nodes = vec![];

	for _ in 0..10 {
		let node = Network::builder(network_id)
			.with_discovery(
				DiscoveryConfig::builder()
					.with_bootstrap(n0.local().id())
					.build()?,
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
