use {
	crate::utils::{discover_all, timeout_s},
	mosaik::{collections::StoreId, *},
};

#[tokio::test]
async fn smoke_no_catchup() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let s0 = mosaik::collections::Set::<u64>::writer(&n0, store_id);
	let s1 = mosaik::collections::Set::<u64>::reader(&n1, store_id);

	timeout_s(10, s0.when().online()).await?;
	timeout_s(10, s1.when().online()).await?;

	let ver = timeout_s(2, s0.insert(42)).await??;
	tracing::info!("Inserted 42 with version {ver}");

	timeout_s(2, s1.when().reaches(ver)).await?;
	assert!(s1.contains(&42));
	assert!(!s1.contains(&99));
	assert_eq!(s1.len(), 1);

	let ver = timeout_s(2, s0.extend(vec![7, 13, 21])).await??;
	timeout_s(2, s1.when().reaches(ver)).await?;

	assert!(s1.contains(&7));
	assert!(s1.contains(&13));
	assert!(s1.contains(&21));
	assert!(s1.contains(&42));
	assert_eq!(s1.len(), 4);

	// Inserting a duplicate should not change the length.
	let ver = timeout_s(2, s0.insert(42)).await??;
	timeout_s(2, s1.when().reaches(ver)).await?;
	assert_eq!(s1.len(), 4);
	assert!(s1.contains(&42));

	let ver = timeout_s(2, s0.remove(7)).await??;
	timeout_s(2, s1.when().reaches(ver)).await?;

	assert!(!s1.contains(&7));
	assert!(s1.contains(&42));
	assert!(s1.contains(&13));
	assert!(s1.contains(&21));
	assert_eq!(s1.len(), 3);

	Ok(())
}

#[tokio::test]
async fn smoke_catchup() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;

	let s0 = mosaik::collections::Set::<u64>::writer(&n0, store_id);
	timeout_s(10, s0.when().online()).await?;

	for i in 0..100 {
		timeout_s(2, s0.insert(i)).await??;
	}

	let ver = timeout_s(2, s0.extend([200, 201, 202, 203, 204])).await??;
	timeout_s(2, s0.when().reaches(ver)).await?;

	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let s1 = mosaik::collections::Set::<u64>::reader(&n1, store_id);
	timeout_s(10, s1.when().online()).await?;
	timeout_s(10, s1.when().reaches(ver)).await?;

	assert_eq!(s1.len(), 105);

	for i in 0..100 {
		assert!(s1.contains(&i));
	}

	for v in [200, 201, 202, 203, 204] {
		assert!(s1.contains(&v));
	}

	let ver = timeout_s(2, s0.remove(50)).await??;
	timeout_s(2, s1.when().reaches(ver)).await?;

	assert_eq!(s1.len(), 104);
	assert!(!s1.contains(&50));

	Ok(())
}
