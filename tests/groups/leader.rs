use {
	crate::utils::discover_all,
	mosaik::{primitives::Short, *},
};

#[tokio::test]
async fn leader_is_elected() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let key1 = GroupKey::from_secret("secret1".into());

	let n0 = Network::new(network_id).await?;
	let g0 = n0.groups().join(key1.clone())?;

	// Immediately after joining a group with no other known members,
	// there should be no leader elected before the bootstrap delay.
	assert_eq!(g0.leadership().leader(), None);
	assert_eq!(g0.leadership().is_leader(), false);
	assert_eq!(g0.leadership().term(), 0);
	assert_eq!(g0.leadership().role(), Role::Follower);

	// Wait for longer than the bootstrap delay and election timeout to allow
	// leader election.
	let wait_for = g0.config().bootstrap_delay
		+ g0.config().election_timeout
		+ g0.config().election_timeout_jitter;
	tokio::time::sleep(wait_for * 2).await;

	// There should now be a leader elected, which in this case should be the
	// only member, n0.
	assert_eq!(g0.leadership().leader(), Some(n0.local().id()));
	assert_eq!(g0.leadership().is_leader(), true);

	// Add a second node to the group.
	let n1 = Network::new(network_id).await?;
	let g1 = n1.groups().join(key1.clone())?;

	discover_all([&n0, &n1]).await?;

	// Wait for the leader to be known on g1.
	let g1_leader = g1.leadership().wait_for_leader().await;

	// g1 and g0 should now see n0 as the leader.
	assert_eq!(g1_leader, n0.local().id());
	assert_eq!(g1.leadership().is_leader(), false);
	assert_eq!(g1.leadership().leader(), Some(n0.local().id()));
	assert_eq!(g0.leadership().leader(), Some(n0.local().id()));

	Ok(())
}
