use {
	crate::utils::discover_all,
	mosaik::{primitives::Short, *},
};

#[tokio::test]
async fn members_can_see_each_other() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;
	let n3 = Network::new(network_id).await?;

	discover_all([&n0, &n1, &n2, &n3]).await?;

	let key1 = GroupKey::from_secret("group1".into());
	let key2 = GroupKey::from_secret("group2".into());
	let key3 = GroupKey::from_secret("group3".into());

	tracing::debug!("key1: {key1}, id: {:?}", key1.id());
	tracing::debug!("key2: {key2}, id: {:?}", key2.id());
	tracing::debug!("key3: {key3}, id: {:?}", key3.id());

	// group1: n0 and n1
	let g1_0 = n0.groups().join(key1.clone())?;
	let g1_1 = n1.groups().join(key1.clone())?;

	// group2: n0, n2 and n3
	let g2_0 = n0.groups().join(key2.clone())?;
	let g2_2 = n2.groups().join(key2.clone())?;
	let g2_3 = n3.groups().join(key2.clone())?;

	// group3: n1 and n2
	let g3_1 = n1.groups().join(key3.clone())?;
	let g3_2 = n2.groups().join(key3.clone())?;

	loop {
		tokio::select! {
			() = observe_group_bonds(&g1_0, &n0, "g1_0") => {}
			() = observe_group_bonds(&g1_1, &n1, "g1_1") => {}

			() = observe_group_bonds(&g2_0, &n0, "g2_0") => {}
			() = observe_group_bonds(&g2_2, &n2, "g2_2") => {}
			() = observe_group_bonds(&g2_3, &n3, "g2_3") => {}

			() = observe_group_bonds(&g3_1, &n1, "g3_1") => {}
			() = observe_group_bonds(&g3_2, &n2, "g3_2") => {},
		};
	}
}

async fn observe_group_bonds(group: &Group, net: &Network, name: &str) {
	group.bonds().changed().await;
	tracing::info!("{name} bonds changed (local = {})", Short(net.local().id()));
	for bond in group.bonds().iter() {
		tracing::info!("- {name} bond: {bond}");
	}
}
