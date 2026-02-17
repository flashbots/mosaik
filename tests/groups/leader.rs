use {
	crate::{
		groups::{Counter, CounterCommand, leaders_converged},
		utils::{discover_all, timeout_after, timeout_s},
	},
	mosaik::{groups::IndexRange, primitives::Short, *},
};

/// This test verifies that a leader is elected when there are no other known
/// members in the group, and that as new members join the group they recognize
/// the existing leader without waiting for new elections.
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

	// Wait for longer than the bootstrap delay and election timeout to allow
	// leader election.
	let timeout = 2
		* (g0.config().consensus().bootstrap_delay
			+ g0.config().consensus().election_timeout
			+ g0.config().consensus().election_timeout_jitter);
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
	assert_eq!(g1.leader(), Some(n0.local().id()));
	tracing::info!("n1 recognizes n0 ({}) as leader", Short(n0.local().id()));

	// start a third node and have it join the group. It should also recognize the
	// existing leader immediately without waiting for a new election.
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;

	let g2 = n2.groups().with_key(key1).join();

	// After joining, the new node should recognize the existing leader.
	let new_leader = timeout_after(timeout, g2.when().leader_elected()).await?;
	assert_eq!(new_leader, n0.local().id());

	assert!(!g2.is_leader());
	assert_eq!(g2.leader(), Some(n0.local().id()));
	tracing::info!("n2 recognizes n0 ({}) as leader", Short(n0.local().id()));

	// Set up futures to detect leader changes on g1 and g2
	let g1_leader_changed = g1.when().leader_changed();
	let g2_leader_changed = g2.when().leader_changed();

	// now kill the current leader and ensure a new leader is elected among the
	// remaining nodes
	tracing::info!("Shutting down leader n0 ({})", Short(n0.local().id()));
	drop(n0);

	// Both g1 and g2 should detect that n0 is down and elect a new leader.
	let g1_leader = timeout_after(timeout, g1_leader_changed).await?;
	let g2_leader = timeout_after(timeout, g2_leader_changed).await?;

	assert_eq!(g1_leader, g2_leader);
	assert!(g1.is_leader() ^ g2.is_leader());
	tracing::info!("New leader elected after n0 shutdown: {}", Short(g1_leader));

	// start a third node and have it join the group. It should recognize the
	// newly elected leader immediately without waiting for a new election.
	let n3 = Network::new(network_id).await?;
	discover_all([&n1, &n2, &n3]).await?;

	let g3 = n3.groups().with_key(key1).join();

	let g3_leader = timeout_after(timeout, g3.when().leader_elected()).await?;
	assert_eq!(g3_leader, g1_leader);
	assert!(!g3.is_leader());
	assert_eq!(g3.leader(), Some(g1_leader));
	tracing::info!("n3 recognizes new leader {}", Short(g1_leader));

	Ok(())
}

/// This test verifies that if two nodes become leaders on their own due to
/// network partition or delayed discovery, that they will detect each other as
/// rivals and resolve the conflict by stepping down and starting new elections
/// until one of them wins and becomes the leader.
///
/// In this variant of the test, both nodes will have the same log position and
/// term so the leader they converge on will be unpredictable, the one that
/// starts elections first will likely win.
#[tokio::test]
async fn rivals_with_same_log_pos() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let group_key = GroupKey::random();

	let n0 = Network::new(network_id).await?;
	let g0 = n0.groups().with_key(group_key).join();

	let timeout = 2
		* (g0.config().consensus().bootstrap_delay
			+ g0.config().consensus().election_timeout
			+ g0.config().consensus().election_timeout_jitter);

	// no other members in the group, n0 should eventually self-elect itself as
	// the leader of the group.
	timeout_after(timeout, g0.when().is_leader()).await?;
	assert_eq!(g0.leader(), Some(n0.local().id()));
	assert_eq!(g0.committed(), 0);
	tracing::info!("n0 ({}) self-elected as leader", Short(n0.local().id()));

	// let n1 also join the group and don't do any discovery, they should both
	// think they are the only members of the group and self-elect as leaders,
	// creating a rivalry.
	let n1 = Network::new(network_id).await?;
	let g1 = n1.groups().with_key(group_key).join();

	timeout_after(timeout, g1.when().is_leader()).await?;
	assert_eq!(g1.leader(), Some(n1.local().id()));
	assert_eq!(g1.committed(), 0);
	tracing::info!("n1 ({}) self-elected as leader", Short(n1.local().id()));

	// let n2 also join the group and don't do any discovery, they should both
	// think they are the only members of the group and self-elect as leaders,
	// creating a rivalry.
	let n2 = Network::new(network_id).await?;
	let g2 = n2.groups().with_key(group_key).join();

	timeout_after(timeout, g2.when().is_leader()).await?;
	assert_eq!(g2.leader(), Some(n2.local().id()));
	assert_eq!(g2.committed(), 0);
	tracing::info!("n2 ({}) self-elected as leader", Short(n2.local().id()));

	// make them discover each other and they should detect the rivalry
	discover_all([&n0, &n1, &n2]).await?;

	// wait for all nodes to detect the rivalry and converge on the same new
	// leader.
	let new_leader = timeout_s(10, leaders_converged([&g0, &g1, &g2])).await?;

	// both sides should now recognize the new leader and step down to followers.
	timeout_s(2, g0.when().leader_is(new_leader)).await?;
	timeout_s(2, g1.when().leader_is(new_leader)).await?;
	timeout_s(2, g2.when().leader_is(new_leader)).await?;

	assert_eq!(g0.leader(), Some(new_leader));
	assert_eq!(g1.leader(), Some(new_leader));
	assert_eq!(g2.leader(), Some(new_leader));

	tracing::info!(
		"Rivalry resolved, new leader elected: {}",
		Short(new_leader)
	);

	Ok(())
}

/// This test verifies that if two nodes become leader in the same group and
/// have different log positions, that the one with the higher log position will
/// eventually win and become the leader.
#[tokio::test]
async fn rivals_with_different_log_pos() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let group_key = GroupKey::random();

	let n0 = Network::new(network_id).await?;
	let g0 = n0
		.groups()
		.with_key(group_key)
		.with_state_machine(Counter::default())
		.join();

	let timeout = 2
		* (g0.config().consensus().bootstrap_delay
			+ g0.config().consensus().election_timeout
			+ g0.config().consensus().election_timeout_jitter);

	// no other members in the group, n0 should eventually self-elect itself as
	// the leader of the group.
	timeout_after(timeout, g0.when().is_leader()).await?;
	assert_eq!(g0.leader(), Some(n0.local().id()));
	assert_eq!(g0.committed(), 0);
	tracing::info!("n0 ({}) self-elected as leader", Short(n0.local().id()));

	// send some commands to advance the log position of n0
	let index = g0
		.execute_many([
			CounterCommand::Increment(3),
			CounterCommand::Increment(2),
			CounterCommand::Increment(1),
		])
		.await?;
	assert_eq!(index.end(), 3);
	assert_eq!(index, IndexRange::new(1.into(), 3.into()));

	// wait for the commands to be committed
	timeout_s(2, g0.when().committed().reaches(index)).await?;
	assert_eq!(g0.committed(), 3);

	tracing::info!(
		"n0 ({}) advanced log position to {}",
		Short(n0.local().id()),
		g0.log_position()
	);

	// let n1 also join the group and don't do any discovery, they should both
	// think they are the only members of the group and self-elect as leaders,
	// creating a rivalry.
	let n1 = Network::new(network_id).await?;
	let g1 = n1
		.groups()
		.with_key(group_key)
		.with_state_machine(Counter::default())
		.join();

	timeout_after(timeout, g1.when().is_leader()).await?;
	assert_eq!(g1.leader(), Some(n1.local().id()));
	assert_eq!(g1.committed(), 0);
	tracing::info!("n1 ({}) self-elected as leader", Short(n1.local().id()));

	// n1 has a lower log position than n0, so when they discover each other, n0
	// should win the rivalry and n1 should step down to follower.
	let index = g1.execute(CounterCommand::Increment(1)).await?;
	assert_eq!(index, 1);
	timeout_s(3, g1.when().committed().reaches(1)).await?;
	assert_eq!(g1.committed(), 1);
	tracing::info!(
		"n1 ({}) advanced log position to {}",
		Short(n1.local().id()),
		g1.log_position()
	);

	// let n2 also join the group and don't do any discovery, they should both
	// think they are the only members of the group and self-elect as leaders,
	// creating a rivalry.
	let n2 = Network::new(network_id).await?;
	let g2 = n2
		.groups()
		.with_key(group_key)
		.with_state_machine(Counter::default())
		.join();

	timeout_after(timeout, g2.when().is_leader()).await?;
	assert_eq!(g2.leader(), Some(n2.local().id()));
	assert_eq!(g2.committed(), 0);
	tracing::info!("n2 ({}) self-elected as leader", Short(n2.local().id()));

	// make them discover each other and they should detect the rivalry
	// n0 should win since it has a higher log position.
	discover_all([&n0, &n1, &n2]).await?;

	// both sides should now recognize the new leader and step down to followers.
	timeout_s(10, g0.when().leader_is(n0.local().id())).await?;
	timeout_s(10, g1.when().leader_is(n0.local().id())).await?;
	timeout_s(10, g2.when().leader_is(n0.local().id())).await?;

	assert_eq!(g0.leader(), Some(n0.local().id()));
	assert_eq!(g1.leader(), Some(n0.local().id()));
	assert_eq!(g2.leader(), Some(n0.local().id()));

	tracing::info!(
		"Rivalry detected, n0 ({}) wins with higher log position {}",
		Short(n0.local().id()),
		g0.log_position()
	);

	Ok(())
}
