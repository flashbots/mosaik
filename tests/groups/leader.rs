use {
	crate::utils::{discover_all, timeout_after},
	mosaik::{primitives::Short, *},
};

#[tokio::test]
async fn is_elected() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let key1 = GroupKey::from_secret("secret1");

	let n0 = Network::new(network_id).await?;
	let g0 = n0.groups().with_key(key1).join();

	// Immediately after joining a group with no other known members,
	// there should be no leader elected before the bootstrap delay.
	// The node should be a follower with no known leader.
	assert_eq!(g0.leader(), None);
	assert!(!g0.is_leader());
	assert!(g0.is_follower());

	// Wait for longer than the bootstrap delay and election timeout to allow
	// leader election.
	let timeout = 2
		* (g0.config().intervals().bootstrap_delay
			+ g0.config().intervals().election_timeout
			+ g0.config().intervals().election_timeout_jitter);
	let new_leader = timeout_after(timeout, g0.when().leader_elected()).await?;
	tracing::debug!("New leader elected: {}", Short(new_leader));

	// There should now be a leader elected, which in this case should be the
	// only member, n0. As the only member of the group it will self-elect as
	// the leader.
	assert!(g0.is_leader());

	assert_eq!(new_leader, n0.local().id());
	assert_eq!(g0.leader(), Some(n0.local().id()));

	// start a new node and have it join the group. It should recognize the
	// existing leader immediately without waiting for a new election.
	let n1 = Network::new(network_id).await?;
	discover_all([&n0, &n1]).await?;

	let g1 = n1.groups().with_key(key1).join();

	// After joining, the new node should recognize the existing leader.
	let new_leader = timeout_after(timeout, g1.when().leader_elected()).await?;
	assert_eq!(new_leader, n0.local().id());

	assert!(!g1.is_leader());
	assert!(g1.is_follower());
	assert_eq!(g1.leader(), Some(n0.local().id()));
	tracing::debug!("n1 recognizes n0 ({}) as leader", Short(n0.local().id()));

	// start a third node and have it join the group. It should also recognize the
	// existing leader immediately without waiting for a new election.
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;

	let g2 = n2.groups().with_key(key1).join();

	// After joining, the new node should recognize the existing leader.
	let new_leader = timeout_after(timeout, g2.when().leader_elected()).await?;
	assert_eq!(new_leader, n0.local().id());

	assert!(!g2.is_leader());
	assert!(g2.is_follower());
	assert_eq!(g2.leader(), Some(n0.local().id()));
	tracing::debug!("n2 recognizes n0 ({}) as leader", Short(n0.local().id()));

	// Set up futures to detect leader changes on g1 and g2
	let g1_leader_changed = g1.when().leader_changed();
	let g2_leader_changed = g2.when().leader_changed();

	// now kill the current leader and ensure a new leader is elected among the
	// remaining nodes
	tracing::debug!("Shutting down leader n0 ({})", Short(n0.local().id()));
	drop(n0);

	// Both g1 and g2 should detect that n0 is down and elect a new leader.
	let g1_leader = timeout_after(timeout, g1_leader_changed).await?;
	let g2_leader = timeout_after(timeout, g2_leader_changed).await?;

	assert_eq!(g1_leader, g2_leader);
	assert!(g1.is_leader() ^ g2.is_leader());
	assert!(g1.is_follower() ^ g2.is_follower());
	tracing::debug!("New leader elected after n0 shutdown: {}", Short(g1_leader));

	// start a third node and have it join the group. It should recognize the
	// newly elected leader immediately without waiting for a new election.
	let n3 = Network::new(network_id).await?;
	n3.discovery().dial(n1.local().addr()).await;

	let g3 = n3.groups().with_key(key1).join();

	let g3_leader = timeout_after(timeout, g3.when().leader_elected()).await?;
	assert_eq!(g3_leader, g1_leader);
	assert!(!g3.is_leader());
	assert!(g3.is_follower());
	assert_eq!(g3.leader(), Some(g1_leader));
	tracing::debug!("n3 recognizes new leader {}", Short(g1_leader));

	Ok(())
}
