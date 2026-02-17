use {
	crate::{
		groups::{Counter, CounterCommand, CounterValueQuery},
		utils::{discover_all, timeout_after, timeout_s},
	},
	mosaik::{groups::IndexRange, *},
	rstest::rstest,
};

/// This test verifies that commands can be executed on leaders and followers,
/// then the state queried with weak consistency when followers need to catch up
/// with log entries, small test, all commands fit in one sync chunk.
#[tokio::test]
async fn one_leader_one_follower_small() -> anyhow::Result<()> {
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

	// wait for n0 to become online by electing itself as leader and being ready
	// to accept commands
	timeout_after(timeout, g0.when().online()).await?;
	assert_eq!(g0.leader(), Some(n0.local().id()));
	assert_eq!(g0.committed(), 0);
	tracing::info!("g0 is online");

	// execute four commands on the leader and wait for them to be committed to
	// the state machine.
	timeout_s(2, g0.execute(CounterCommand::Increment(3))).await??;
	timeout_s(
		2,
		g0.execute_many([
			CounterCommand::Increment(4),
			CounterCommand::Decrement(2),
			CounterCommand::Increment(1),
		]),
	)
	.await??;

	let index = g0.committed();
	tracing::info!("leader committed to index {index}");
	assert_eq!(index, 4);

	let g0_value = g0.query(CounterValueQuery, Consistency::Weak).await?;
	tracing::info!("counter value on leader (weak): {g0_value}");
	assert_eq!(g0_value, 6);

	// start a new node and have it join the group.
	// Since there are log entries to catch up with, this node should not be
	// online immediately after joining the group, and needs to synchronize with
	// the group.
	let n1 = Network::new(network_id).await?;
	let g1 = n1
		.groups()
		.with_key(group_key)
		.with_state_machine(Counter::default())
		.join();

	discover_all([&n0, &n1]).await?;

	// wait for g1 to recognize the existing leader
	timeout_after(timeout, g1.when().leader_is(n0.local().id())).await?;
	assert_eq!(g1.leader(), Some(n0.local().id()));
	tracing::info!("g1 recognizes g0 as leader");

	// wait for g1 to catch up with the log state of g0
	let index =
		timeout_s(10, g1.when().log().reaches(g0.log_position().index())).await?;
	tracing::info!("g1 log position caught up with the log state of g0: {index}");

	let index =
		timeout_s(2, g1.when().committed().reaches(g0.committed())).await?;
	tracing::info!("g1 committed index caught up with g0: {index}");

	let g1_value = g1.query(CounterValueQuery, Consistency::Weak).await?;
	tracing::info!("counter value on follower (weak): {g1_value}");
	assert_eq!(g1_value, 6);

	// now replication should work as normal.
	let index = timeout_s(
		2,
		g0.execute_many([
			CounterCommand::Increment(5),
			CounterCommand::Decrement(3),
		]),
	)
	.await??;

	let index = timeout_s(2, g1.when().committed().reaches(index)).await?;
	assert_eq!(index, 6);

	let g1_value = g1.query(CounterValueQuery, Consistency::Weak).await?;
	assert_eq!(g1_value, 8);

	tracing::info!(
		"counter value on follower after replication (weak): {g1_value}"
	);

	Ok(())
}

#[rstest]
#[case(7)]
#[case(20)]
#[case(100)]
#[case(1000)]
#[tokio::test]
async fn one_leader_one_follower_large(
	#[case] batch_size: u64,
) -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let group_key = GroupKey::random();

	let n0 = Network::new(network_id).await?;
	let g0 = n0
		.groups()
		.with_key(group_key)
		.with_state_machine(Counter::default().with_sync_batch_size(batch_size))
		.join();

	let timeout = 2
		* (g0.config().consensus().bootstrap_delay
			+ g0.config().consensus().election_timeout
			+ g0.config().consensus().election_timeout_jitter);

	// wait for n0 to become online by electing itself as leader and being ready
	// to accept commands
	timeout_after(timeout, g0.when().online()).await?;
	assert_eq!(g0.leader(), Some(n0.local().id()));
	assert_eq!(g0.committed(), 0);
	tracing::info!("g0 is online");

	// feed a large number of commands to the leader that spans multiple catchup
	// chunks.
	let commands = (0..200).map(CounterCommand::Increment).collect::<Vec<_>>();
	let ixs = timeout_s(5, g0.execute_many(commands)).await??;

	assert_eq!(ixs.end(), 200);
	assert_eq!(ixs, IndexRange::new(1.into(), 200.into()));
	assert_eq!(g0.committed(), 200);

	// start a new node and have it join the group.
	// Since there are log entries to catch up with, this node should not be
	// online immediately after joining the group, and needs to synchronize with
	// the group.
	let n1 = Network::new(network_id).await?;
	let g1 = n1
		.groups()
		.with_key(group_key)
		.with_state_machine(Counter::default().with_sync_batch_size(batch_size))
		.join();

	discover_all([&n0, &n1]).await?;

	// wait for g1 to recognize the existing leader
	timeout_after(timeout, g1.when().leader_is(n0.local().id())).await?;
	assert_eq!(g1.leader(), Some(n0.local().id()));
	tracing::info!("g1 recognizes g0 as leader");

	// wait for g1 to catch up with the log state of g0
	let index =
		timeout_s(20, g1.when().committed().reaches(g0.log_position().index()))
			.await?;
	assert_eq!(index, 200);
	assert_eq!(g1.committed(), 200);
	tracing::info!("g1 log position caught up with the log state of g0: {index}");
	let value = g1.query(CounterValueQuery, Consistency::Weak).await?;
	assert_eq!(value, 19900);
	tracing::info!(
		"counter value on g1 after catching up with large log: {value}"
	);

	// g1 issues many commands that span multiple catchup chunks,
	let ixs =
		timeout_s(5, g1.execute_many((0..200).map(CounterCommand::Increment)))
			.await??;
	assert_eq!(ixs.end(), 400);
	assert_eq!(ixs, IndexRange::new(201.into(), 400.into()));
	assert_eq!(g1.committed(), 400);

	timeout_s(20, g0.when().committed().reaches(400)).await?;
	assert_eq!(g0.committed(), 400);

	// now start a new node and have it join the group, it should be able to catch
	// up with the large log state that spans multiple catchup chunks.
	let n2 = Network::new(network_id).await?;
	let g2 = n2
		.groups()
		.with_key(group_key)
		.with_state_machine(Counter::default().with_sync_batch_size(batch_size))
		.join();

	discover_all([&n0, &n1, &n2]).await?;

	// wait for g2 to recognize the existing leader
	timeout_after(timeout, g2.when().leader_is(n0.local().id())).await?;
	assert_eq!(g2.leader(), Some(n0.local().id()));
	tracing::info!("g2 recognizes g0 as leader");

	// wait for g2 to catch up with the log state of g0
	let index =
		timeout_s(20, g2.when().committed().reaches(g0.log_position().index()))
			.await?;
	assert_eq!(index, 400);
	assert_eq!(g2.committed(), 400);
	tracing::info!("g2 log position caught up with the log state of g0: {index}");

	// query the state on g2 to verify that it has the correct state after
	// catching up with the large log.
	let value = g2.query(CounterValueQuery, Consistency::Weak).await?;
	assert_eq!(value, 39800);
	tracing::info!(
		"counter value on g2 after catching up with large log: {value}"
	);

	let value = g0.query(CounterValueQuery, Consistency::Weak).await?;
	assert_eq!(value, 39800);
	tracing::info!(
		"counter value on g0 after g2 catches up with large log: {value}"
	);

	let value = g1.query(CounterValueQuery, Consistency::Weak).await?;
	assert_eq!(value, 39800);
	tracing::info!(
		"counter value on g1 after g2 catches up with large log: {value}"
	);

	Ok(())
}
