use {
	crate::utils::{discover_all, timeout_s},
	mosaik::{collections::StoreId, *},
};

// Basic smoke: writer + reader, all read/write operations, no late join
#[tokio::test]
async fn smoke_no_catchup() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::new(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// -- empty state --
	assert!(r.is_empty());
	assert_eq!(r.len(), 0);
	assert_eq!(r.get(&1), None);
	assert_eq!(r.get_priority(&1), None);
	assert!(!r.contains_key(&1));
	assert_eq!(r.get_min(), None);
	assert_eq!(r.get_max(), None);
	assert_eq!(r.min_priority(), None);
	assert_eq!(r.max_priority(), None);
	assert_eq!(r.iter().count(), 0);

	// -- insert --
	let ver = timeout_s(2, w.insert(10, 1, 100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);
	assert!(!r.is_empty());
	assert_eq!(r.get(&1), Some(100));
	assert_eq!(r.get_priority(&1), Some(10));
	assert!(r.contains_key(&1));
	assert!(!r.contains_key(&2));

	// -- min/max on single entry --
	assert_eq!(r.get_min(), Some((10, 1, 100)));
	assert_eq!(r.get_max(), Some((10, 1, 100)));
	assert_eq!(r.min_priority(), Some(10));
	assert_eq!(r.max_priority(), Some(10));

	// -- insert more entries with different priorities --
	let ver = timeout_s(2, w.insert(5, 2, 200)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	let ver = timeout_s(2, w.insert(20, 3, 300)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);

	// -- min/max with multiple entries --
	assert_eq!(r.get_min(), Some((5, 2, 200)));
	assert_eq!(r.get_max(), Some((20, 3, 300)));
	assert_eq!(r.min_priority(), Some(5));
	assert_eq!(r.max_priority(), Some(20));

	// -- insert overwrites existing key --
	let ver = timeout_s(2, w.insert(15, 1, 150)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3); // length unchanged
	assert_eq!(r.get(&1), Some(150)); // value updated
	assert_eq!(r.get_priority(&1), Some(15)); // priority updated

	// -- extend --
	let ver =
		timeout_s(2, w.extend(vec![(1, 4, 400), (30, 5, 500), (8, 6, 600)]))
			.await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 6);
	assert_eq!(r.get(&4), Some(400));
	assert_eq!(r.get_priority(&4), Some(1));
	assert_eq!(r.get(&5), Some(500));
	assert_eq!(r.get_priority(&5), Some(30));

	// -- min/max after extend --
	assert_eq!(r.min_priority(), Some(1)); // key 4 has priority 1
	assert_eq!(r.max_priority(), Some(30)); // key 5 has priority 30

	// -- iter ascending --
	let asc: Vec<(u64, u64, u64)> = r.iter_asc().collect();
	let priorities: Vec<u64> = asc.iter().map(|(p, _, _)| *p).collect();
	assert!(priorities.windows(2).all(|w| w[0] <= w[1]));
	assert_eq!(asc.len(), 6);

	// -- iter descending --
	let desc: Vec<(u64, u64, u64)> = r.iter_desc().collect();
	let priorities: Vec<u64> = desc.iter().map(|(p, _, _)| *p).collect();
	assert!(priorities.windows(2).all(|w| w[0] >= w[1]));
	assert_eq!(desc.len(), 6);

	// -- contains_key --
	assert!(r.contains_key(&4));
	assert!(!r.contains_key(&99));

	// -- remove --
	let ver = timeout_s(2, w.remove(&4)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 5);
	assert_eq!(r.get(&4), None);
	assert!(!r.contains_key(&4));
	// min_priority should change since key 4 had priority 1
	assert_eq!(r.min_priority(), Some(5));

	// -- clear --
	let ver = timeout_s(2, w.clear()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());
	assert_eq!(r.len(), 0);
	assert_eq!(r.get_min(), None);
	assert_eq!(r.get_max(), None);

	Ok(())
}

// Late-joining reader catches up with full log replay
#[tokio::test]
async fn catchup_single_reader() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	// build a non-trivial history: insert 100 entries
	for i in 0u64..100 {
		timeout_s(2, w.insert(i * 10, i, i * 100)).await??;
	}

	let ver = timeout_s(
		2,
		w.extend([(1000, 100, 10000), (1010, 101, 10100), (1020, 102, 10200)]),
	)
	.await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	// late-joining reader
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, r.when().online()).await?;
	timeout_s(10, r.when().reaches(ver)).await?;

	assert_eq!(r.len(), 103);
	for i in 0u64..100 {
		assert_eq!(r.get(&i), Some(i * 100));
		assert_eq!(r.get_priority(&i), Some(i * 10));
	}
	assert_eq!(r.get(&100), Some(10000));
	assert_eq!(r.get(&101), Some(10100));
	assert_eq!(r.get(&102), Some(10200));

	// min should be key 0 with priority 0
	assert_eq!(r.min_priority(), Some(0));
	// max should be key 102 with priority 1020
	assert_eq!(r.max_priority(), Some(1020));

	// writer can continue operating after reader caught up
	let ver = timeout_s(2, w.remove(&50)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 102);
	assert_eq!(r.get(&50), None);

	Ok(())
}

// Multiple readers all converge on the same state
#[tokio::test]
async fn multiple_readers() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n_w = Network::new(network_id).await?;
	let n_r1 = Network::new(network_id).await?;
	let n_r2 = Network::new(network_id).await?;
	let n_r3 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n_w, &n_r1, &n_r2, &n_r3])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n_w, store_id);
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader(&n_r1, store_id);
	let r2 = collections::PriorityQueue::<u64, u64, u64>::reader(&n_r2, store_id);
	let r3 = collections::PriorityQueue::<u64, u64, u64>::reader(&n_r3, store_id);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r1.when().online()).await?;
	timeout_s(10, r2.when().online()).await?;
	timeout_s(10, r3.when().online()).await?;

	// insert a batch
	let ver = timeout_s(
		2,
		w.extend(vec![
			(10, 1, 100),
			(20, 2, 200),
			(30, 3, 300),
			(40, 4, 400),
			(50, 5, 500),
		]),
	)
	.await??;

	// all readers converge
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert_eq!(r.len(), 5);
		assert_eq!(r.min_priority(), Some(10));
		assert_eq!(r.max_priority(), Some(50));
		assert_eq!(r.get(&1), Some(100));
		assert_eq!(r.get(&5), Some(500));
	}

	// more mutations replicate to all
	let ver = timeout_s(2, w.insert(60, 6, 600)).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert_eq!(r.get(&6), Some(600));
		assert_eq!(r.len(), 6);
		assert_eq!(r.max_priority(), Some(60));
	}

	let ver = timeout_s(2, w.remove(&1)).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert_eq!(r.len(), 5);
		assert_eq!(r.get(&1), None);
		assert_eq!(r.min_priority(), Some(20));
	}

	Ok(())
}

// Multiple writers converge to the same state
#[tokio::test]
async fn multiple_writers() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1, &n2])).await??;

	let w0 = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let w1 = collections::PriorityQueue::<u64, u64, u64>::writer(&n1, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n2, store_id);

	timeout_s(10, w0.when().online()).await?;
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// both writers insert values (sequentially to avoid races in the test itself)
	let v0 = timeout_s(2, w0.insert(10, 1, 100)).await??;
	timeout_s(2, r.when().reaches(v0)).await?;

	let v1 = timeout_s(2, w1.insert(20, 2, 200)).await??;
	timeout_s(2, r.when().reaches(v1)).await?;

	let v2 = timeout_s(2, w0.insert(30, 3, 300)).await??;
	timeout_s(2, r.when().reaches(v2)).await?;

	assert_eq!(r.len(), 3);
	assert_eq!(r.get(&1), Some(100));
	assert_eq!(r.get(&2), Some(200));
	assert_eq!(r.get(&3), Some(300));

	// a writer that extends
	let v3 = timeout_s(2, w1.extend(vec![(40, 4, 400), (50, 5, 500)])).await??;
	timeout_s(2, r.when().reaches(v3)).await?;

	assert_eq!(r.len(), 5);
	assert_eq!(r.get(&4), Some(400));
	assert_eq!(r.get(&5), Some(500));

	timeout_s(2, w0.when().reaches(v3)).await?;
	timeout_s(2, w1.when().reaches(v3)).await?;

	// all three nodes agree on the snapshot
	let snap_w0: Vec<(u64, u64, u64)> = w0.iter_asc().collect();
	let snap_w1: Vec<(u64, u64, u64)> = w1.iter_asc().collect();
	let snap_r: Vec<(u64, u64, u64)> = r.iter_asc().collect();
	assert_eq!(snap_w0, snap_r);
	assert_eq!(snap_w1, snap_r);

	Ok(())
}

// Multiple late-joining readers catch up independently
#[tokio::test]
async fn catchup_multiple_readers() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n_w = Network::new(network_id).await?;
	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n_w, store_id);
	timeout_s(10, w.when().online()).await?;

	// build history
	for i in 0u64..50 {
		timeout_s(2, w.insert(i * 10, i, i * 100)).await??;
	}
	let mid_ver = timeout_s(2, w.insert(9990, 999, 99_900)).await??;
	timeout_s(2, w.when().reaches(mid_ver)).await?;

	// first late reader
	let n_r1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n_w, &n_r1])).await??;
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader(&n_r1, store_id);
	timeout_s(10, r1.when().online()).await?;
	timeout_s(10, r1.when().reaches(mid_ver)).await?;
	assert_eq!(r1.len(), 51);
	assert_eq!(r1.get(&999), Some(99_900));

	// add more entries while first reader is alive
	for i in 50u64..80 {
		timeout_s(2, w.insert(i * 10, i, i * 100)).await??;
	}
	let late_ver = timeout_s(2, w.insert(10000, 1000, 100_000)).await??;
	timeout_s(2, w.when().reaches(late_ver)).await?;

	// second late reader joins even later
	let n_r2 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n_w, &n_r1, &n_r2])).await??;
	let r2 = collections::PriorityQueue::<u64, u64, u64>::reader(&n_r2, store_id);
	timeout_s(10, r2.when().online()).await?;
	timeout_s(10, r2.when().reaches(late_ver)).await?;

	// first reader should also converge to latest
	timeout_s(10, r1.when().reaches(late_ver)).await?;

	assert_eq!(r1.len(), 82);
	assert_eq!(r2.len(), 82);
	assert_eq!(r1.get(&1000), Some(100_000));
	assert_eq!(r2.get(&1000), Some(100_000));

	// snapshots match
	let snap1: Vec<(u64, u64, u64)> = r1.iter_asc().collect();
	let snap2: Vec<(u64, u64, u64)> = r2.iter_asc().collect();
	assert_eq!(snap1, snap2);

	Ok(())
}

// insert overwrites existing key's priority and value
#[tokio::test]
async fn insert_overwrite() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.insert(10, 1, 100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(100));
	assert_eq!(r.get_priority(&1), Some(10));

	// overwrite with new priority and value
	let ver = timeout_s(2, w.insert(99, 1, 999)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(999));
	assert_eq!(r.get_priority(&1), Some(99));
	assert_eq!(r.len(), 1); // still only one entry

	// old priority bucket should be cleaned up
	assert_eq!(r.min_priority(), Some(99));
	assert_eq!(r.max_priority(), Some(99));

	Ok(())
}

// update_priority changes priority while keeping value
#[tokio::test]
async fn update_priority() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver =
		timeout_s(2, w.extend(vec![(10, 1, 100), (20, 2, 200), (30, 3, 300)]))
			.await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// update priority of key 1 from 10 to 50
	let ver = timeout_s(2, w.update_priority(&1, 50)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	assert_eq!(r.get(&1), Some(100)); // value unchanged
	assert_eq!(r.get_priority(&1), Some(50)); // priority changed
	assert_eq!(r.len(), 3);

	// min should now be key 2 with priority 20 (key 1 moved from 10 to 50)
	assert_eq!(r.min_priority(), Some(20));
	// max should now be key 1 with priority 50
	assert_eq!(r.max_priority(), Some(50));

	// update_priority on non-existent key is a no-op
	let ver = timeout_s(2, w.update_priority(&999, 1)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);
	assert_eq!(r.get(&999), None);

	Ok(())
}

// update_value changes value while keeping priority
#[tokio::test]
async fn update_value() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver =
		timeout_s(2, w.extend(vec![(10, 1, 100), (20, 2, 200), (30, 3, 300)]))
			.await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// update value of key 2 from 200 to 999
	let ver = timeout_s(2, w.update_value(&2, 999)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	assert_eq!(r.get(&2), Some(999)); // value changed
	assert_eq!(r.get_priority(&2), Some(20)); // priority unchanged
	assert_eq!(r.len(), 3);

	// update_value on non-existent key is a no-op
	let ver = timeout_s(2, w.update_value(&999, 42)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);
	assert_eq!(r.get(&999), None);

	Ok(())
}

// remove_range with various bound types
#[tokio::test]
async fn remove_range() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// insert entries with priorities 10, 20, 30, 40, 50
	let ver = timeout_s(
		2,
		w.extend(vec![
			(10, 1, 100),
			(20, 2, 200),
			(30, 3, 300),
			(40, 4, 400),
			(50, 5, 500),
		]),
	)
	.await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 5);

	// remove_range(..25) — removes priorities below 25 (i.e. 10, 20)
	let ver = timeout_s(2, w.remove_range(..25u64)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);
	assert_eq!(r.get(&1), None); // priority 10 removed
	assert_eq!(r.get(&2), None); // priority 20 removed
	assert_eq!(r.get(&3), Some(300)); // priority 30 kept
	assert_eq!(r.min_priority(), Some(30));

	// remove_range(45..) — removes priorities >= 45 (i.e. 50)
	let ver = timeout_s(2, w.remove_range(45u64..)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 2);
	assert_eq!(r.get(&5), None); // priority 50 removed
	assert_eq!(r.max_priority(), Some(40));

	// re-populate to test inclusive range
	let ver = timeout_s(2, w.clear()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	let ver = timeout_s(
		2,
		w.extend(vec![
			(10, 1, 100),
			(20, 2, 200),
			(30, 3, 300),
			(40, 4, 400),
			(50, 5, 500),
		]),
	)
	.await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// remove_range(20..=40) — removes priorities 20, 30, 40
	let ver = timeout_s(2, w.remove_range(20u64..=40u64)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 2);
	assert_eq!(r.get(&1), Some(100)); // priority 10 kept
	assert_eq!(r.get(&5), Some(500)); // priority 50 kept
	assert_eq!(r.get(&2), None); // priority 20 removed
	assert_eq!(r.get(&3), None); // priority 30 removed
	assert_eq!(r.get(&4), None); // priority 40 removed

	// remove_range(..) — equivalent to clear
	let ver = timeout_s(2, w.remove_range(..)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());

	Ok(())
}

// remove non-existent key is a safe no-op
#[tokio::test]
async fn remove_nonexistent() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver =
		timeout_s(2, w.extend(vec![(10, 1, 100), (20, 2, 200), (30, 3, 300)]))
			.await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// remove a key that doesn't exist — no-op
	let ver = timeout_s(2, w.remove(&999)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);
	assert_eq!(r.get(&1), Some(100));
	assert_eq!(r.get(&2), Some(200));
	assert_eq!(r.get(&3), Some(300));

	Ok(())
}

// clear then rebuild
#[tokio::test]
async fn clear_then_rebuild() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver =
		timeout_s(2, w.extend(vec![(10, 1, 100), (20, 2, 200), (30, 3, 300)]))
			.await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);

	let ver = timeout_s(2, w.clear()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());

	// rebuild after clear — state should be fresh
	let ver = timeout_s(2, w.insert(99, 100, 1000)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);
	assert_eq!(r.get(&100), Some(1000));
	assert_eq!(r.get_priority(&100), Some(99));
	assert_eq!(r.get(&1), None); // old keys gone

	Ok(())
}

// extend with empty entries — no version bump needed, no crash
#[tokio::test]
async fn extend_empty() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.insert(10, 1, 100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// extending with empty should succeed and return a version
	let _ver_empty =
		timeout_s(2, w.extend(Vec::<(u64, u64, u64)>::new())).await??;
	// queue should still have just the one entry
	assert_eq!(r.len(), 1);

	// subsequent operations still work
	let ver = timeout_s(2, w.insert(20, 2, 200)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 2);

	Ok(())
}

// extend overwrites existing keys
#[tokio::test]
async fn extend_overwrite() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver =
		timeout_s(2, w.extend(vec![(10, 1, 100), (20, 2, 200), (30, 3, 300)]))
			.await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);

	// extend with overlapping keys — priority and value should be updated
	let ver =
		timeout_s(2, w.extend(vec![(99, 2, 999), (88, 3, 888), (40, 4, 400)]))
			.await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 4);
	assert_eq!(r.get(&1), Some(100)); // unchanged
	assert_eq!(r.get_priority(&1), Some(10)); // unchanged
	assert_eq!(r.get(&2), Some(999)); // updated
	assert_eq!(r.get_priority(&2), Some(99)); // updated
	assert_eq!(r.get(&3), Some(888)); // updated
	assert_eq!(r.get_priority(&3), Some(88)); // updated
	assert_eq!(r.get(&4), Some(400)); // new

	Ok(())
}

// entries with the same priority (bucket behavior)
#[tokio::test]
async fn same_priority_bucket() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// insert multiple entries with the same priority
	let ver = timeout_s(
		2,
		w.extend(vec![(10, 1, 100), (10, 2, 200), (10, 3, 300), (20, 4, 400)]),
	)
	.await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	assert_eq!(r.len(), 4);
	assert_eq!(r.min_priority(), Some(10));
	assert_eq!(r.max_priority(), Some(20));

	// get_min returns one of the entries with priority 10
	let (p, k, _) = r.get_min().unwrap();
	assert_eq!(p, 10);
	assert!([1, 2, 3].contains(&k));

	// remove one from the bucket
	let ver = timeout_s(2, w.remove(&2)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);
	assert_eq!(r.min_priority(), Some(10)); // bucket still has keys 1 and 3

	// remove all from the bucket
	let ver = timeout_s(2, w.remove(&1)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	let ver = timeout_s(2, w.remove(&3)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);
	assert_eq!(r.min_priority(), Some(20)); // only priority 20 remains

	Ok(())
}

// Many writers + many readers converge
#[tokio::test]
async fn many_writers_many_readers() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;
	let n3 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1, &n2, &n3])).await??;

	let w0 = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let w1 = collections::PriorityQueue::<u64, u64, u64>::writer(&n1, store_id);
	let r0 = collections::PriorityQueue::<u64, u64, u64>::reader(&n2, store_id);
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader(&n3, store_id);

	timeout_s(10, w0.when().online()).await?;
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, r0.when().online()).await?;
	timeout_s(10, r1.when().online()).await?;

	// writer 0 inserts some values
	let v =
		timeout_s(2, w0.extend(vec![(10, 1, 100), (20, 2, 200), (30, 3, 300)]))
			.await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	// writer 1 inserts more
	let v = timeout_s(2, w1.extend(vec![(40, 4, 400), (50, 5, 500)])).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	assert_eq!(r0.len(), 5);
	assert_eq!(r1.len(), 5);

	// writer 0 removes a key
	let v = timeout_s(2, w0.remove(&1)).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	// writer 1 inserts one more
	let v = timeout_s(2, w1.insert(60, 6, 600)).await??;
	timeout_s(2, w1.when().reaches(v)).await?;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	// all nodes agree
	let expected: Vec<(u64, u64, u64)> = w0.iter_asc().collect();
	assert_eq!(w1.iter_asc().collect::<Vec<_>>(), expected);
	assert_eq!(r0.iter_asc().collect::<Vec<_>>(), expected);
	assert_eq!(r1.iter_asc().collect::<Vec<_>>(), expected);

	let mut expected_keys: Vec<u64> =
		expected.iter().map(|(_, k, _)| *k).collect();
	expected_keys.sort_unstable();
	assert_eq!(expected_keys, vec![2, 3, 4, 5, 6]);

	Ok(())
}

// Late writer catches up with existing state then contributes
#[tokio::test]
async fn late_writer_catchup() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w0 = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	timeout_s(10, w0.when().online()).await?;

	// build up some state
	let ver = timeout_s(
		2,
		w0.extend(vec![
			(10, 1, 100),
			(20, 2, 200),
			(30, 3, 300),
			(40, 4, 400),
			(50, 5, 500),
		]),
	)
	.await??;
	timeout_s(2, w0.when().reaches(ver)).await?;

	// late-joining writer
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w1 = collections::PriorityQueue::<u64, u64, u64>::writer(&n1, store_id);
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, w1.when().reaches(ver)).await?;

	// late writer should see existing state
	assert_eq!(w1.len(), 5);

	// late writer can now insert
	let v1 = timeout_s(2, w1.insert(99, 99, 990)).await??;
	timeout_s(2, w0.when().reaches(v1)).await?;
	timeout_s(2, w1.when().reaches(v1)).await?;

	assert_eq!(w0.len(), 6);
	assert_eq!(w0.get(&99), Some(990));
	assert_eq!(w1.get(&99), Some(990));

	Ok(())
}

// Writer reads its own writes
#[tokio::test]
async fn writer_reads_own_writes() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w =
		collections::PriorityQueue::<u64, String, String>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	let ver =
		timeout_s(2, w.insert(10, "hello".into(), "world".into())).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	assert_eq!(w.len(), 1);
	assert_eq!(w.get(&"hello".into()), Some("world".into()));
	assert_eq!(w.get_priority(&"hello".into()), Some(10));
	assert!(w.contains_key(&"hello".into()));
	assert!(!w.contains_key(&"missing".into()));

	let ver = timeout_s(2, w.insert(20, "foo".into(), "bar".into())).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	let snap: Vec<(u64, String, String)> = w.iter_asc().collect();
	assert_eq!(snap.len(), 2);
	assert_eq!(snap[0], (10, "hello".into(), "world".into()));
	assert_eq!(snap[1], (20, "foo".into(), "bar".into()));

	Ok(())
}

// Remove all entries one by one
#[tokio::test]
async fn remove_all_entries() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver =
		timeout_s(2, w.extend(vec![(10, 1, 100), (20, 2, 200), (30, 3, 300)]))
			.await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);

	let ver = timeout_s(2, w.remove(&1)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 2);

	let ver = timeout_s(2, w.remove(&2)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);

	let ver = timeout_s(2, w.remove(&3)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());
	assert_eq!(r.get_min(), None);
	assert_eq!(r.get_max(), None);

	// can re-insert after removing all
	let ver = timeout_s(2, w.insert(42, 4, 400)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);
	assert_eq!(r.get(&4), Some(400));
	assert_eq!(r.get_priority(&4), Some(42));

	Ok(())
}

// remove_range on empty queue is a safe no-op
#[tokio::test]
async fn remove_range_empty() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	// remove_range on empty queue should not crash
	let ver = timeout_s(2, w.remove_range(..100u64)).await??;
	timeout_s(2, w.when().reaches(ver)).await?;
	assert!(w.is_empty());

	// insert still works after
	let ver = timeout_s(2, w.insert(10, 1, 100)).await??;
	timeout_s(2, w.when().reaches(ver)).await?;
	assert_eq!(w.len(), 1);

	Ok(())
}

// iter ordering is correct after mixed operations
#[tokio::test]
async fn iter_ordering_after_mutations() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	// Insert in non-sorted order
	let ver = timeout_s(
		2,
		w.extend(vec![
			(50, 1, 100),
			(10, 2, 200),
			(30, 3, 300),
			(20, 4, 400),
			(40, 5, 500),
		]),
	)
	.await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	// ascending iteration should yield priorities in order
	let asc: Vec<u64> = w.iter_asc().map(|(p, _, _)| p).collect();
	assert_eq!(asc, vec![10, 20, 30, 40, 50]);

	// descending iteration should yield priorities in reverse order
	let desc: Vec<u64> = w.iter_desc().map(|(p, _, _)| p).collect();
	assert_eq!(desc, vec![50, 40, 30, 20, 10]);

	// remove the middle element and check again
	let ver = timeout_s(2, w.remove(&3)).await??; // priority 30
	timeout_s(2, w.when().reaches(ver)).await?;

	let asc: Vec<u64> = w.iter_asc().map(|(p, _, _)| p).collect();
	assert_eq!(asc, vec![10, 20, 40, 50]);

	let desc: Vec<u64> = w.iter_desc().map(|(p, _, _)| p).collect();
	assert_eq!(desc, vec![50, 40, 20, 10]);

	// update_priority to change ordering
	let ver = timeout_s(2, w.update_priority(&2, 45)).await??; // 10 → 45
	timeout_s(2, w.when().reaches(ver)).await?;

	let asc: Vec<u64> = w.iter_asc().map(|(p, _, _)| p).collect();
	assert_eq!(asc, vec![20, 40, 45, 50]);

	Ok(())
}

// compare_exchange_value succeeds when current value matches
#[tokio::test]
async fn compare_exchange_value_success() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// insert initial entry
	let ver = timeout_s(2, w.insert(10, 1, 100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(100));
	assert_eq!(r.get_priority(&1), Some(10));

	// compare_exchange_value: expected matches, new value is written
	let ver =
		timeout_s(2, w.compare_exchange_value(&1, 100, Some(200))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(200));
	// priority should remain unchanged
	assert_eq!(r.get_priority(&1), Some(10));
	assert_eq!(r.len(), 1);

	Ok(())
}

// compare_exchange_value removes entry when new is None
#[tokio::test]
async fn compare_exchange_value_remove() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// insert two entries
	let ver = timeout_s(2, w.extend(vec![(10, 1, 100), (20, 2, 200)])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 2);

	// compare_exchange_value: Some(100) -> None removes key 1
	let ver = timeout_s(2, w.compare_exchange_value(&1, 100, None)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), None);
	assert!(!r.contains_key(&1));
	assert_eq!(r.len(), 1);
	// other entry untouched
	assert_eq!(r.get(&2), Some(200));

	Ok(())
}

// compare_exchange_value does not change value when expected doesn't match
#[tokio::test]
async fn compare_exchange_value_mismatch() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// insert initial entry
	let ver = timeout_s(2, w.insert(10, 1, 100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// compare_exchange_value with wrong expected value — no change
	let ver =
		timeout_s(2, w.compare_exchange_value(&1, 999, Some(200))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(100)); // unchanged
	assert_eq!(r.get_priority(&1), Some(10)); // priority unchanged

	// mismatch removal attempt — no change
	let ver = timeout_s(2, w.compare_exchange_value(&1, 999, None)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(100)); // still unchanged
	assert_eq!(r.len(), 1);

	Ok(())
}

// compare_exchange_value on non-existent key is a no-op
#[tokio::test]
async fn compare_exchange_value_missing_key() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	assert!(r.is_empty());

	// compare_exchange_value on non-existent key — no-op
	let ver =
		timeout_s(2, w.compare_exchange_value(&1, 100, Some(200))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());

	// insert an entry, then try compare_exchange_value on a different key
	let ver = timeout_s(2, w.insert(10, 1, 100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	let ver =
		timeout_s(2, w.compare_exchange_value(&99, 100, Some(200))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);
	assert_eq!(r.get(&99), None);

	Ok(())
}

// compare_exchange_value replicates to multiple readers
#[tokio::test]
async fn compare_exchange_value_replicates_to_readers() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n_w = Network::new(network_id).await?;
	let n_r1 = Network::new(network_id).await?;
	let n_r2 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n_w, &n_r1, &n_r2])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n_w, store_id);
	let r1 = collections::PriorityQueue::<u64, u64, u64>::reader(&n_r1, store_id);
	let r2 = collections::PriorityQueue::<u64, u64, u64>::reader(&n_r2, store_id);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r1.when().online()).await?;
	timeout_s(10, r2.when().online()).await?;

	// insert initial entries
	let ver = timeout_s(2, w.extend(vec![(10, 1, 100), (20, 2, 200)])).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;

	// compare_exchange_value replicates to all readers
	let ver =
		timeout_s(2, w.compare_exchange_value(&1, 100, Some(150))).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;

	assert_eq!(r1.get(&1), Some(150));
	assert_eq!(r2.get(&1), Some(150));
	// priority preserved
	assert_eq!(r1.get_priority(&1), Some(10));
	assert_eq!(r2.get_priority(&1), Some(10));

	Ok(())
}

// compare_exchange_value chained: multiple sequential CAS operations
#[tokio::test]
async fn compare_exchange_value_chained() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// insert initial entry
	let ver = timeout_s(2, w.insert(10, 1, 100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// chain: 100 -> 200 -> 300 -> remove
	let ver =
		timeout_s(2, w.compare_exchange_value(&1, 100, Some(200))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(200));

	let ver =
		timeout_s(2, w.compare_exchange_value(&1, 200, Some(300))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(300));

	let ver = timeout_s(2, w.compare_exchange_value(&1, 300, None)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), None);
	assert!(r.is_empty());

	Ok(())
}

// compare_exchange_value preserves priority and doesn't affect other entries
#[tokio::test]
async fn compare_exchange_value_preserves_priority() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// insert entries with different priorities
	let ver =
		timeout_s(2, w.extend(vec![(5, 1, 100), (10, 2, 200), (20, 3, 300)]))
			.await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// compare_exchange_value on the middle entry
	let ver =
		timeout_s(2, w.compare_exchange_value(&2, 200, Some(250))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// value changed, priority preserved
	assert_eq!(r.get(&2), Some(250));
	assert_eq!(r.get_priority(&2), Some(10));

	// min/max unaffected (priorities unchanged)
	assert_eq!(r.min_priority(), Some(5));
	assert_eq!(r.max_priority(), Some(20));

	// other entries untouched
	assert_eq!(r.get(&1), Some(100));
	assert_eq!(r.get(&3), Some(300));

	// ordering preserved
	let asc: Vec<u64> = r.iter_asc().map(|(p, _, _)| p).collect();
	assert_eq!(asc, vec![5, 10, 20]);

	Ok(())
}

// compare_exchange_value interleaved with other mutations
#[tokio::test]
async fn compare_exchange_value_mixed_with_mutations() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// regular insert
	let ver = timeout_s(2, w.insert(10, 1, 100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// compare_exchange_value after insert
	let ver =
		timeout_s(2, w.compare_exchange_value(&1, 100, Some(200))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(200));

	// update_value, then compare_exchange_value
	let ver = timeout_s(2, w.update_value(&1, 300)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(300));

	let ver =
		timeout_s(2, w.compare_exchange_value(&1, 300, Some(400))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(400));

	// update_priority, then compare_exchange_value (priority changes, value still
	// matches)
	let ver = timeout_s(2, w.update_priority(&1, 50)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get_priority(&1), Some(50));

	let ver =
		timeout_s(2, w.compare_exchange_value(&1, 400, Some(500))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(500));
	assert_eq!(r.get_priority(&1), Some(50)); // priority still 50

	// remove via compare_exchange_value, then insert fresh
	let ver = timeout_s(2, w.compare_exchange_value(&1, 500, None)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());

	let ver = timeout_s(2, w.insert(1, 1, 10)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(10));
	assert_eq!(r.len(), 1);

	Ok(())
}

// compare_exchange_value with late-joining reader catches up correctly
#[tokio::test]
async fn compare_exchange_value_catchup() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	// build state with compare_exchange_value
	let ver = timeout_s(2, w.insert(10, 1, 100)).await??;
	timeout_s(2, w.when().reaches(ver)).await?;
	let ver =
		timeout_s(2, w.compare_exchange_value(&1, 100, Some(200))).await??;
	timeout_s(2, w.when().reaches(ver)).await?;
	let ver = timeout_s(2, w.insert(20, 2, 300)).await??;
	timeout_s(2, w.when().reaches(ver)).await?;
	let ver =
		timeout_s(2, w.compare_exchange_value(&2, 300, Some(400))).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	// late-joining reader
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, r.when().online()).await?;
	timeout_s(10, r.when().reaches(ver)).await?;

	// reader sees latest state
	assert_eq!(r.get(&1), Some(200));
	assert_eq!(r.get_priority(&1), Some(10));
	assert_eq!(r.get(&2), Some(400));
	assert_eq!(r.get_priority(&2), Some(20));
	assert_eq!(r.len(), 2);

	Ok(())
}

// compare_exchange_value removal affects min/max correctly
#[tokio::test]
async fn compare_exchange_value_remove_affects_minmax() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = collections::PriorityQueue::<u64, u64, u64>::writer(&n0, store_id);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// insert entries: key 1 has min priority, key 3 has max priority
	let ver =
		timeout_s(2, w.extend(vec![(5, 1, 100), (10, 2, 200), (20, 3, 300)]))
			.await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	assert_eq!(r.min_priority(), Some(5));
	assert_eq!(r.max_priority(), Some(20));

	// remove min entry via compare_exchange_value
	let ver = timeout_s(2, w.compare_exchange_value(&1, 100, None)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	assert_eq!(r.min_priority(), Some(10)); // new min
	assert_eq!(r.max_priority(), Some(20)); // unchanged
	assert_eq!(r.len(), 2);

	// remove max entry via compare_exchange_value
	let ver = timeout_s(2, w.compare_exchange_value(&3, 300, None)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	assert_eq!(r.min_priority(), Some(10)); // only entry left
	assert_eq!(r.max_priority(), Some(10));
	assert_eq!(r.len(), 1);

	Ok(())
}
