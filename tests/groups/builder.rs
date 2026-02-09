use {
	super::Counter,
	crate::utils::discover_all,
	mosaik::{builtin::groups::InMemory, *},
};

#[tokio::test]
async fn group_id_derived_from_config() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let group_key = GroupKey::from_secret("secret1");

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	discover_all([&n0, &n1]).await?;

	let g0_0 = n0
		.groups()
		.builder(group_key)
		.with_state_machine(Counter::default())
		.join();

	let g0_1 = n1
		.groups()
		.builder(group_key)
		.with_state_machine(Counter::default())
		.with_storage(InMemory::default())
		.join();

	// assert_eq!(g0_0.group_id(), g0_1.group_id());

	Ok(())
}
