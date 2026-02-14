use {
	crate::{
		groups::{Counter, CounterCommand, CounterValueQuery},
		utils::{discover_all, timeout_after, timeout_s},
	},
	mosaik::*,
};

/// This test verifies that commands can be executed on leaders and followers,
/// then the state queried with weak consistency when followers need to catch up
/// with log entries.
#[tokio::test]
async fn one_leader_one_follower() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let group_key = GroupKey::random();

	let n0 = Network::new(network_id).await?;
	let g0 = n0
		.groups()
		.with_key(group_key)
		.with_state_machine(Counter::default())
		.join();

	let timeout = 2
		* (g0.config().intervals().bootstrap_delay
			+ g0.config().intervals().election_timeout
			+ g0.config().intervals().election_timeout_jitter);

	// wait for n0 to become online by electing itself as leader and being ready
	// to accept commands
	timeout_after(timeout, g0.when().is_online()).await?;
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
	timeout_s(10, g1.when().log().reaches(g0.log_position().index())).await?;
	assert_eq!(g1.committed(), 4);
	tracing::info!("g1 caught up with the log state of g0");

	Ok(())
}
