use mosaik::*;

#[tokio::test]
async fn smoke() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;

	let key = GroupKey::with_secret("test-group-key-123".into());
	let g1 = n0.groups().join(key)?;

	let store_id = StoreId::from("test-store-1");
	let _db0 = n0.stores().primary(&g1, store_id);
	let _db1 = n1.stores().replica(store_id);

	core::future::pending::<()>().await;
	Ok(())
}
