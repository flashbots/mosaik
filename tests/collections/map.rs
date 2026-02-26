use {
	crate::utils::{discover_all, timeout_s},
	mosaik::{collections::StoreId, *},
	std::collections::HashMap,
};

// Basic smoke: writer + reader, all read/write operations, no late join
#[tokio::test]
async fn smoke_no_catchup() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Map::<u64, u64>::new(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// -- empty state --
	assert!(r.is_empty());
	assert_eq!(r.len(), 0);
	assert_eq!(r.get(&1), None);
	assert!(!r.contains_key(&1));
	assert_eq!(r.iter().count(), 0);
	assert_eq!(r.keys().count(), 0);
	assert_eq!(r.values().count(), 0);

	// -- insert --
	let ver = timeout_s(2, w.insert(1, 10)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);
	assert!(!r.is_empty());
	assert_eq!(r.get(&1), Some(10));
	assert!(r.contains_key(&1));
	assert!(!r.contains_key(&2));

	// -- insert another key --
	let ver = timeout_s(2, w.insert(2, 20)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 2);
	assert_eq!(r.get(&2), Some(20));

	// -- update existing key --
	let ver = timeout_s(2, w.insert(1, 100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 2); // length unchanged
	assert_eq!(r.get(&1), Some(100)); // value updated

	// -- extend --
	let ver = timeout_s(2, w.extend(vec![(3, 30), (4, 40), (5, 50)])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 5);
	assert_eq!(r.get(&3), Some(30));
	assert_eq!(r.get(&4), Some(40));
	assert_eq!(r.get(&5), Some(50));

	// -- contains_key --
	assert!(r.contains_key(&3));
	assert!(!r.contains_key(&99));

	// -- iter, keys, values --
	let snapshot: HashMap<u64, u64> = r.iter().collect();
	assert_eq!(snapshot.len(), 5);
	assert_eq!(snapshot[&1], 100);
	assert_eq!(snapshot[&2], 20);
	assert_eq!(snapshot[&3], 30);
	assert_eq!(snapshot[&4], 40);
	assert_eq!(snapshot[&5], 50);

	let keys: std::collections::HashSet<u64> = r.keys().collect();
	assert_eq!(keys.len(), 5);
	assert!(keys.contains(&1));
	assert!(keys.contains(&5));

	let values: std::collections::HashSet<u64> = r.values().collect();
	assert!(values.contains(&100));
	assert!(values.contains(&50));

	// -- remove --
	let ver = timeout_s(2, w.remove(&3)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 4);
	assert_eq!(r.get(&3), None);
	assert!(!r.contains_key(&3));

	// -- clear --
	let ver = timeout_s(2, w.clear()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());
	assert_eq!(r.len(), 0);

	Ok(())
}

// Late-joining reader catches up with full log replay
#[tokio::test]
async fn catchup_single_reader() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	// build a non-trivial history: insert 100 entries
	for i in 0u64..100 {
		timeout_s(2, w.insert(i, i * 10)).await??;
	}

	let ver =
		timeout_s(2, w.extend([(100, 1000), (101, 1010), (102, 1020)])).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	// late-joining reader
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, r.when().online()).await?;
	timeout_s(10, r.when().reaches(ver)).await?;

	assert_eq!(r.len(), 103);
	for i in 0u64..100 {
		assert_eq!(r.get(&i), Some(i * 10));
	}
	assert_eq!(r.get(&100), Some(1000));
	assert_eq!(r.get(&101), Some(1010));
	assert_eq!(r.get(&102), Some(1020));

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

	let w = mosaik::collections::Map::<u64, u64>::writer(&n_w, store_id);
	let r1 = mosaik::collections::Map::<u64, u64>::reader(&n_r1, store_id);
	let r2 = mosaik::collections::Map::<u64, u64>::reader(&n_r2, store_id);
	let r3 = mosaik::collections::Map::<u64, u64>::reader(&n_r3, store_id);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r1.when().online()).await?;
	timeout_s(10, r2.when().online()).await?;
	timeout_s(10, r3.when().online()).await?;

	// insert a batch
	let ver = timeout_s(
		2,
		w.extend(vec![(1, 10), (2, 20), (3, 30), (4, 40), (5, 50)]),
	)
	.await??;

	// all readers converge
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert_eq!(r.len(), 5);
		let snap: HashMap<u64, u64> = r.iter().collect();
		assert_eq!(snap[&1], 10);
		assert_eq!(snap[&5], 50);
	}

	// more mutations replicate to all
	let ver = timeout_s(2, w.insert(6, 60)).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert_eq!(r.get(&6), Some(60));
		assert_eq!(r.len(), 6);
	}

	let ver = timeout_s(2, w.remove(&1)).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert_eq!(r.len(), 5);
		assert_eq!(r.get(&1), None);
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

	let w0 = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let w1 = mosaik::collections::Map::<u64, u64>::writer(&n1, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n2, store_id);

	timeout_s(10, w0.when().online()).await?;
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// both writers insert values (sequentially to avoid races in the test itself)
	let v0 = timeout_s(2, w0.insert(1, 100)).await??;
	timeout_s(2, r.when().reaches(v0)).await?;

	let v1 = timeout_s(2, w1.insert(2, 200)).await??;
	timeout_s(2, r.when().reaches(v1)).await?;

	let v2 = timeout_s(2, w0.insert(3, 300)).await??;
	timeout_s(2, r.when().reaches(v2)).await?;

	assert_eq!(r.len(), 3);
	assert_eq!(r.get(&1), Some(100));
	assert_eq!(r.get(&2), Some(200));
	assert_eq!(r.get(&3), Some(300));

	// a writer that extends
	let v3 = timeout_s(2, w1.extend(vec![(4, 400), (5, 500)])).await??;
	timeout_s(2, r.when().reaches(v3)).await?;

	assert_eq!(r.len(), 5);
	assert_eq!(r.get(&4), Some(400));
	assert_eq!(r.get(&5), Some(500));

	timeout_s(2, w0.when().reaches(v3)).await?;
	timeout_s(2, w1.when().reaches(v3)).await?;

	// all three nodes agree on the snapshot
	let snap_w0: HashMap<u64, u64> = w0.iter().collect();
	let snap_w1: HashMap<u64, u64> = w1.iter().collect();
	let snap_r: HashMap<u64, u64> = r.iter().collect();
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
	let w = mosaik::collections::Map::<u64, u64>::writer(&n_w, store_id);
	timeout_s(10, w.when().online()).await?;

	// build history
	for i in 0u64..50 {
		timeout_s(2, w.insert(i, i * 10)).await??;
	}
	let mid_ver = timeout_s(2, w.insert(999, 9990)).await??;
	timeout_s(2, w.when().reaches(mid_ver)).await?;

	// first late reader
	let n_r1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n_w, &n_r1])).await??;
	let r1 = mosaik::collections::Map::<u64, u64>::reader(&n_r1, store_id);
	timeout_s(10, r1.when().online()).await?;
	timeout_s(10, r1.when().reaches(mid_ver)).await?;
	assert_eq!(r1.len(), 51);
	assert_eq!(r1.get(&999), Some(9990));

	// add more entries while first reader is alive
	for i in 50u64..80 {
		timeout_s(2, w.insert(i, i * 10)).await??;
	}
	let late_ver = timeout_s(2, w.insert(1000, 10000)).await??;
	timeout_s(2, w.when().reaches(late_ver)).await?;

	// second late reader joins even later
	let n_r2 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n_w, &n_r1, &n_r2])).await??;
	let r2 = mosaik::collections::Map::<u64, u64>::reader(&n_r2, store_id);
	timeout_s(10, r2.when().online()).await?;
	timeout_s(10, r2.when().reaches(late_ver)).await?;

	// first reader should also converge to latest
	timeout_s(10, r1.when().reaches(late_ver)).await?;

	assert_eq!(r1.len(), 82);
	assert_eq!(r2.len(), 82);
	assert_eq!(r1.get(&1000), Some(10000));
	assert_eq!(r2.get(&1000), Some(10000));

	// snapshots match
	let snap1: HashMap<u64, u64> = r1.iter().collect();
	let snap2: HashMap<u64, u64> = r2.iter().collect();
	assert_eq!(snap1, snap2);

	Ok(())
}

// insert overwrites existing value for the same key
#[tokio::test]
async fn insert_overwrite() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.insert(1, 10)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(10));

	// overwrite with new value
	let ver = timeout_s(2, w.insert(1, 99)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(99));
	assert_eq!(r.len(), 1); // still only one entry

	// overwrite multiple times
	let ver = timeout_s(2, w.insert(1, 200)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(200));
	assert_eq!(r.len(), 1);

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

	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.extend(vec![(1, 10), (2, 20), (3, 30)])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);

	// extend with overlapping keys — values should be updated
	let ver =
		timeout_s(2, w.extend(vec![(2, 200), (3, 300), (4, 400)])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 4);
	assert_eq!(r.get(&1), Some(10)); // unchanged
	assert_eq!(r.get(&2), Some(200)); // updated
	assert_eq!(r.get(&3), Some(300)); // updated
	assert_eq!(r.get(&4), Some(400)); // new

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

	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.extend(vec![(1, 10), (2, 20), (3, 30)])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// remove a key that doesn't exist — no-op
	let ver = timeout_s(2, w.remove(&999)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);
	assert_eq!(r.get(&1), Some(10));
	assert_eq!(r.get(&2), Some(20));
	assert_eq!(r.get(&3), Some(30));

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

	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.extend(vec![(1, 10), (2, 20), (3, 30)])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);

	let ver = timeout_s(2, w.clear()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());

	// rebuild after clear — state should be fresh
	let ver = timeout_s(2, w.insert(100, 1000)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);
	assert_eq!(r.get(&100), Some(1000));
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

	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.insert(1, 10)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// extending with empty should succeed and return a version
	let _ver_empty = timeout_s(2, w.extend(Vec::<(u64, u64)>::new())).await??;
	// map should still have just the one entry
	assert_eq!(r.len(), 1);

	// subsequent operations still work
	let ver = timeout_s(2, w.insert(2, 20)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 2);

	Ok(())
}

// Multiple writers + multiple readers converge
#[tokio::test]
async fn many_writers_many_readers() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;
	let n3 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1, &n2, &n3])).await??;

	let w0 = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let w1 = mosaik::collections::Map::<u64, u64>::writer(&n1, store_id);
	let r0 = mosaik::collections::Map::<u64, u64>::reader(&n2, store_id);
	let r1 = mosaik::collections::Map::<u64, u64>::reader(&n3, store_id);

	timeout_s(10, w0.when().online()).await?;
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, r0.when().online()).await?;
	timeout_s(10, r1.when().online()).await?;

	// writer 0 inserts some values
	let v = timeout_s(2, w0.extend(vec![(1, 10), (2, 20), (3, 30)])).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	// writer 1 inserts more
	let v = timeout_s(2, w1.extend(vec![(4, 40), (5, 50)])).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	assert_eq!(r0.len(), 5);
	assert_eq!(r1.len(), 5);

	// writer 0 removes a key
	let v = timeout_s(2, w0.remove(&1)).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	// writer 1 inserts one more
	let v = timeout_s(2, w1.insert(6, 60)).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	// all nodes agree
	let expected: HashMap<u64, u64> = w0.iter().collect();
	assert_eq!(w1.iter().collect::<HashMap<_, _>>(), expected);
	assert_eq!(r0.iter().collect::<HashMap<_, _>>(), expected);
	assert_eq!(r1.iter().collect::<HashMap<_, _>>(), expected);

	let mut expected_keys: Vec<u64> = expected.keys().copied().collect();
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
	let w0 = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	timeout_s(10, w0.when().online()).await?;

	// build up some state
	let ver = timeout_s(
		2,
		w0.extend(vec![(1, 10), (2, 20), (3, 30), (4, 40), (5, 50)]),
	)
	.await??;
	timeout_s(2, w0.when().reaches(ver)).await?;

	// late-joining writer
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w1 = mosaik::collections::Map::<u64, u64>::writer(&n1, store_id);
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, w1.when().reaches(ver)).await?;

	// late writer should see existing state
	assert_eq!(w1.len(), 5);

	// late writer can now insert
	let v1 = timeout_s(2, w1.insert(99, 990)).await??;
	timeout_s(2, w0.when().reaches(v1)).await?;

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
	let w = mosaik::collections::Map::<String, String>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	let ver = timeout_s(2, w.insert("hello".into(), "world".into())).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	assert_eq!(w.len(), 1);
	assert_eq!(w.get("hello"), Some("world".into()));
	assert!(w.contains_key("hello"));
	assert!(!w.contains_key("missing"));

	let ver = timeout_s(2, w.insert("foo".into(), "bar".into())).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	let snap: HashMap<String, String> = w.iter().collect();
	assert_eq!(snap.len(), 2);
	assert_eq!(snap["hello"], "world");
	assert_eq!(snap["foo"], "bar");

	let keys: std::collections::HashSet<String> = w.keys().collect();
	assert!(keys.contains("hello"));
	assert!(keys.contains("foo"));

	let values: std::collections::HashSet<String> = w.values().collect();
	assert!(values.contains("world"));
	assert!(values.contains("bar"));

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

	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.extend(vec![(1, 10), (2, 20), (3, 30)])).await??;
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

	// can re-insert after removing all
	let ver = timeout_s(2, w.insert(4, 40)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);
	assert_eq!(r.get(&4), Some(40));

	Ok(())
}

// compare_exchange succeeds when current value matches
#[tokio::test]
async fn compare_exchange_success() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// insert initial value
	let ver = timeout_s(2, w.insert(1, 10)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(10));

	// compare_exchange: current matches, so new value is written
	let ver = timeout_s(2, w.compare_exchange(1, Some(10), Some(99))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(99));
	assert_eq!(r.len(), 1); // still one entry

	Ok(())
}

// compare_exchange inserts into empty map: None -> Some
#[tokio::test]
async fn compare_exchange_insert_new_key() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// map starts empty
	assert!(r.is_empty());

	// compare_exchange: None -> Some(10) inserts a new key
	let ver = timeout_s(2, w.compare_exchange(1, None, Some(10))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(10));
	assert_eq!(r.len(), 1);

	Ok(())
}

// compare_exchange removes a key: Some -> None
#[tokio::test]
async fn compare_exchange_remove_key() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// insert initial values
	let ver = timeout_s(2, w.extend(vec![(1, 10), (2, 20)])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 2);

	// compare_exchange: Some(10) -> None removes key 1
	let ver = timeout_s(2, w.compare_exchange(1, Some(10), None)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), None);
	assert!(!r.contains_key(&1));
	assert_eq!(r.len(), 1);
	assert_eq!(r.get(&2), Some(20)); // other key untouched

	Ok(())
}

// compare_exchange does not change value when expected doesn't match
#[tokio::test]
async fn compare_exchange_mismatch_no_change() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// insert initial value
	let ver = timeout_s(2, w.insert(1, 10)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// compare_exchange with wrong expected value — should NOT change
	let ver = timeout_s(2, w.compare_exchange(1, Some(999), Some(200))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(10)); // unchanged

	// compare_exchange with None expected on existing key — should NOT change
	let ver = timeout_s(2, w.compare_exchange(1, None, Some(300))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(10)); // still unchanged

	Ok(())
}

// compare_exchange mismatch on missing key (expected Some on absent key)
#[tokio::test]
async fn compare_exchange_mismatch_on_missing_key() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	assert!(r.is_empty());

	// compare_exchange expecting Some on non-existent key — no insertion
	let ver = timeout_s(2, w.compare_exchange(1, Some(42), Some(99))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());
	assert_eq!(r.get(&1), None);

	Ok(())
}

// compare_exchange replicates to multiple readers
#[tokio::test]
async fn compare_exchange_replicates_to_readers() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n_w = Network::new(network_id).await?;
	let n_r1 = Network::new(network_id).await?;
	let n_r2 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n_w, &n_r1, &n_r2])).await??;

	let w = mosaik::collections::Map::<u64, u64>::writer(&n_w, store_id);
	let r1 = mosaik::collections::Map::<u64, u64>::reader(&n_r1, store_id);
	let r2 = mosaik::collections::Map::<u64, u64>::reader(&n_r2, store_id);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r1.when().online()).await?;
	timeout_s(10, r2.when().online()).await?;

	// insert initial values
	let ver = timeout_s(2, w.extend(vec![(1, 10), (2, 20)])).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;

	// compare_exchange replicates to all readers
	let ver = timeout_s(2, w.compare_exchange(1, Some(10), Some(100))).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;

	assert_eq!(r1.get(&1), Some(100));
	assert_eq!(r2.get(&1), Some(100));
	// other key untouched
	assert_eq!(r1.get(&2), Some(20));
	assert_eq!(r2.get(&2), Some(20));

	Ok(())
}

// compare_exchange chained: multiple sequential CAS operations on the same key
#[tokio::test]
async fn compare_exchange_chained() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// None -> Some(1) -> Some(2) -> Some(3) -> None chain on key 1
	let ver = timeout_s(2, w.compare_exchange(1, None, Some(1))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(1));

	let ver = timeout_s(2, w.compare_exchange(1, Some(1), Some(2))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(2));

	let ver = timeout_s(2, w.compare_exchange(1, Some(2), Some(3))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(3));

	let ver = timeout_s(2, w.compare_exchange(1, Some(3), None)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), None);
	assert!(r.is_empty());

	Ok(())
}

// compare_exchange interleaved with regular insert, remove, and clear
#[tokio::test]
async fn compare_exchange_mixed_with_insert_remove() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// regular insert
	let ver = timeout_s(2, w.insert(1, 10)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(10));

	// compare_exchange after insert
	let ver = timeout_s(2, w.compare_exchange(1, Some(10), Some(20))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(20));

	// remove, then compare_exchange from None to re-insert
	let ver = timeout_s(2, w.remove(&1)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), None);

	let ver = timeout_s(2, w.compare_exchange(1, None, Some(30))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(30));

	// clear, then compare_exchange on a fresh map
	let ver = timeout_s(2, w.clear()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());

	let ver = timeout_s(2, w.compare_exchange(1, None, Some(40))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(40));
	assert_eq!(r.len(), 1);

	// insert after compare_exchange
	let ver = timeout_s(2, w.insert(2, 50)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 2);

	Ok(())
}

// compare_exchange on multiple different keys
#[tokio::test]
async fn compare_exchange_multiple_keys() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// insert several keys
	let ver = timeout_s(2, w.extend(vec![(1, 10), (2, 20), (3, 30)])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// compare_exchange on different keys independently
	let ver = timeout_s(2, w.compare_exchange(1, Some(10), Some(100))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	let ver = timeout_s(2, w.compare_exchange(2, Some(20), None)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	let ver = timeout_s(2, w.compare_exchange(4, None, Some(40))).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	assert_eq!(r.get(&1), Some(100)); // updated
	assert_eq!(r.get(&2), None); // removed
	assert_eq!(r.get(&3), Some(30)); // untouched
	assert_eq!(r.get(&4), Some(40)); // inserted
	assert_eq!(r.len(), 3);

	Ok(())
}

// compare_exchange None -> None on missing key is a no-op
#[tokio::test]
async fn compare_exchange_none_to_none() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// compare_exchange(key, None, None) on empty map — no-op
	let ver = timeout_s(2, w.compare_exchange(1, None, None)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());

	// subsequent operations still work
	let ver = timeout_s(2, w.insert(1, 10)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(10));

	Ok(())
}

// compare_exchange with late-joining reader catches up correctly
#[tokio::test]
async fn compare_exchange_catchup() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w = mosaik::collections::Map::<u64, u64>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	// build state with compare_exchange
	let ver = timeout_s(2, w.compare_exchange(1, None, Some(10))).await??;
	timeout_s(2, w.when().reaches(ver)).await?;
	let ver = timeout_s(2, w.compare_exchange(2, None, Some(20))).await??;
	timeout_s(2, w.when().reaches(ver)).await?;
	let ver = timeout_s(2, w.compare_exchange(1, Some(10), Some(100))).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	// late-joining reader
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let r = mosaik::collections::Map::<u64, u64>::reader(&n1, store_id);
	timeout_s(10, r.when().online()).await?;
	timeout_s(10, r.when().reaches(ver)).await?;

	// reader sees latest state
	assert_eq!(r.get(&1), Some(100));
	assert_eq!(r.get(&2), Some(20));
	assert_eq!(r.len(), 2);

	Ok(())
}

// compare_exchange with string keys and values
#[tokio::test]
async fn compare_exchange_string_keys() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Map::<String, String>::writer(&n0, store_id);
	let r = mosaik::collections::Map::<String, String>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// insert via compare_exchange
	let ver = timeout_s(
		2,
		w.compare_exchange("key".into(), None, Some("value".into())),
	)
	.await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get("key"), Some("value".into()));

	// update via compare_exchange
	let ver = timeout_s(
		2,
		w.compare_exchange(
			"key".into(),
			Some("value".into()),
			Some("updated".into()),
		),
	)
	.await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get("key"), Some("updated".into()));

	// delete via compare_exchange
	let ver = timeout_s(
		2,
		w.compare_exchange("key".into(), Some("updated".into()), None),
	)
	.await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get("key"), None);
	assert!(r.is_empty());

	Ok(())
}
