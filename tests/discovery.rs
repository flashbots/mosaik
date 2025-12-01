use {core::time::Duration, mosaik::prelude::*};

#[tokio::test]
async fn peers_have_consistent_maps() {
	let network_id = NetworkId::random();
	let n0 = NetworkBuilder::new(network_id.clone())
		.build_and_run()
		.await
		.unwrap();

	let mut nodes = vec![];

	for _ in 0..10 {
		let node = NetworkBuilder::new(network_id.clone())
			.with_bootstrap_peer(n0.local().addr())
			.build_and_run()
			.await
			.unwrap();
		nodes.push(node);
	}

	tokio::time::sleep(Duration::from_secs(2)).await;

	for node in &nodes {
		println!(
			"Peer {} knows about {} peers",
			node.local().id(),
			node.discovery().catalog().len()
		);
	}
}
