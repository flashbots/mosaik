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

#[tokio::test]
async fn graceful_departure() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;

	let mut n0_events = n0.discovery().events();
	let mut n1_events = n1.discovery().events();
	let mut n2_events = n2.discovery().events();

	timeout_s(3, discover_all([&n0, &n1, &n2])).await??;

	// ensure all nodes know each other
	assert_eq!(n0.discovery().catalog().peers_count(), 2);
	assert_eq!(n1.discovery().catalog().peers_count(), 2);
	assert_eq!(n2.discovery().catalog().peers_count(), 2);

	// ensure that they all received PeerDiscovered events for each other
	for (node, events) in [
		(&n0, &mut n0_events),
		(&n1, &mut n1_events),
		(&n2, &mut n2_events),
	] {
		let mut discovered = Vec::new();

		for _ in 0..2 {
			if let discovery::Event::PeerDiscovered(entry) =
				timeout_s(1, events.recv()).await??
			{
				discovered.push(*entry.id());
			}
		}
		let mut expected: Vec<PeerId> = node
			.discovery()
			.catalog()
			.peers()
			.map(|e| *e.id())
			.collect();

		discovered.sort();
		expected.sort();

		assert_eq!(discovered.len(), 2);
		assert_eq!(discovered, expected);
	}

	// kill n1 gracefully
	let n1_id = n1.local().id();
	drop(n1);

	// n0 and n2 should receive PeerDeparted events for n1
	for events in [&mut n0_events, &mut n2_events] {
		match timeout_s(2, events.recv()).await?? {
			discovery::Event::PeerDeparted(peer_id) => {
				assert_eq!(peer_id, n1_id);
			}
			other => panic!("expected PeerDeparted event, got {other:?}"),
		}
	}

	Ok(())
}
