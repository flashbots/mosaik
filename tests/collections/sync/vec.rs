use {
	crate::utils::{discover_all, timeout_ms, timeout_s},
	mosaik::{collections::SyncConfig, primitives::Short, *},
	rstest::rstest,
};

#[rstest]
#[case(1, 100, false)]
#[case(100, 30, false)]
#[case(1000, 100, false)]
#[case(10_000, 100, false)]
#[case(10_000, 1000, false)]
#[case(100_000, 100, false)]
#[case(100_000, 100, true)]
#[tokio::test]
async fn frozen(
	#[case] data_size: u64,
	#[case] fetch_batch_size: u64,
	#[case] oneshot_data: bool,
) -> anyhow::Result<()> {
	tracing::info!(
		"test params: data_size={data_size}, fetch_batch_size={fetch_batch_size}, \
		 oneshot_data={oneshot_data}"
	);

	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let sync_config =
		SyncConfig::default().with_fetch_batch_size(fetch_batch_size);

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
	tracing::info!("w0 is online as {}", Short(n0.local().id()));

	timeout_s(15, r1.when().online()).await?;
	tracing::info!("r1 is online as {}", Short(n1.local().id()));

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
	tracing::info!("vec populated in {elapsed:?}, final version: {ver}");

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
	tracing::info!("r2 joining the network as {}", Short(n2.local().id()));

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

	// verify that all data on all nodes is the same after the sync
	assert_eq!(w0.len(), data_size as usize);
	assert_eq!(r1.len(), data_size as usize);
	assert_eq!(r2.len(), data_size as usize);

	for i in 0..data_size {
		assert_eq!(w0.get(i), Some(i * 10));
		assert_eq!(r1.get(i), Some(i * 10));
		assert_eq!(r2.get(i), Some(i * 10));
	}
	tracing::info!("data consistency verified across all nodes");

	Ok(())
}

/// Similar to `vec`, but new data keeps arriving from the writer while `r2` is
/// syncing a snapshot. This ensures that the snapshot sync mechanism handles
/// concurrent writes correctly and all nodes converge to a consistent state.
#[rstest]
#[case(100, 50, 100, true)]
#[case(1000, 500, 100, true)]
#[case(100_000, 5000, 100, false)]
#[case(100_000, 5000, 100, true)]
#[case(100_000, 5000, 1000, false)]
#[case(100_000, 5000, 1000, true)]
#[tokio::test]
async fn writes_during_sync(
	#[case] initial_size: u64,
	#[case] extra_size: u64,
	#[case] fetch_batch_size: u64,
	#[case] oneshot_data: bool,
) -> anyhow::Result<()> {
	tracing::info!(
		"test params: initial_size={initial_size}, extra_size={extra_size}, \
		 fetch_batch_size={fetch_batch_size}, oneshot_data={oneshot_data}"
	);

	let network_id = NetworkId::random();
	let store_id = StoreId::random();
	let total_size = initial_size + extra_size;

	let sync_config =
		SyncConfig::default().with_fetch_batch_size(fetch_batch_size);

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
	tracing::info!("w0 is online as {}", Short(n0.local().id()));

	timeout_s(15, r1.when().online()).await?;
	tracing::info!("r1 is online as {}", Short(n1.local().id()));

	// populate initial data
	let mut ver = w0.version();
	let start = std::time::Instant::now();
	if oneshot_data {
		// large dataset created with one command / log entry
		ver = w0.extend((0..initial_size).map(|i| i * 10)).await?;
	} else {
		// same dataset created in with a separate command/log entry for each item
		for i in 0..initial_size {
			ver = w0.push_back(i * 10).await?;
		}
	}
	tracing::info!(
		"initial data ({initial_size} items) populated in {:?}, version: {ver}",
		start.elapsed()
	);

	timeout_ms(2000 + 10 * initial_size, w0.when().reaches(ver)).await?;
	timeout_ms(2000 + 10 * initial_size, r1.when().reaches(ver)).await?;
	tracing::info!("w0 and r1 have initial data at version {ver}");

	// bring up r2 — it will need to snapshot-sync to catch up
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;
	tracing::info!("r2 joining the network as {}", Short(n2.local().id()));

	let r2 = collections::Vec::<u64>::reader_with_sync_config(
		&n2,
		store_id,
		sync_config.clone(),
	);

	// push more data while r2 is syncing the snapshot
	for i in initial_size..total_size {
		ver = w0.push_back(i * 10).await?;
	}
	tracing::info!(
		"extra data ({extra_size} items) pushed, final version: {ver}"
	);

	// wait for all nodes to converge to the final version
	timeout_ms(2000 + 10 * total_size, w0.when().reaches(ver)).await?;
	tracing::info!("w0 reached version {ver} in {:?}", start.elapsed());

	timeout_ms(2000 + 10 * total_size, r1.when().reaches(ver)).await?;
	tracing::info!("r1 reached version {ver} in {:?}", start.elapsed());

	timeout_ms(2000 + 10 * total_size, r2.when().reaches(ver)).await?;
	tracing::info!("r2 reached version {ver} in {:?}", start.elapsed());

	// verify data consistency across all nodes
	assert_eq!(w0.len(), total_size as usize);
	assert_eq!(r1.len(), total_size as usize);
	assert_eq!(r2.len(), total_size as usize);

	for i in 0..total_size {
		let expected = i * 10;
		assert_eq!(w0.get(i), Some(expected), "w0 mismatch at index {i}");
		assert_eq!(r1.get(i), Some(expected), "r1 mismatch at index {i}");
		assert_eq!(r2.get(i), Some(expected), "r2 mismatch at index {i}");
	}
	tracing::info!("data consistency verified across all nodes");

	Ok(())
}

#[tokio::test]
async fn empty() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::Vec::<u64>::new(&n0, store_id);
	let r1 = collections::Vec::<u64>::reader(&n1, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	// No data pushed — reader should converge to empty
	assert_eq!(r1.len(), 0);
	assert_eq!(r1.get(0u64), None);
	Ok(())
}

/// Single-element edge case: ensures snapshot/batch logic handles the minimum
/// non-empty case without off-by-one errors.
#[tokio::test]
async fn single_element() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::Vec::<u64>::new(&n0, store_id);
	let r1 = collections::Vec::<u64>::reader(&n1, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	let ver = w0.push_back(42).await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;

	assert_eq!(r1.len(), 1);
	assert_eq!(r1.get(0u64), Some(42));
	assert_eq!(r1.get(1u64), None);

	// late joiner also gets the single element
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;
	let r2 = collections::Vec::<u64>::reader(&n2, store_id);
	timeout_s(5, r2.when().online()).await?;
	timeout_ms(5000, r2.when().reaches(ver)).await?;

	assert_eq!(r2.len(), 1);
	assert_eq!(r2.get(0u64), Some(42));
	Ok(())
}

/// Reader is created on the network before any writer exists. Then the writer
/// appears and populates data. Verifies the reader eventually discovers the
/// writer and syncs.
#[tokio::test]
async fn reader_before_writer() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	// reader first
	let r1 = collections::Vec::<u64>::reader(&n1, store_id);

	// writer second
	let w0 = collections::Vec::<u64>::new(&n0, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	let ver = w0.extend((0..100u64).map(|i| i * 10)).await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;

	assert_eq!(r1.len(), 100);
	for i in 0..100u64 {
		assert_eq!(r1.get(i), Some(i * 10));
	}
	Ok(())
}

/// Extreme batch sizes: `batch_size` >> `data_size` and `batch_size` = 1.
/// Ensures the batching loop handles edge cases without panics or off-by-one
/// errors.
#[rstest]
#[case(10, 10_000)]
#[case(100, 100_000)]
#[case(100, 1)]
#[case(1000, 1)]
#[tokio::test]
async fn extreme_batch_sizes(
	#[case] data_size: u64,
	#[case] fetch_batch_size: u64,
) -> anyhow::Result<()> {
	tracing::info!(
		"test params: data_size={data_size}, fetch_batch_size={fetch_batch_size}"
	);

	let network_id = NetworkId::random();
	let store_id = StoreId::random();
	let sync_config =
		SyncConfig::default().with_fetch_batch_size(fetch_batch_size);

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

	let ver = w0.extend((0..data_size).map(|i| i * 10)).await?;
	timeout_ms(5000 + 10 * data_size, r1.when().reaches(ver)).await?;

	assert_eq!(r1.len(), data_size as usize);
	for i in 0..data_size {
		assert_eq!(r1.get(i), Some(i * 10));
	}

	// verify late joiner with same extreme batch size
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;
	let r2 = collections::Vec::<u64>::reader_with_sync_config(
		&n2,
		store_id,
		sync_config,
	);
	timeout_s(5, r2.when().online()).await?;
	timeout_ms(5000 + 10 * data_size, r2.when().reaches(ver)).await?;

	assert_eq!(r2.len(), data_size as usize);
	for i in 0..data_size {
		assert_eq!(r2.get(i), Some(i * 10));
	}
	Ok(())
}

/// Multiple readers joining simultaneously. Spins up several readers at once
/// to verify the writer isn't overwhelmed and all converge correctly.
#[rstest]
#[case(5)]
#[tokio::test]
async fn many_readers(#[case] num_readers: usize) -> anyhow::Result<()> {
	tracing::info!("test params: num_readers={num_readers}");

	let network_id = NetworkId::random();
	let store_id = StoreId::random();
	let data_size = 1000u64;

	let n0 = Network::new(network_id).await?;

	let w0 = collections::Vec::<u64>::new(&n0, store_id);
	timeout_s(15, w0.when().online()).await?;

	let ver = w0.extend((0..data_size).map(|i| i * 10)).await?;
	timeout_ms(5000, w0.when().reaches(ver)).await?;
	tracing::info!("writer populated {data_size} items, version: {ver}");

	// create all reader networks and discover
	let mut networks = vec![];
	for _ in 0..num_readers {
		let n = Network::new(network_id).await?;
		networks.push(n);
	}

	let mut all_nets: std::vec::Vec<&Network> = vec![&n0];
	all_nets.extend(networks.iter());
	timeout_s(15, discover_all(all_nets)).await??;
	tracing::info!("all {num_readers} reader networks discovered the writer");

	// create all readers at once
	let readers: std::vec::Vec<_> = networks
		.iter()
		.map(|n| collections::Vec::<u64>::reader(n, store_id))
		.collect();

	// wait for all to come online and converge
	for (i, r) in readers.iter().enumerate() {
		timeout_s(15, r.when().online()).await?;
		timeout_ms(10_000, r.when().reaches(ver)).await?;
		tracing::info!("reader {i} reached version {ver}");
	}

	// verify all have correct data
	for (i, r) in readers.iter().enumerate() {
		assert_eq!(r.len(), data_size as usize, "reader {i} length mismatch");
		for j in 0..data_size {
			assert_eq!(r.get(j), Some(j * 10), "reader {i} mismatch at index {j}");
		}
	}
	tracing::info!("all {num_readers} readers have correct data");
	Ok(())
}

/// After full convergence, push more data and verify all existing readers pick
/// up the new items via incremental sync (not a new snapshot).
#[tokio::test]
async fn incremental_after_convergence() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1, &n2])).await??;

	let w0 = collections::Vec::<u64>::new(&n0, store_id);
	let r1 = collections::Vec::<u64>::reader(&n1, store_id);
	let r2 = collections::Vec::<u64>::reader(&n2, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;
	timeout_s(15, r2.when().online()).await?;

	// initial data
	let ver1 = w0.extend(0..500u64).await?;
	timeout_ms(5000, r1.when().reaches(ver1)).await?;
	timeout_ms(5000, r2.when().reaches(ver1)).await?;
	tracing::info!("phase 1 converged at version {ver1}");

	assert_eq!(r1.len(), 500);
	assert_eq!(r2.len(), 500);

	// push more data after convergence
	let mut ver2 = ver1;
	for i in 500..1000u64 {
		ver2 = w0.push_back(i).await?;
	}
	tracing::info!("phase 2: pushed 500 more items, version: {ver2}");

	timeout_ms(10_000, r1.when().reaches(ver2)).await?;
	timeout_ms(10_000, r2.when().reaches(ver2)).await?;

	assert_eq!(w0.len(), 1000);
	assert_eq!(r1.len(), 1000);
	assert_eq!(r2.len(), 1000);

	for i in 0..1000u64 {
		assert_eq!(w0.get(i), Some(i), "w0 mismatch at {i}");
		assert_eq!(r1.get(i), Some(i), "r1 mismatch at {i}");
		assert_eq!(r2.get(i), Some(i), "r2 mismatch at {i}");
	}
	tracing::info!("incremental sync verified");
	Ok(())
}

/// Concurrent reads while sync is in progress. Spawns a task that continuously
/// reads from r2 during snapshot sync and verifies no panics or inconsistent
/// intermediate states (e.g. `len()` returns N but `get(N-1)` returns `None`).
#[tokio::test]
async fn concurrent_reads_during_sync() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();
	let data_size = 10_000u64;

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::Vec::<u64>::new(&n0, store_id);
	let r1 = collections::Vec::<u64>::reader(&n1, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	let ver = w0.extend((0..data_size).map(|i| i * 10)).await?;
	timeout_ms(10_000, r1.when().reaches(ver)).await?;
	tracing::info!("initial data ready, version: {ver}");

	// bring up r2 — it will snapshot-sync
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;

	let r2 = collections::Vec::<u64>::reader(&n2, store_id);

	// wait for r2 to come online before spawning the reader task, so we can
	// use r2 inside the spawned task without borrow conflicts.
	timeout_s(15, r2.when().online()).await?;

	// spawn a task that continuously reads during sync
	let reader_task = tokio::spawn({
		async move {
			let mut read_count = 0u64;
			loop {
				let len = r2.len();
				if len > 0 {
					// if len > 0, the last element must be readable
					let last = r2.get((len - 1) as u64);
					assert!(
						last.is_some(),
						"len={len} but get({}) returned None (read #{read_count})",
						len - 1
					);

					// spot-check first element
					let first = r2.get(0u64);
					assert!(
						first.is_some(),
						"len={len} but get(0) returned None (read #{read_count})"
					);
				}
				read_count += 1;

				if len == data_size as usize {
					break;
				}
				tokio::task::yield_now().await;
			}
			tracing::info!("concurrent reader performed {read_count} reads");
			read_count
		}
	});

	let read_count = timeout_ms(10_000 + 10 * data_size, reader_task).await??;
	tracing::info!("reader task completed with {read_count} reads");

	Ok(())
}

/// Writer extends after snapshot, before reader catches up on log.
/// Writer creates initial items, reader starts syncing, then writer does a
/// single `extend` of more items. Tests the transition from snapshot-sync to
/// log-replay when the log contains a multi-item command.
#[rstest]
#[case(1000, 500, 100)]
#[case(10_000, 5000, 100)]
#[case(10_000, 5000, 1000)]
#[tokio::test]
async fn extend_during_snapshot_sync(
	#[case] initial_size: u64,
	#[case] extend_size: u64,
	#[case] fetch_batch_size: u64,
) -> anyhow::Result<()> {
	tracing::info!(
		"test params: initial_size={initial_size}, extend_size={extend_size}, \
		 fetch_batch_size={fetch_batch_size}"
	);

	let network_id = NetworkId::random();
	let store_id = StoreId::random();
	let total_size = initial_size + extend_size;
	let sync_config =
		SyncConfig::default().with_fetch_batch_size(fetch_batch_size);

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

	// populate initial data with individual pushes
	let mut ver = w0.version();
	for i in 0..initial_size {
		ver = w0.push_back(i * 10).await?;
	}
	timeout_ms(2000 + 10 * initial_size, w0.when().reaches(ver)).await?;
	timeout_ms(2000 + 10 * initial_size, r1.when().reaches(ver)).await?;
	tracing::info!("initial data ready at version {ver}");

	// bring up r2 — it will need snapshot sync
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;

	let r2 = collections::Vec::<u64>::reader_with_sync_config(
		&n2,
		store_id,
		sync_config,
	);

	// while r2 is syncing the snapshot, do a single large extend
	ver = w0
		.extend((initial_size..total_size).map(|i| i * 10))
		.await?;
	tracing::info!("extend of {extend_size} items done, version: {ver}");

	// wait for convergence
	timeout_ms(5000 + 10 * total_size, w0.when().reaches(ver)).await?;
	timeout_ms(5000 + 10 * total_size, r1.when().reaches(ver)).await?;
	timeout_ms(5000 + 10 * total_size, r2.when().reaches(ver)).await?;

	assert_eq!(w0.len(), total_size as usize);
	assert_eq!(r1.len(), total_size as usize);
	assert_eq!(r2.len(), total_size as usize);

	for i in 0..total_size {
		let expected = i * 10;
		assert_eq!(w0.get(i), Some(expected), "w0 mismatch at {i}");
		assert_eq!(r1.get(i), Some(expected), "r1 mismatch at {i}");
		assert_eq!(r2.get(i), Some(expected), "r2 mismatch at {i}");
	}
	tracing::info!("data consistency verified");
	Ok(())
}

/// Reader syncs a snapshot, then the writer clears the vec and repopulates it.
/// Verifies that all nodes converge to the new data after a destructive
/// mutation.
#[tokio::test]
async fn clear_and_repopulate() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::Vec::<u64>::new(&n0, store_id);
	let r1 = collections::Vec::<u64>::reader(&n1, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	// populate
	let ver1 = w0.extend(0..500u64).await?;
	timeout_ms(5000, r1.when().reaches(ver1)).await?;
	assert_eq!(r1.len(), 500);

	// clear
	let ver2 = w0.clear().await?;
	timeout_ms(5000, r1.when().reaches(ver2)).await?;
	assert_eq!(r1.len(), 0);
	tracing::info!("cleared, version: {ver2}");

	// repopulate with different data
	let ver3 = w0.extend((0..200u64).map(|i| i * 100)).await?;
	timeout_ms(5000, r1.when().reaches(ver3)).await?;

	assert_eq!(w0.len(), 200);
	assert_eq!(r1.len(), 200);
	for i in 0..200u64 {
		assert_eq!(w0.get(i), Some(i * 100));
		assert_eq!(r1.get(i), Some(i * 100));
	}

	// late joiner should see only the final repopulated data
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;
	let r2 = collections::Vec::<u64>::reader(&n2, store_id);
	timeout_s(5, r2.when().online()).await?;
	timeout_ms(5000, r2.when().reaches(ver3)).await?;

	assert_eq!(r2.len(), 200);
	for i in 0..200u64 {
		assert_eq!(r2.get(i), Some(i * 100));
	}
	tracing::info!("clear and repopulate verified");
	Ok(())
}

/// Mixed mutation operations: `push_front`, `push_back`, `insert`, `remove`,
/// `swap`, `pop_front`, `pop_back`, `truncate`. Verifies that all operations
/// replicate correctly across nodes.
#[tokio::test]
async fn mixed_mutations() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::Vec::<u64>::new(&n0, store_id);
	let r1 = collections::Vec::<u64>::reader(&n1, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	// push_back: [10, 20, 30]
	w0.push_back(10).await?;
	w0.push_back(20).await?;
	let mut ver = w0.push_back(30).await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.len(), 3);

	// push_front: [5, 10, 20, 30]
	ver = w0.push_front(5).await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.get(0u64), Some(5));

	// insert at index 2: [5, 10, 15, 20, 30]
	ver = w0.insert(2, 15).await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.get(2u64), Some(15));
	assert_eq!(r1.len(), 5);

	// swap indices 0 and 4: [30, 10, 15, 20, 5]
	ver = w0.swap(0, 4).await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.get(0u64), Some(30));
	assert_eq!(r1.get(4u64), Some(5));

	// pop_back: [30, 10, 15, 20]
	ver = w0.pop_back().await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.len(), 4);
	assert_eq!(r1.back(), Some(20));

	// pop_front: [10, 15, 20]
	ver = w0.pop_front().await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.len(), 3);
	assert_eq!(r1.front(), Some(10));

	// remove index 1: [10, 20]
	ver = w0.remove(1).await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.len(), 2);
	assert_eq!(r1.get(0u64), Some(10));
	assert_eq!(r1.get(1u64), Some(20));

	// extend: [10, 20, 100, 200, 300]
	ver = w0.extend([100, 200, 300]).await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.len(), 5);

	// truncate to 3: [10, 20, 100]
	ver = w0.truncate(3).await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.len(), 3);
	assert_eq!(r1.get(0u64), Some(10));
	assert_eq!(r1.get(1u64), Some(20));
	assert_eq!(r1.get(2u64), Some(100));

	// verify writer and reader agree
	for i in 0..3u64 {
		assert_eq!(w0.get(i), r1.get(i), "mismatch at index {i}");
	}

	// late joiner sees final state
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;
	let r2 = collections::Vec::<u64>::reader(&n2, store_id);
	timeout_s(5, r2.when().online()).await?;
	timeout_ms(5000, r2.when().reaches(ver)).await?;

	assert_eq!(r2.len(), 3);
	assert_eq!(r2.get(0u64), Some(10));
	assert_eq!(r2.get(1u64), Some(20));
	assert_eq!(r2.get(2u64), Some(100));
	tracing::info!("mixed mutations verified across all nodes");
	Ok(())
}

/// Large individual elements: uses Vec<String> with large payloads per element
/// to stress serialization and batch-size logic.
#[tokio::test]
async fn large_elements() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();
	let element_size = 10_000; // 10KB per element
	let count = 100u64;

	let sync_config = SyncConfig::default().with_fetch_batch_size(10);

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::Vec::<String>::new_with_sync_config(
		&n0,
		store_id,
		sync_config.clone(),
	);
	let r1 = collections::Vec::<String>::reader_with_sync_config(
		&n1,
		store_id,
		sync_config.clone(),
	);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	// push large strings
	let mut ver = w0.version();
	for i in 0..count {
		let payload = format!("{i:0>element_size$}");
		ver = w0.push_back(payload).await?;
	}
	tracing::info!("pushed {count} large elements, version: {ver}");

	timeout_ms(30_000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.len(), count as usize);

	for i in 0..count {
		let expected = format!("{i:0>element_size$}");
		assert_eq!(r1.get(i), Some(expected), "mismatch at index {i}");
	}

	// late joiner
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;
	let r2 = collections::Vec::<String>::reader_with_sync_config(
		&n2,
		store_id,
		sync_config,
	);
	timeout_s(5, r2.when().online()).await?;
	timeout_ms(30_000, r2.when().reaches(ver)).await?;

	assert_eq!(r2.len(), count as usize);
	for i in 0..count {
		let expected = format!("{i:0>element_size$}");
		assert_eq!(r2.get(i), Some(expected), "r2 mismatch at index {i}");
	}
	tracing::info!("large elements verified");
	Ok(())
}

/// Drop and recreate a reader on the same store. Verifies that a fresh reader
/// can sync cleanly without stale state corruption from a previous instance.
#[tokio::test]
async fn recreate_reader() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::Vec::<u64>::new(&n0, store_id);
	timeout_s(15, w0.when().online()).await?;

	let ver1 = w0.extend(0..100u64).await?;
	timeout_ms(5000, w0.when().reaches(ver1)).await?;

	// first reader syncs and is dropped
	{
		let r1 = collections::Vec::<u64>::reader(&n1, store_id);
		timeout_s(15, r1.when().online()).await?;
		timeout_ms(5000, r1.when().reaches(ver1)).await?;
		assert_eq!(r1.len(), 100);
		tracing::info!("first reader synced and will be dropped");
	}
	// r1 is dropped here

	// push more data
	let ver2 = w0.extend(100..200u64).await?;
	timeout_ms(5000, w0.when().reaches(ver2)).await?;

	// recreate reader on the same node and store
	let r1_new = collections::Vec::<u64>::reader(&n1, store_id);
	timeout_s(15, r1_new.when().online()).await?;
	timeout_ms(10_000, r1_new.when().reaches(ver2)).await?;

	assert_eq!(r1_new.len(), 200);
	for i in 0..200u64 {
		assert_eq!(r1_new.get(i), Some(i), "mismatch at index {i}");
	}
	tracing::info!("recreated reader verified");
	Ok(())
}
