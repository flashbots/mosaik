use {
	core::time::Duration,
	futures::future::join_all,
	mosaik::{primitives::Short, *},
	rstest::rstest,
};

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

	// Wait until every node has discovered at least one other peer.
	let mut events: Vec<_> =
		nodes.iter().map(|n| n.discovery().events()).collect();

	tokio::select! {
		results = join_all(events.iter_mut().map(|e| e.recv())) => {
			for (i, result) in results.into_iter().enumerate() {
				let peer_id = result?;
				tracing::info!(
					node = i,
					discovered = %Short(&peer_id),
					network = %network_id,
					"node discovered a peer via DHT bootstrap"
				);
			}
		},
		() = tokio::time::sleep(Duration::from_secs(30 * num_nodes as u64)) => {
			panic!("{num_nodes} nodes failed to discover each other within timeout");
		}
	}

	// Every node should have at least one other node in its catalog.
	for (i, node) in nodes.iter().enumerate() {
		let catalog = node.discovery().catalog();
		let known_peers: Vec<_> = nodes
			.iter()
			.enumerate()
			.filter(|(j, _)| *j != i)
			.filter(|(_, other)| catalog.get(&other.local().id()).is_some())
			.map(|(j, _)| j)
			.collect();

		assert!(
			!known_peers.is_empty(),
			"node {i} did not discover any peers"
		);

		tracing::info!(
			node = i,
			known_peers = ?known_peers,
			"node has discovered peers in catalog"
		);
	}

	Ok(())
}
