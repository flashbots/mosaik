//! Replicated State Machine tests

use {
	crate::{
		groups::{Counter, CounterCommand, CounterValueQuery},
		utils::{discover_all, forever, timeout_after, timeout_s},
	},
	mosaik::*,
};

/// This test verifies that a command can be executed on a leader and a follower
/// when the follower does not need to catch up with any log entries.
#[tokio::test]
async fn no_catchup_weak_follower() -> anyhow::Result<()> {
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
	assert_eq!(g0.committed_index(), 0);
	tracing::info!("g0 is online");

	// start a new node and have it join the group.
	// Since there are no log entries to catch up with, this node should be online
	// immediately after joining the group.
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;

	let g1 = n1
		.groups()
		.with_key(group_key)
		.with_state_machine(Counter::default())
		.join();

	let g2 = n2
		.groups()
		.with_key(group_key)
		.with_state_machine(Counter::default())
		.join();

	// make sure that they are in the same group
	assert_eq!(g0.id(), g1.id());
	assert_eq!(g0.id(), g2.id());

	// wait for g1 to recognize the existing leader and catch up with the log
	timeout_after(timeout, g1.when().is_online()).await?;
	assert_eq!(g1.leader(), Some(n0.local().id()));
	assert_eq!(g1.committed_index(), 0);
	tracing::info!("g1 is online and is following g0 as leader");

	// wait for g2 to recognize the existing leader and catch up with the log
	timeout_after(timeout, g2.when().is_online()).await?;
	assert_eq!(g2.leader(), Some(n0.local().id()));
	assert_eq!(g2.committed_index(), 0);
	tracing::info!("g2 is online and is following g0 as leader");

	// execute two commands on the leader and wait for them to be committed to the
	// state machine.
	timeout_s(2, g0.execute(CounterCommand::Increment(3))).await??;
	timeout_s(2, g0.execute(CounterCommand::Increment(4))).await??;

	let index = g0.committed_index();
	tracing::info!("leader committed to index {index}");
	assert_eq!(index, 2);

	// verify that they have been applied correctly
	let value = g0.query(CounterValueQuery, Consistency::Strong).await?;
	tracing::info!("counter value on leader: {value}");
	assert_eq!(value, 7);

	// wait for the follower g1 to learn from the leader about the new committed
	// index
	let index = timeout_s(2, g1.when().committed_up_to(2)).await?;
	tracing::info!("follower g1 knows that index {index} is committed");
	assert_eq!(index, 2);

	// wait for the follower g2 to learn from the leader about the new committed
	// index
	let index = timeout_s(2, g2.when().committed_up_to(2)).await?;
	tracing::info!("follower g2 knows that index {index} is committed");
	assert_eq!(index, 2);

	// verify that the new node has the correct state after synchronizing the log
	let value = g1.query(CounterValueQuery, Consistency::Weak).await?;
	tracing::info!("counter value on follower g1 (weak): {value}");
	assert_eq!(value, 7);

	// verify that the new node has the correct state after synchronizing the log
	let value = g2.query(CounterValueQuery, Consistency::Weak).await?;
	tracing::info!("counter value on follower g2 (weak): {value}");
	assert_eq!(value, 7);

	forever().await;

	// follower executes a command, should resolve after its replicated and
	// committed to the state machine by the group.
	g1.execute(CounterCommand::Decrement(2)).await?;

	// verify that the command has been applied correctly on both nodes after
	// being replicated to the leader and then applied to the state machine on
	// both nodes.
	let value_n0 = g0.query(CounterValueQuery, Consistency::Strong).await?;
	let value_n1 = g1.query(CounterValueQuery, Consistency::Strong).await?;
	let value_n2 = g2.query(CounterValueQuery, Consistency::Strong).await?;

	assert_eq!(value_n0, 5);
	assert_eq!(value_n1, 5);
	assert_eq!(value_n2, 5);

	Ok(())
}

#[tokio::test]
async fn feed() -> anyhow::Result<()> {
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
	tracing::info!("g0 is online");

	// execute two commands
	g0.feed(CounterCommand::Increment(3)).await?;
	g0.feed(CounterCommand::Increment(4)).await?;

	Ok(())
}
