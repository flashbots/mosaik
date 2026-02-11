//! Replicated State Machine tests

use {
	crate::{
		groups::{Counter, CounterCommand, CounterValueQuery},
		utils::{discover_all, timeout_after},
	},
	mosaik::*,
};

#[tokio::test]
async fn commands_are_synced() -> anyhow::Result<()> {
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

	// issue two commands
	g0.command(CounterCommand::Increment(3)).await?;
	g0.command(CounterCommand::Increment(4)).await?;

	core::future::pending::<()>().await;

	// verify that they have been applied correctly
	let value = g0.query(CounterValueQuery, Consistency::Strong).await?;
	assert_eq!(value, 7);

	// start a new node and have it join the group. It should synchronize the
	// existing log entries and apply them to its state machine.
	let n1 = Network::new(network_id).await?;
	discover_all([&n0, &n1]).await?;

	let g1 = n1
		.groups()
		.with_key(group_key)
		.with_state_machine(Counter::default())
		.join();

	// make sure that they are in the same group
	assert_eq!(g0.id(), g1.id());

	// wait for g1 to recognize the existing leader and catch up with the log
	timeout_after(timeout, g1.when().is_online()).await?;
	assert_eq!(g1.leader(), Some(n0.local().id()));
	tracing::info!("g1 is online");

	// verify that the new node has the correct state after synchronizing the log
	let value = g1.query(CounterValueQuery, Consistency::Strong).await?;
	assert_eq!(value, 7);

	// let n1 issue a command as a follower.
	g1.command(CounterCommand::Decrement(2)).await?;

	// verify that the command has been applied correctly on both nodes after
	// being replicated to the leader and then applied to the state machine on
	// both nodes.
	let value_n0 = g0.query(CounterValueQuery, Consistency::Strong).await?;
	let value_n1 = g1.query(CounterValueQuery, Consistency::Strong).await?;

	assert_eq!(value_n0, 5);
	assert_eq!(value_n1, 5);

	Ok(())
}
