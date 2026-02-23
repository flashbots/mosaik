use {
	crate::utils::{discover_all, timeout_ms, timeout_s},
	mosaik::{collections::SyncConfig, primitives::Short, *},
	rstest::rstest,
};

/// Populate a priority queue on two nodes, then bring up a third node that
/// needs to sync the snapshot.
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

	let w0 = collections::PriorityQueue::<u64, u64, u64>::new_with_config(
		&n0,
		store_id,
		sync_config.clone(),
	);

	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader_with_config(
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
		ver = w0.extend((0..data_size).map(|i| (i, i, i * 10))).await?;
	} else {
		for i in 0..data_size {
			ver = w0.insert(i, i, i * 10).await?;
		}
	}

	let elapsed = start.elapsed();
	tracing::info!("depq populated in {elapsed:?}, final version: {ver}");

	timeout_ms(2000 + 10 * data_size, w0.when().reaches(ver)).await?;
	tracing::info!("w0 state committed version {ver} in {:?}", start.elapsed());

	timeout_ms(2000 + 10 * data_size, r1.when().reaches(ver)).await?;
	tracing::info!("r1 state committed version {ver} in {:?}", start.elapsed());

	assert_eq!(r1.len(), data_size as usize);
	assert_eq!(r1.get(&0), Some(0));

	for i in 0..data_size {
		assert_eq!(w0.get(&i), Some(i * 10), "w0 mismatch at key {i}");
		assert_eq!(r1.get(&i), Some(i * 10), "r1 mismatch at key {i}");
		assert_eq!(
			w0.get_priority(&i),
			Some(i),
			"w0 priority mismatch at key {i}"
		);
		assert_eq!(
			r1.get_priority(&i),
			Some(i),
			"r1 priority mismatch at key {i}"
		);
	}

	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;
	tracing::info!("r2 joining the network as {}", Short(n2.local().id()));

	let r2 = collections::PriorityQueue::<u64, u64, u64>::reader_with_config(
		&n2,
		store_id,
		sync_config.clone(),
	);
	timeout_s(5, r2.when().online()).await?;
	timeout_ms(2000 + 10 * data_size, r2.when().reaches(ver)).await?;
	tracing::info!("r2 state reached version {ver}");

	assert_eq!(r2.len(), data_size as usize);
	assert_eq!(r2.get(&0), Some(0));

	for i in 0..data_size {
		assert_eq!(r2.get(&i), Some(i * 10), "r2 mismatch at key {i}");
		assert_eq!(
			r2.get_priority(&i),
			Some(i),
			"r2 priority mismatch at key {i}"
		);
	}

	// verify that all data on all nodes is the same after the sync
	assert_eq!(w0.len(), data_size as usize);
	assert_eq!(r1.len(), data_size as usize);
	assert_eq!(r2.len(), data_size as usize);

	// verify min/max are consistent
	if data_size > 0 {
		assert_eq!(w0.min_priority(), r2.min_priority());
		assert_eq!(w0.max_priority(), r2.max_priority());
	}
	tracing::info!("data consistency verified across all nodes");

	Ok(())
}

/// Similar to `frozen`, but new data keeps arriving from the writer while `r2`
/// is syncing a snapshot. This ensures that the snapshot sync mechanism handles
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

	let w0 = collections::PriorityQueue::<u64, u64, u64>::new_with_config(
		&n0,
		store_id,
		sync_config.clone(),
	);

	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader_with_config(
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
		ver = w0.extend((0..initial_size).map(|i| (i, i, i * 10))).await?;
	} else {
		for i in 0..initial_size {
			ver = w0.insert(i, i, i * 10).await?;
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

	let r2 = collections::PriorityQueue::<u64, u64, u64>::reader_with_config(
		&n2,
		store_id,
		sync_config.clone(),
	);

	// insert more data while r2 is syncing the snapshot
	for i in initial_size..total_size {
		ver = w0.insert(i, i, i * 10).await?;
	}
	tracing::info!(
		"extra data ({extra_size} items) inserted, final version: {ver}"
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
		assert_eq!(w0.get(&i), Some(expected), "w0 mismatch at key {i}");
		assert_eq!(r1.get(&i), Some(expected), "r1 mismatch at key {i}");
		assert_eq!(r2.get(&i), Some(expected), "r2 mismatch at key {i}");
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

	let w0 = collections::PriorityQueue::<u64, u64, u64>::new(&n0, store_id);
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	// No data inserted — reader should converge to empty
	assert_eq!(r1.len(), 0);
	assert!(r1.is_empty());
	assert_eq!(r1.get(&0), None);
	assert_eq!(r1.get_min(), None);
	assert_eq!(r1.get_max(), None);
	Ok(())
}

/// Single-entry edge case: ensures snapshot/batch logic handles the minimum
/// non-empty case without off-by-one errors.
#[tokio::test]
async fn single_entry() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::PriorityQueue::<u64, u64, u64>::new(&n0, store_id);
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	let ver = w0.insert(42, 1, 100).await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;

	assert_eq!(r1.len(), 1);
	assert_eq!(r1.get(&1), Some(100));
	assert_eq!(r1.get_priority(&1), Some(42));
	assert_eq!(r1.get_min(), Some((42, 1, 100)));
	assert_eq!(r1.get_max(), Some((42, 1, 100)));
	assert_eq!(r1.get(&0), None);

	// late joiner also gets the single entry
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;
	let r2 = collections::PriorityQueue::<u64, u64, u64>::reader(&n2, store_id);
	timeout_s(5, r2.when().online()).await?;
	timeout_ms(5000, r2.when().reaches(ver)).await?;

	assert_eq!(r2.len(), 1);
	assert_eq!(r2.get(&1), Some(100));
	assert_eq!(r2.get_priority(&1), Some(42));
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
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);

	// writer second
	let w0 = collections::PriorityQueue::<u64, u64, u64>::new(&n0, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	let ver = w0.extend((0..100u64).map(|i| (i, i, i * 10))).await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;

	assert_eq!(r1.len(), 100);
	for i in 0..100u64 {
		assert_eq!(r1.get(&i), Some(i * 10));
		assert_eq!(r1.get_priority(&i), Some(i));
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

	let w0 = collections::PriorityQueue::<u64, u64, u64>::new_with_config(
		&n0,
		store_id,
		sync_config.clone(),
	);
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader_with_config(
		&n1,
		store_id,
		sync_config.clone(),
	);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	let ver = w0.extend((0..data_size).map(|i| (i, i, i * 10))).await?;
	timeout_ms(5000 + 10 * data_size, r1.when().reaches(ver)).await?;

	assert_eq!(r1.len(), data_size as usize);
	for i in 0..data_size {
		assert_eq!(r1.get(&i), Some(i * 10));
	}

	// verify late joiner with same extreme batch size
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;
	let r2 = collections::PriorityQueue::<u64, u64, u64>::reader_with_config(
		&n2,
		store_id,
		sync_config,
	);
	timeout_s(5, r2.when().online()).await?;
	timeout_ms(5000 + 10 * data_size, r2.when().reaches(ver)).await?;

	assert_eq!(r2.len(), data_size as usize);
	for i in 0..data_size {
		assert_eq!(r2.get(&i), Some(i * 10));
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

	let w0 = collections::PriorityQueue::<u64, u64, u64>::new(&n0, store_id);
	timeout_s(15, w0.when().online()).await?;

	let ver = w0.extend((0..data_size).map(|i| (i, i, i * 10))).await?;
	timeout_ms(5000, w0.when().reaches(ver)).await?;
	tracing::info!("writer populated {data_size} entries, version: {ver}");

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
		.map(|n| collections::PriorityQueue::<u64, u64, u64>::reader(n, store_id))
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
			assert_eq!(r.get(&j), Some(j * 10), "reader {i} mismatch at key {j}");
		}
	}
	tracing::info!("all {num_readers} readers have correct data");
	Ok(())
}

/// After full convergence, insert more data and verify all existing readers
/// pick up the new entries via incremental sync (not a new snapshot).
#[tokio::test]
async fn incremental_after_convergence() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1, &n2])).await??;

	let w0 = collections::PriorityQueue::<u64, u64, u64>::new(&n0, store_id);
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	let r2 = collections::PriorityQueue::<u64, u64, u64>::reader(&n2, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;
	timeout_s(15, r2.when().online()).await?;

	// initial data
	let ver1 = w0.extend((0..500u64).map(|i| (i, i, i))).await?;
	timeout_ms(5000, r1.when().reaches(ver1)).await?;
	timeout_ms(5000, r2.when().reaches(ver1)).await?;
	tracing::info!("phase 1 converged at version {ver1}");

	assert_eq!(r1.len(), 500);
	assert_eq!(r2.len(), 500);

	// insert more data after convergence
	let mut ver2 = ver1;
	for i in 500..1000u64 {
		ver2 = w0.insert(i, i, i).await?;
	}
	tracing::info!("phase 2: inserted 500 more entries, version: {ver2}");

	timeout_ms(10_000, r1.when().reaches(ver2)).await?;
	timeout_ms(10_000, r2.when().reaches(ver2)).await?;

	assert_eq!(w0.len(), 1000);
	assert_eq!(r1.len(), 1000);
	assert_eq!(r2.len(), 1000);

	for i in 0..1000u64 {
		assert_eq!(w0.get(&i), Some(i), "w0 mismatch at key {i}");
		assert_eq!(r1.get(&i), Some(i), "r1 mismatch at key {i}");
		assert_eq!(r2.get(&i), Some(i), "r2 mismatch at key {i}");
	}
	tracing::info!("incremental sync verified");
	Ok(())
}

/// Concurrent reads while sync is in progress. Spawns a task that continuously
/// reads from r2 during snapshot sync and verifies no panics or inconsistent
/// intermediate states (e.g. `len()` returns N but iterating yields fewer).
#[tokio::test]
async fn concurrent_reads_during_sync() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();
	let data_size = 10_000u64;

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::PriorityQueue::<u64, u64, u64>::new(&n0, store_id);
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	let ver = w0.extend((0..data_size).map(|i| (i, i, i * 10))).await?;
	timeout_ms(10_000, r1.when().reaches(ver)).await?;
	tracing::info!("initial data ready, version: {ver}");

	// bring up r2 — it will snapshot-sync
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;

	let r2 = collections::PriorityQueue::<u64, u64, u64>::reader(&n2, store_id);

	// wait for r2 to come online before spawning the reader task
	timeout_s(15, r2.when().online()).await?;

	// spawn a task that continuously reads during sync
	let reader_task = tokio::spawn({
		async move {
			let mut read_count = 0u64;
			loop {
				let len = r2.len();
				if len > 0 {
					// spot-check: key 0 should always be present once queue is non-empty
					let first = r2.get(&0);
					assert!(
						first.is_some(),
						"len={len} but get(&0) returned None (read #{read_count})",
					);

					// contains_key should agree with get
					assert!(
						r2.contains_key(&0),
						"len={len} but contains_key(&0) is false (read #{read_count})",
					);

					// min should exist when non-empty
					assert!(
						r2.get_min().is_some(),
						"len={len} but get_min() returned None (read #{read_count})",
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
/// Writer creates initial entries, reader starts syncing, then writer does a
/// single `extend` of more entries. Tests the transition from snapshot-sync to
/// log-replay when the log contains a multi-entry command.
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

	let w0 = collections::PriorityQueue::<u64, u64, u64>::new_with_config(
		&n0,
		store_id,
		sync_config.clone(),
	);
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader_with_config(
		&n1,
		store_id,
		sync_config.clone(),
	);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	// populate initial data with individual inserts
	let mut ver = w0.version();
	for i in 0..initial_size {
		ver = w0.insert(i, i, i * 10).await?;
	}
	timeout_ms(2000 + 10 * initial_size, w0.when().reaches(ver)).await?;
	timeout_ms(2000 + 10 * initial_size, r1.when().reaches(ver)).await?;
	tracing::info!("initial data ready at version {ver}");

	// bring up r2 — it will need snapshot sync
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;

	let r2 = collections::PriorityQueue::<u64, u64, u64>::reader_with_config(
		&n2,
		store_id,
		sync_config,
	);

	// while r2 is syncing the snapshot, do a single large extend
	ver = w0
		.extend((initial_size..total_size).map(|i| (i, i, i * 10)))
		.await?;
	tracing::info!("extend of {extend_size} entries done, version: {ver}");

	// wait for convergence
	timeout_ms(5000 + 10 * total_size, w0.when().reaches(ver)).await?;
	timeout_ms(5000 + 10 * total_size, r1.when().reaches(ver)).await?;
	timeout_ms(5000 + 10 * total_size, r2.when().reaches(ver)).await?;

	assert_eq!(w0.len(), total_size as usize);
	assert_eq!(r1.len(), total_size as usize);
	assert_eq!(r2.len(), total_size as usize);

	for i in 0..total_size {
		let expected = i * 10;
		assert_eq!(w0.get(&i), Some(expected), "w0 mismatch at key {i}");
		assert_eq!(r1.get(&i), Some(expected), "r1 mismatch at key {i}");
		assert_eq!(r2.get(&i), Some(expected), "r2 mismatch at key {i}");
	}
	tracing::info!("data consistency verified");
	Ok(())
}

/// Reader syncs a snapshot, then the writer clears the queue and repopulates
/// it. Verifies that all nodes converge to the new data after a destructive
/// mutation.
#[tokio::test]
async fn clear_and_repopulate() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::PriorityQueue::<u64, u64, u64>::new(&n0, store_id);
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	// populate
	let ver1 = w0.extend((0..500u64).map(|i| (i, i, i))).await?;
	timeout_ms(5000, r1.when().reaches(ver1)).await?;
	assert_eq!(r1.len(), 500);

	// clear
	let ver2 = w0.clear().await?;
	timeout_ms(5000, r1.when().reaches(ver2)).await?;
	assert_eq!(r1.len(), 0);
	assert!(r1.is_empty());
	assert_eq!(r1.get_min(), None);
	assert_eq!(r1.get_max(), None);
	tracing::info!("cleared, version: {ver2}");

	// repopulate with different data
	let ver3 = w0.extend((0..200u64).map(|i| (i * 2, i, i * 100))).await?;
	timeout_ms(5000, r1.when().reaches(ver3)).await?;

	assert_eq!(w0.len(), 200);
	assert_eq!(r1.len(), 200);
	for i in 0..200u64 {
		assert_eq!(w0.get(&i), Some(i * 100));
		assert_eq!(r1.get(&i), Some(i * 100));
		assert_eq!(r1.get_priority(&i), Some(i * 2));
	}

	// late joiner should see only the final repopulated data
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;
	let r2 = collections::PriorityQueue::<u64, u64, u64>::reader(&n2, store_id);
	timeout_s(5, r2.when().online()).await?;
	timeout_ms(5000, r2.when().reaches(ver3)).await?;

	assert_eq!(r2.len(), 200);
	for i in 0..200u64 {
		assert_eq!(r2.get(&i), Some(i * 100));
		assert_eq!(r2.get_priority(&i), Some(i * 2));
	}
	tracing::info!("clear and repopulate verified");
	Ok(())
}

/// Mixed mutation operations: `insert`, `extend`, `update_priority`,
/// `update_value`, `remove`, `clear`.
/// Verifies that all operations replicate correctly across nodes.
#[tokio::test]
async fn mixed_mutations() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::PriorityQueue::<u64, u64, u64>::new(&n0, store_id);
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	// insert: {key=1 pri=10 val=100}, {key=2 pri=20 val=200}, {key=3 pri=30
	// val=300}
	w0.insert(10, 1, 100).await?;
	w0.insert(20, 2, 200).await?;
	let mut ver = w0.insert(30, 3, 300).await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.len(), 3);
	assert_eq!(r1.get(&1), Some(100));
	assert_eq!(r1.get_priority(&1), Some(10));
	assert_eq!(r1.get_min(), Some((10, 1, 100)));
	assert_eq!(r1.get_max(), Some((30, 3, 300)));

	// update_priority: key=1 now has priority 50 (becomes max)
	ver = w0.update_priority(&1, 50).await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.get_priority(&1), Some(50));
	assert_eq!(r1.get(&1), Some(100)); // value unchanged
	assert_eq!(r1.get_max(), Some((50, 1, 100)));
	assert_eq!(r1.get_min(), Some((20, 2, 200)));
	assert_eq!(r1.len(), 3); // length unchanged

	// update_value: key=2 now has value 999
	ver = w0.update_value(&2, 999).await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.get(&2), Some(999));
	assert_eq!(r1.get_priority(&2), Some(20)); // priority unchanged

	// remove: remove key=3
	ver = w0.remove(&3).await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.len(), 2);
	assert_eq!(r1.get(&3), None);
	assert!(!r1.contains_key(&3));

	// extend: add more entries
	ver = w0
		.extend([(5, 10, 500), (15, 20, 600), (25, 30, 700)])
		.await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.len(), 5);
	assert_eq!(r1.get(&10), Some(500));
	assert_eq!(r1.get(&20), Some(600));
	assert_eq!(r1.get(&30), Some(700));

	// verify writer and reader agree
	for key in [1u64, 2, 10, 20, 30] {
		assert_eq!(w0.get(&key), r1.get(&key), "mismatch at key {key}");
		assert_eq!(
			w0.get_priority(&key),
			r1.get_priority(&key),
			"priority mismatch at key {key}"
		);
	}

	// late joiner sees final state
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;
	let r2 = collections::PriorityQueue::<u64, u64, u64>::reader(&n2, store_id);
	timeout_s(5, r2.when().online()).await?;
	timeout_ms(5000, r2.when().reaches(ver)).await?;

	assert_eq!(r2.len(), 5);
	assert_eq!(r2.get(&1), Some(100));
	assert_eq!(r2.get_priority(&1), Some(50));
	assert_eq!(r2.get(&2), Some(999));
	assert_eq!(r2.get(&3), None);
	assert_eq!(r2.get(&10), Some(500));
	assert_eq!(r2.get(&20), Some(600));
	assert_eq!(r2.get(&30), Some(700));
	tracing::info!("mixed mutations verified across all nodes");
	Ok(())
}

/// Large individual values: uses `PriorityQueue<u64, u64, String>` with large
/// payloads per entry to stress serialization and batch-size logic.
#[tokio::test]
async fn large_values() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();
	let element_size = 10_000; // 10KB per value
	let count = 100u64;

	let sync_config = SyncConfig::default().with_fetch_batch_size(10);

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::PriorityQueue::<u64, u64, String>::new_with_config(
		&n0,
		store_id,
		sync_config.clone(),
	);
	let r1 = collections::PriorityQueue::<u64, u64, String>::reader_with_config(
		&n1,
		store_id,
		sync_config.clone(),
	);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	// insert large strings
	let mut ver = w0.version();
	for i in 0..count {
		let payload = format!("{i:0>element_size$}");
		ver = w0.insert(i, i, payload).await?;
	}
	tracing::info!("inserted {count} large entries, version: {ver}");

	timeout_ms(30_000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.len(), count as usize);

	for i in 0..count {
		let expected = format!("{i:0>element_size$}");
		assert_eq!(r1.get(&i), Some(expected), "mismatch at key {i}");
	}

	// late joiner
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;
	let r2 = collections::PriorityQueue::<u64, u64, String>::reader_with_config(
		&n2,
		store_id,
		sync_config,
	);
	timeout_s(5, r2.when().online()).await?;
	timeout_ms(30_000, r2.when().reaches(ver)).await?;

	assert_eq!(r2.len(), count as usize);
	for i in 0..count {
		let expected = format!("{i:0>element_size$}");
		assert_eq!(r2.get(&i), Some(expected), "r2 mismatch at key {i}");
	}
	tracing::info!("large values verified");
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

	let w0 = collections::PriorityQueue::<u64, u64, u64>::new(&n0, store_id);
	timeout_s(15, w0.when().online()).await?;

	let ver1 = w0.extend((0..100u64).map(|i| (i, i, i))).await?;
	timeout_ms(5000, w0.when().reaches(ver1)).await?;

	// first reader syncs and is dropped
	{
		let r1 = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
		timeout_s(15, r1.when().online()).await?;
		timeout_ms(5000, r1.when().reaches(ver1)).await?;
		assert_eq!(r1.len(), 100);
		tracing::info!("first reader synced and will be dropped");
	}
	// r1 is dropped here

	// insert more data
	let ver2 = w0.extend((100..200u64).map(|i| (i, i, i))).await?;
	timeout_ms(5000, w0.when().reaches(ver2)).await?;

	// recreate reader on the same node and store
	let r1_new =
		collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(15, r1_new.when().online()).await?;
	timeout_ms(10_000, r1_new.when().reaches(ver2)).await?;

	assert_eq!(r1_new.len(), 200);
	for i in 0..200u64 {
		assert_eq!(r1_new.get(&i), Some(i), "mismatch at key {i}");
	}
	tracing::info!("recreated reader verified");
	Ok(())
}

/// Insert duplicate keys: the last value for a given key wins. Verifies that
/// updates to existing keys are replicated correctly and the queue converges to
/// the expected state.
#[tokio::test]
async fn overwrite_keys() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::PriorityQueue::<u64, u64, u64>::new(&n0, store_id);
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	// insert initial values
	let mut ver = w0.version();
	for i in 0..100u64 {
		ver = w0.insert(i, i, i).await?;
	}
	timeout_ms(5000, r1.when().reaches(ver)).await?;
	assert_eq!(r1.len(), 100);

	// overwrite all keys with new priorities and values
	for i in 0..100u64 {
		ver = w0.insert(i * 1000, i, i * 1000).await?;
	}
	timeout_ms(10_000, r1.when().reaches(ver)).await?;

	// length should still be 100 — no new keys were added
	assert_eq!(w0.len(), 100);
	assert_eq!(r1.len(), 100);

	for i in 0..100u64 {
		assert_eq!(w0.get(&i), Some(i * 1000), "w0 mismatch at key {i}");
		assert_eq!(r1.get(&i), Some(i * 1000), "r1 mismatch at key {i}");
		assert_eq!(
			w0.get_priority(&i),
			Some(i * 1000),
			"w0 priority mismatch at key {i}"
		);
		assert_eq!(
			r1.get_priority(&i),
			Some(i * 1000),
			"r1 priority mismatch at key {i}"
		);
	}

	// late joiner sees only the latest values
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;
	let r2 = collections::PriorityQueue::<u64, u64, u64>::reader(&n2, store_id);
	timeout_s(5, r2.when().online()).await?;
	timeout_ms(5000, r2.when().reaches(ver)).await?;

	assert_eq!(r2.len(), 100);
	for i in 0..100u64 {
		assert_eq!(r2.get(&i), Some(i * 1000), "r2 mismatch at key {i}");
		assert_eq!(
			r2.get_priority(&i),
			Some(i * 1000),
			"r2 priority mismatch at key {i}"
		);
	}
	tracing::info!("overwrite keys verified");
	Ok(())
}

/// Verify that `remove_range` operations replicate correctly and the late
/// joiner sees the final state after range removals.
#[tokio::test]
async fn remove_range_sync() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::PriorityQueue::<u64, u64, u64>::new(&n0, store_id);
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	// insert entries with priorities 0..100
	let ver1 = w0.extend((0..100u64).map(|i| (i, i, i * 10))).await?;
	timeout_ms(5000, r1.when().reaches(ver1)).await?;
	assert_eq!(r1.len(), 100);

	// remove entries with priority in [20, 50)
	let ver2 = w0.remove_range(20u64..50u64).await?;
	timeout_ms(5000, r1.when().reaches(ver2)).await?;

	// should have 70 entries left (0..20 + 50..100)
	assert_eq!(r1.len(), 70);
	for i in 0..20u64 {
		assert!(r1.contains_key(&i), "expected key {i} present");
	}
	for i in 20..50u64 {
		assert!(!r1.contains_key(&i), "expected key {i} removed");
	}
	for i in 50..100u64 {
		assert!(r1.contains_key(&i), "expected key {i} present");
	}

	// late joiner should see the range-removed state
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;
	let r2 = collections::PriorityQueue::<u64, u64, u64>::reader(&n2, store_id);
	timeout_s(5, r2.when().online()).await?;
	timeout_ms(5000, r2.when().reaches(ver2)).await?;

	assert_eq!(r2.len(), 70);
	for i in 20..50u64 {
		assert!(!r2.contains_key(&i), "r2: expected key {i} removed");
	}
	tracing::info!("remove_range sync verified");
	Ok(())
}

/// Verify that min/max priorities are consistent across all nodes after
/// snapshot sync of a queue with non-contiguous priorities.
#[tokio::test]
async fn min_max_after_sync() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(3, discover_all([&n0, &n1])).await??;

	let w0 = collections::PriorityQueue::<u64, u64, u64>::new(&n0, store_id);
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);

	timeout_s(15, w0.when().online()).await?;
	timeout_s(15, r1.when().online()).await?;

	// insert entries with non-contiguous, scattered priorities
	let ver = w0
		.extend([
			(500, 1, 10),
			(100, 2, 20),
			(999, 3, 30),
			(1, 4, 40),
			(750, 5, 50),
		])
		.await?;
	timeout_ms(5000, r1.when().reaches(ver)).await?;

	assert_eq!(r1.min_priority(), Some(1));
	assert_eq!(r1.max_priority(), Some(999));
	assert_eq!(r1.get_min(), Some((1, 4, 40)));
	assert_eq!(r1.get_max(), Some((999, 3, 30)));

	// late joiner
	let n2 = Network::new(network_id).await?;
	discover_all([&n0, &n1, &n2]).await?;
	let r2 = collections::PriorityQueue::<u64, u64, u64>::reader(&n2, store_id);
	timeout_s(5, r2.when().online()).await?;
	timeout_ms(5000, r2.when().reaches(ver)).await?;

	assert_eq!(r2.min_priority(), Some(1));
	assert_eq!(r2.max_priority(), Some(999));
	assert_eq!(r2.get_min(), Some((1, 4, 40)));
	assert_eq!(r2.get_max(), Some((999, 3, 30)));
	assert_eq!(r2.len(), 5);
	tracing::info!("min/max after sync verified");
	Ok(())
}
