use {
	crate::{
		groups::{Counter, CounterCommand, CounterValueQuery},
		utils::{discover_all, timeout_after, timeout_s},
	},
	mosaik::{groups::IndexRange, *},
};

#[tokio::test]
async fn smoke() -> anyhow::Result<()> {
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

	// bring g1 online as a follower
	let n1 = Network::new(network_id).await?;
	let g1 = n1
		.groups()
		.with_key(group_key)
		.with_state_machine(Counter::default())
		.join();

	discover_all([&n0, &n1]).await?;

	timeout_after(timeout, g1.when().online()).await?;
	assert_eq!(g0.leader(), Some(n0.local().id()));
	tracing::info!("g0 is online");

	// feed two commands through follower
	let index1 = timeout_s(2, g1.feed(CounterCommand::Increment(3))).await??;
	let index2 = timeout_s(2, g1.feed(CounterCommand::Increment(4))).await??;
	let range = timeout_s(
		2,
		g1.feed_many([
			CounterCommand::Increment(3),
			CounterCommand::Increment(3),
			CounterCommand::Increment(2),
		]),
	)
	.await??;

	assert_eq!(index1, 1);
	assert_eq!(index2, 2);
	assert_eq!(range, IndexRange::new(3.into(), 5.into()));

	// wait for the commands to be committed to the log and applied to the state
	// machine on g0
	timeout_after(timeout, g0.when().committed().reaches(5)).await?;
	let value = timeout_s(2, g0.query(CounterValueQuery, Weak)).await??;
	assert_eq!(value, 15);
	tracing::info!("g0 has committed commands fed through g1: {value}");

	// wait for the commands to be committed to the log and applied to the state
	// machine on g1
	timeout_after(timeout, g1.when().committed().reaches(5)).await?;
	let value = timeout_s(2, g1.query(CounterValueQuery, Weak)).await??;
	assert_eq!(value, 15);
	tracing::info!("g1 has committed commands fed through g1: {value}");

	Ok(())
}
