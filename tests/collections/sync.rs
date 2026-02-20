use {
	crate::utils::{discover_all, timeout_ms, timeout_s},
	mosaik::{collections::SyncConfig, *},
	rstest::rstest,
};

#[rstest]
#[case(1, 50, 100, 5, false)]
#[case(1000, 50, 100, 5, false)]
#[case(10_000, 50, 100, 5, false)]
#[case(100_000, 50, 100, 5, false)]
#[case(100_000, 50, 100, 5, true)]
#[tokio::test]
async fn vec(
	#[case] data_size: u64,
	#[case] batch_size: u64,
	#[case] interval: u64,
	#[case] retention_window: u32,
	#[case] oneshot_data: bool,
) -> anyhow::Result<()> {
	tracing::info!(
		"test params: data_size={data_size}, batch_size={batch_size}, \
		 interval={interval}, retention_window={retention_window}, \
		 oneshot_data={oneshot_data}"
	);

	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let sync_config = SyncConfig::default()
		.with_interval(interval)
		.with_batch_size(batch_size)
		.with_retention_window(retention_window);

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::Vec::<u64>::new_with_sync_config(
		&n0,
		store_id,
		sync_config.clone(),
	);

	let r1 = collections::Vec::<u64>::reader_with_sync_config(
		&n1,
		store_id,
		sync_config.clone(),
	);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	let mut ver = w0.version();
	let start = std::time::Instant::now();

	if oneshot_data {
		// large dataset created with one command / log entry
		ver = w0.extend((0..data_size).map(|i| i * 10)).await?;
	} else {
		// same dataset created in with a separate command/log entry for each item
		for i in 0..data_size {
			ver = w0.push_back(i * 10).await?;
		}
	}

	let elapsed = start.elapsed();
	tracing::info!("data appended in {elapsed:?}, final version: {ver}");

	timeout_ms(2000 + 10 * data_size, w0.when().reaches(ver)).await?;
	tracing::info!("w0 state committed version {ver} in {:?}", start.elapsed());

	timeout_ms(2000 + 10 * data_size, r1.when().reaches(ver)).await?;
	tracing::info!("r1 state committed version {ver} in {:?}", start.elapsed());

	assert_eq!(r1.len(), data_size as usize);
	assert_eq!(r1.get(0), Some(0));

	for i in 0..data_size {
		assert_eq!(w0.get(i), Some(i * 10));
		assert_eq!(r1.get(i), Some(i * 10));
	}

	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;

	tracing::info!("r2 joining the network");
	let r2 = collections::Vec::<u64>::reader_with_sync_config(
		&n2,
		store_id,
		sync_config.clone(),
	);
	timeout_s(5, r2.when().online()).await?;
	timeout_ms(2000 + 10 * data_size, r2.when().reaches(ver)).await?;
	tracing::info!("r2 state reached version {ver}");

	assert_eq!(r2.len(), data_size as usize);
	assert_eq!(r2.get(0), Some(0));

	for i in 0..data_size {
		assert_eq!(r2.get(i), Some(i * 10));
	}

	Ok(())
}
