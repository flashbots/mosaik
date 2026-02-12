use {
	crate::{
		groups::{Counter, CounterCommand},
		utils::timeout_after,
	},
	mosaik::*,
};

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
