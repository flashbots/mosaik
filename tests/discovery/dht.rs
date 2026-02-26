use {core::time::Duration, mosaik::*, rstest::rstest};

/// This test verifies that N nodes can discover each other via the DHT
/// bootstrap mechanism without explicitly being provided with each other's peer
/// ID or address.
#[rstest]
#[case(2)]
#[case(3)]
#[case(5)]
#[case(10)]
#[case(15)]
#[case(35)]
#[tokio::test]
async fn auto_bootstrap(#[case] num_nodes: usize) -> anyhow::Result<()> {
	tracing::info!("starting auto bootstrap test with {num_nodes} nodes");
	let network_id = NetworkId::random();

	let mut nodes = Vec::with_capacity(num_nodes);
	for _ in 0..num_nodes {
		nodes.push(Network::new(network_id).await?);
	}

	// every node should have discovered all other nodes within the network
	let mut success = false;
	for _ in 0..(num_nodes * 10) {
		let mut all_discovered = true;
		for (i, node) in nodes.iter().enumerate() {
			let catalog = node.discovery().catalog();

			tracing::info!(
				"node {i} has discovered {} peers",
				catalog.signed_peers_count()
			);

			if catalog.signed_peers_count() != num_nodes - 1 {
				all_discovered = false;
			}
		}

		if all_discovered {
			success = true;
			break;
		}

		tracing::info!("---");
		tokio::time::sleep(Duration::from_secs(1)).await;
	}

	assert!(
		success,
		"not all nodes discovered each other within timeout"
	);

	Ok(())
}
