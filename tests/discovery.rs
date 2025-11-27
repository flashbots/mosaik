use {core::time::Duration, mosaik::*, tracing::info};

#[tokio::test]
async fn peers_have_consistent_maps() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;

	info!(
		"Node0 is known as {:?} on network {}",
		n0.local().id(),
		n0.network_id()
	);

	info!("Bootstrap addresses: {:?}", n0.local().addr());

	let mut nodes = vec![];

	for _ in 0..10 {
		let node = Network::new(network_id).await?;
		nodes.push(node);
	}

	loop {
		tokio::time::sleep(Duration::from_secs(3)).await;

		for node in &nodes {
			info!("Peer {} knows about - peers", node.local().id(),);
		}
	}
}
