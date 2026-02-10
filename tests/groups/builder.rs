use {
	super::Counter,
	crate::utils::discover_all,
	mosaik::{
		builtin::groups::{InMemory, NoOp},
		*,
	},
};

#[tokio::test]
async fn group_id_derived_from_config() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let group_key1 = GroupKey::random();
	let group_key2 = GroupKey::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;

	discover_all([&n0, &n1]).await?;

	let g0_0 = n0
		.groups()
		.with_key(group_key1)
		.with_state_machine(Counter::default())
		.join();

	let g0_1 = n1
		.groups()
		.with_key(group_key1)
		.with_state_machine(Counter::default())
		.with_storage(InMemory::default())
		.join();

	tracing::debug!("g0_0: {g0_0}");
	tracing::debug!("g0_1: {g0_1}");

	// all consensus-relevant configuration parameters are the same for both
	// groups, so they should have the same group id.
	assert_eq!(g0_0.id(), g0_1.id());

	// changing the group key should result in a different group id.
	let g1_0 = n0
		.groups()
		.with_key(group_key2)
		.with_state_machine(Counter::default())
		.join();

	tracing::debug!("g1_0: {g1_0}");
	assert_ne!(g0_0.id(), g1_0.id());

	let g1_1 = n1
		.groups()
		.with_key(group_key2)
		.with_state_machine(Counter::default())
		.join();

	// all consensus-relevant configuration parameters are the same for both	//
	// groups, so they should have the same group id.
	tracing::debug!("g1_1: {g1_1}");
	assert_eq!(g1_0.id(), g1_1.id());

	// g2_0 has all the same configuration parameters as g0_0 except for the state
	// machine and that should render a different group id since the state
	// machine type is part of the group id derivation.
	let g2_0 = n0
		.groups()
		.with_key(group_key1)
		.with_state_machine(NoOp)
		.join();

	tracing::debug!("g2_0: {g2_0}");
	assert_ne!(g0_0.id(), g2_0.id());

	Ok(())
}
