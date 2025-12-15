use {
	core::time::Duration,
	futures::{StreamExt, stream::SelectAll},
	mosaik::*,
	std::time::Instant,
	tokio::time::interval,
	tokio_stream::wrappers::BroadcastStream,
	tracing::info,
};

#[tokio::test]
async fn converge_with_bootstrap() -> anyhow::Result<()> {
	const PEERS_COUNT: usize = 10;
	const MAX_INTERVAL: Duration = Duration::from_secs(10);

	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;

	info!(
		"Node0 (bootstrap) is known as {} on network {}",
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
		.map(|node| {
			BroadcastStream::new(node.discovery().events())
				.map(|event| event.map(|res| (res, node.local().id())))
		})
		.collect();

	let start = Instant::now();
	let mut interval = interval(Duration::from_secs(1));

	loop {
		tokio::select! {
			Some(Ok((event, peer_id))) = updates.next() => {
				info!("Discovery event from {peer_id}: {event:?}");
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

				let elapsed = start.elapsed();
				let all_consistent = nodes.iter().all(|node| {
					let catalog = node.discovery().catalog();
					catalog.peers_count() == PEERS_COUNT
				});

				if all_consistent {
					info!("All nodes catalogs synced in {elapsed:?}");
					return Ok(());
				}

				if start.elapsed() >= MAX_INTERVAL {
					assert!(all_consistent, "Catalogs did not converge within {MAX_INTERVAL:?}");
				}
			}
		}
	}
}

#[tokio::test]
async fn manual_sync_trigger() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	// n0 does not know n1 initially and has no bootstrap peers
	let n0 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder()
				.with_tags("tag1")
				.with_tags(["tag2", "tag3"]),
		)
		.build()
		.await?;

	let _p1 = n0.streams().produce::<String>();

	// n1 does not know n0 initially and has no bootstrap peers
	let n1 = Network::new(network_id).await?;

	// nodes don't know each other yet
	assert_eq!(n0.discovery().catalog().peers_count(), 0);
	assert_eq!(n1.discovery().catalog().peers_count(), 0);

	// insert local peer entry of n0 into n1's catalog that doesn't get synced
	// over the network
	n1.discovery().insert(n0.discovery().me());

	assert_eq!(n0.discovery().catalog().peers_count(), 0);
	assert_eq!(n1.discovery().catalog().peers_count(), 1);
	assert_eq!(n1.discovery().catalog().signed_peers().count(), 0);

	// perform manual catalog sync from n1 to n0
	n1.discovery().sync_with(n0.local().id()).await?;

	// both nodes should now know each other
	assert_eq!(n0.discovery().catalog().peers_count(), 1);
	assert_eq!(n1.discovery().catalog().peers_count(), 1);

	// n1 should have n0's signed peer entry now and no unsigned entries
	assert_eq!(n1.discovery().catalog().signed_peers().count(), 1);
	assert_eq!(n1.discovery().catalog().unsigned_peers().count(), 0);

	Ok(())
}

#[tokio::test]
async fn different_networks_are_isolated() -> anyhow::Result<()> {
	let netid1 = NetworkId::random();
	let netid2 = NetworkId::random();

	let n0 = Network::new(netid1).await?;
	let n1 = Network::new(netid1).await?;

	let n2 = Network::new(netid2).await?;
	let n3 = Network::new(netid2).await?;
	let n4 = Network::new(netid2).await?;

	// perform full mesh syncs within each network
	sync_all(&[&n0, &n1, &n2, &n3, &n4]).await?;

	// ensure that peers on the same network have each other in their catalogs,
	// but peers on different networks do not appear in each other's catalogs
	assert_eq!(n0.discovery().catalog().peers_count(), 1);
	assert!(n0.discovery().catalog().get(&n1.local().id()).is_some());

	assert_eq!(n1.discovery().catalog().peers_count(), 1);
	assert!(n1.discovery().catalog().get(&n0.local().id()).is_some());

	assert_eq!(n2.discovery().catalog().peers_count(), 2);
	assert!(n2.discovery().catalog().get(&n3.local().id()).is_some());
	assert!(n2.discovery().catalog().get(&n4.local().id()).is_some());

	assert_eq!(n3.discovery().catalog().peers_count(), 2);
	assert!(n3.discovery().catalog().get(&n2.local().id()).is_some());
	assert!(n3.discovery().catalog().get(&n4.local().id()).is_some());

	assert_eq!(n4.discovery().catalog().peers_count(), 2);
	assert!(n4.discovery().catalog().get(&n2.local().id()).is_some());
	assert!(n4.discovery().catalog().get(&n3.local().id()).is_some());

	Ok(())
}

async fn sync_all(networks: &[&Network]) -> anyhow::Result<()> {
	for i in 0..networks.len() {
		for j in 0..networks.len() {
			if i != j {
				networks[i]
					.discovery()
					.sync_with(networks[j].local().id())
					.await?;
			}
		}
	}
	Ok(())
}
