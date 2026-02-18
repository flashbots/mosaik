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

	let s0 = mosaik::collections::Vec::<u64>::writer(&n0, store_id);
	let s1 = mosaik::collections::Vec::<u64>::reader(&n1, store_id);

	timeout_s(10, s0.when().online()).await?;
	timeout_s(10, s1.when().online()).await?;

	let ver = timeout_s(2, s0.push(42)).await??;
	tracing::info!("Pushed 42 with version {ver}");

	timeout_s(2, s1.when().reaches(ver)).await?;
	let value = s1.get(0);
	assert_eq!(value, Some(42));

	let ver = timeout_s(2, s0.extend(vec![7, 13, 21])).await??;

	timeout_s(2, s1.when().reaches(ver)).await?;

	assert_eq!(s1.get(1), Some(7));
	assert_eq!(s1.get(2), Some(13));
	assert_eq!(s1.get(3), Some(21));

	let ver = timeout_s(2, s0.remove(1)).await??;
	timeout_s(2, s1.when().reaches(ver)).await?;

	assert_eq!(s1.get(0), Some(42));
	assert_eq!(s1.get(1), Some(13));
	assert_eq!(s1.get(2), Some(21));
	assert_eq!(s1.get(3), None);

	Ok(())
}

#[tokio::test]
async fn smoke_catchup() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;

	let s0 = mosaik::collections::Vec::<u64>::writer(&n0, store_id);
	timeout_s(10, s0.when().online()).await?;

	for i in 0..100 {
		timeout_s(2, s0.push(i + 5)).await??;
	}

	let pos = timeout_s(2, s0.extend([10, 20, 30, 40, 50, 60, 70])).await??;
	timeout_s(2, s0.when().reaches(pos)).await?;

	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let s1 = mosaik::collections::Vec::<u64>::reader(&n1, store_id);
	timeout_s(10, s1.when().online()).await?;
	timeout_s(10, s1.when().reaches(pos)).await?;

	assert_eq!(s1.len(), 107);

	for i in 0..100 {
		assert_eq!(s1.get(i), Some(i + 5));
	}

	for i in 0..7 {
		assert_eq!(s1.get(100 + i), Some((i + 1) * 10));
	}

	let ver = timeout_s(2, s0.remove(50)).await??;
	timeout_s(2, s1.when().reaches(ver)).await?;

	assert_eq!(s1.len(), 106);
	assert_eq!(s1.get(50), Some(56));

	Ok(())
}
