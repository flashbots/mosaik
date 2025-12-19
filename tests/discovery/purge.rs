use {crate::utils::*, core::time::Duration, mosaik::*};

#[tokio::test]
async fn stale_peers_are_purged() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let quick_node = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder()
				.with_purge_after(Duration::from_secs(4))
				.with_announce_interval(Duration::from_secs(2))
				.with_announce_jitter(0.0),
		)
		.build()
		.await?;

	let slow_node = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder()
				.with_purge_after(Duration::from_secs(180))
				.with_announce_interval(Duration::from_secs(10))
				.with_announce_jitter(0.0),
		)
		.build()
		.await?;

	// Subscribe to quick node discovery events
	let mut quick_events = quick_node.discovery().events();
	timeout_s(3, discover_all([&quick_node, &slow_node])).await??;

	// the quick node discovers the slow node
	assert_eq!(
		timeout_s(1, quick_events.recv()).await??,
		discovery::Event::PeerDiscovered(
			slow_node
				.discovery()
				.catalog()
				.local()
				.clone()
				.into_unsigned()
		)
	);

	// Initially, both nodes should know about each other.
	assert_eq!(quick_node.discovery().catalog().peers_count(), 1,);
	assert_eq!(slow_node.discovery().catalog().peers_count(), 1,);

	// sleep for a duration longer than the quick node's purge_after time
	// but shorter than the slow node's announce interval
	tokio::time::sleep(Duration::from_secs(4)).await;

	// The quick node should have purged the slow node, while the slow node
	// should still know about the quick node.
	assert_eq!(quick_node.discovery().catalog().peers_count(), 0);
	assert_eq!(slow_node.discovery().catalog().peers_count(), 1);

	// The quick node should emit a PeerDeparted event for the slow node that
	// did not announce itself in time.
	assert_eq!(
		timeout_s(1, quick_events.recv()).await??,
		discovery::Event::PeerDeparted(slow_node.local().id())
	);

	// sleep long enough for the slow node to announce again
	tokio::time::sleep(Duration::from_secs(6)).await;

	// The quick node should have emitted a PeerDiscovered event for the slow
	// node.
	assert_eq!(
		timeout_s(3, quick_events.recv()).await??,
		discovery::Event::PeerDiscovered(
			slow_node
				.discovery()
				.catalog()
				.local()
				.clone()
				.into_unsigned()
		)
	);

	// Now both nodes should know about each other again.
	assert_eq!(quick_node.discovery().catalog().peers_count(), 1,);
	assert_eq!(slow_node.discovery().catalog().peers_count(), 1,);

	Ok(())
}
