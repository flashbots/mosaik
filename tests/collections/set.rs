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

	let w = mosaik::collections::Set::<u64>::new(&n0, store_id);
	let r = mosaik::collections::Set::<u64>::reader(&n1, store_id);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// -- empty state --
	assert!(r.is_empty());
	assert_eq!(r.len(), 0);
	assert!(!r.contains(&1));
	assert_eq!(r.iter().count(), 0);

	// -- insert --
	let ver = timeout_s(2, w.insert(10)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);
	assert!(!r.is_empty());
	assert!(r.contains(&10));
	assert!(!r.contains(&20));

	// -- insert another value --
	let ver = timeout_s(2, w.insert(20)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 2);
	assert!(r.contains(&20));

	// -- insert duplicate (no-op for length) --
	let ver = timeout_s(2, w.insert(10)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 2); // still 2

	// -- extend --
	let ver = timeout_s(2, w.extend(vec![30, 40, 50])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 5);
	assert!(r.contains(&30));
	assert!(r.contains(&40));
	assert!(r.contains(&50));

	// -- contains --
	assert!(r.contains(&10));
	assert!(!r.contains(&99));

	// -- iter --
	let snapshot: std::collections::HashSet<u64> = r.iter().collect();
	assert_eq!(snapshot.len(), 5);
	assert!(snapshot.contains(&10));
	assert!(snapshot.contains(&20));
	assert!(snapshot.contains(&30));
	assert!(snapshot.contains(&40));
	assert!(snapshot.contains(&50));

	// -- remove --
	let ver = timeout_s(2, w.remove(&30)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 4);
	assert!(!r.contains(&30));

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
	let w = mosaik::collections::Set::<u64>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	// build a non-trivial history: insert 100 elements
	for i in 0u64..100 {
		timeout_s(2, w.insert(i)).await??;
	}

	let ver = timeout_s(2, w.extend([100, 101, 102])).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	// late-joining reader
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let r = mosaik::collections::Set::<u64>::reader(&n1, store_id);
	timeout_s(10, r.when().online()).await?;
	timeout_s(10, r.when().reaches(ver)).await?;

	assert_eq!(r.len(), 103);
	for i in 0u64..103 {
		assert!(r.contains(&i));
	}

	// writer can continue operating after reader caught up
	let ver = timeout_s(2, w.remove(&50)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 102);
	assert!(!r.contains(&50));

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

	let w = mosaik::collections::Set::<u64>::writer(&n_w, store_id);
	let r1 = mosaik::collections::Set::<u64>::reader(&n_r1, store_id);
	let r2 = mosaik::collections::Set::<u64>::reader(&n_r2, store_id);
	let r3 = mosaik::collections::Set::<u64>::reader(&n_r3, store_id);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r1.when().online()).await?;
	timeout_s(10, r2.when().online()).await?;
	timeout_s(10, r3.when().online()).await?;

	// insert a batch
	let ver = timeout_s(2, w.extend(vec![1, 2, 3, 4, 5])).await??;

	// all readers converge
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert_eq!(r.len(), 5);
		assert!(r.contains(&1));
		assert!(r.contains(&5));
	}

	// more mutations replicate to all
	let ver = timeout_s(2, w.insert(6)).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert!(r.contains(&6));
		assert_eq!(r.len(), 6);
	}

	let ver = timeout_s(2, w.remove(&1)).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert_eq!(r.len(), 5);
		assert!(!r.contains(&1));
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

	let w0 = mosaik::collections::Set::<u64>::writer(&n0, store_id);
	let w1 = mosaik::collections::Set::<u64>::writer(&n1, store_id);
	let r = mosaik::collections::Set::<u64>::reader(&n2, store_id);

	timeout_s(10, w0.when().online()).await?;
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// both writers insert values (sequentially to avoid races in the test itself)
	let v0 = timeout_s(2, w0.insert(1)).await??;
	timeout_s(2, r.when().reaches(v0)).await?;

	let v1 = timeout_s(2, w1.insert(2)).await??;
	timeout_s(2, r.when().reaches(v1)).await?;

	let v2 = timeout_s(2, w0.insert(3)).await??;
	timeout_s(2, r.when().reaches(v2)).await?;

	assert_eq!(r.len(), 3);
	assert!(r.contains(&1));
	assert!(r.contains(&2));
	assert!(r.contains(&3));

	// a writer that extends
	let v3 = timeout_s(2, w1.extend(vec![4, 5])).await??;
	timeout_s(2, r.when().reaches(v3)).await?;

	assert_eq!(r.len(), 5);
	assert!(r.contains(&4));
	assert!(r.contains(&5));

	timeout_s(2, w0.when().reaches(v3)).await?;
	timeout_s(2, w1.when().reaches(v3)).await?;

	// all three nodes agree on the snapshot
	let snap_w0: std::collections::HashSet<u64> = w0.iter().collect();
	let snap_w1: std::collections::HashSet<u64> = w1.iter().collect();
	let snap_r: std::collections::HashSet<u64> = r.iter().collect();
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
	let w = mosaik::collections::Set::<u64>::writer(&n_w, store_id);
	timeout_s(10, w.when().online()).await?;

	// build history
	for i in 0u64..50 {
		timeout_s(2, w.insert(i)).await??;
	}
	let mid_ver = timeout_s(2, w.insert(999)).await??;
	timeout_s(2, w.when().reaches(mid_ver)).await?;

	// first late reader
	let n_r1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n_w, &n_r1])).await??;
	let r1 = mosaik::collections::Set::<u64>::reader(&n_r1, store_id);
	timeout_s(10, r1.when().online()).await?;
	timeout_s(10, r1.when().reaches(mid_ver)).await?;
	assert_eq!(r1.len(), 51);
	assert!(r1.contains(&999));

	// add more elements while first reader is alive
	for i in 50u64..80 {
		timeout_s(2, w.insert(i)).await??;
	}
	let late_ver = timeout_s(2, w.insert(1000)).await??;
	timeout_s(2, w.when().reaches(late_ver)).await?;

	// second late reader joins even later
	let n_r2 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n_w, &n_r1, &n_r2])).await??;
	let r2 = mosaik::collections::Set::<u64>::reader(&n_r2, store_id);
	timeout_s(10, r2.when().online()).await?;
	timeout_s(10, r2.when().reaches(late_ver)).await?;

	// first reader should also converge to latest
	timeout_s(10, r1.when().reaches(late_ver)).await?;

	assert_eq!(r1.len(), 82);
	assert_eq!(r2.len(), 82);
	assert!(r1.contains(&1000));
	assert!(r2.contains(&1000));

	// snapshots match
	let snap1: std::collections::HashSet<u64> = r1.iter().collect();
	let snap2: std::collections::HashSet<u64> = r2.iter().collect();
	assert_eq!(snap1, snap2);

	Ok(())
}

// inserting a duplicate is a safe no-op (set semantics)
#[tokio::test]
async fn insert_duplicate() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Set::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Set::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.insert(1)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);

	// duplicate inserts
	let ver = timeout_s(2, w.insert(1)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1); // still only one element

	let ver = timeout_s(2, w.insert(1)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);

	Ok(())
}

// extend with overlapping values — duplicates are ignored
#[tokio::test]
async fn extend_with_duplicates() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Set::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Set::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.extend(vec![1, 2, 3])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);

	// extend with overlapping values — new elements added, existing ignored
	let ver = timeout_s(2, w.extend(vec![2, 3, 4, 5])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 5);
	assert!(r.contains(&1));
	assert!(r.contains(&4));
	assert!(r.contains(&5));

	Ok(())
}

// remove non-existent value is a safe no-op
#[tokio::test]
async fn remove_nonexistent() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Set::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Set::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.extend(vec![1, 2, 3])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// remove a value that doesn't exist — no-op
	let ver = timeout_s(2, w.remove(&999)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);
	assert!(r.contains(&1));
	assert!(r.contains(&2));
	assert!(r.contains(&3));

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

	let w = mosaik::collections::Set::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Set::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.extend(vec![1, 2, 3])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);

	let ver = timeout_s(2, w.clear()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());

	// rebuild after clear — state should be fresh
	let ver = timeout_s(2, w.insert(100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);
	assert!(r.contains(&100));
	assert!(!r.contains(&1)); // old values gone

	Ok(())
}

// extend with empty entries — no crash
#[tokio::test]
async fn extend_empty() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Set::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Set::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.insert(1)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// extending with empty should succeed and return a version
	let _ver_empty = timeout_s(2, w.extend(Vec::<u64>::new())).await??;
	// set should still have just the one element
	assert_eq!(r.len(), 1);

	// subsequent operations still work
	let ver = timeout_s(2, w.insert(2)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 2);

	Ok(())
}

// is_subset between two readers
#[tokio::test]
async fn subset_check() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1, &n2])).await??;

	let w = mosaik::collections::Set::<u64>::writer(&n0, store_id);
	let r1 = mosaik::collections::Set::<u64>::reader(&n1, store_id);
	let r2 = mosaik::collections::Set::<u64>::reader(&n2, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r1.when().online()).await?;
	timeout_s(10, r2.when().online()).await?;

	let ver = timeout_s(2, w.extend(vec![1, 2, 3, 4, 5])).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;

	// both readers have the same contents — each is a subset of the other
	assert!(r1.is_subset(&r2));
	assert!(r2.is_subset(&r1));

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

	let w0 = mosaik::collections::Set::<u64>::writer(&n0, store_id);
	let w1 = mosaik::collections::Set::<u64>::writer(&n1, store_id);
	let r0 = mosaik::collections::Set::<u64>::reader(&n2, store_id);
	let r1 = mosaik::collections::Set::<u64>::reader(&n3, store_id);

	timeout_s(10, w0.when().online()).await?;
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, r0.when().online()).await?;
	timeout_s(10, r1.when().online()).await?;

	// writer 0 inserts some values
	let v = timeout_s(2, w0.extend(vec![1, 2, 3])).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	// writer 1 inserts more
	let v = timeout_s(2, w1.extend(vec![4, 5])).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	assert_eq!(r0.len(), 5);
	assert_eq!(r1.len(), 5);

	// writer 0 removes a value
	let v = timeout_s(2, w0.remove(&1)).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	// writer 1 inserts one more
	let v = timeout_s(2, w1.insert(6)).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;
	timeout_s(2, w0.when().reaches(v)).await?;

	// all nodes agree
	let expected: std::collections::HashSet<u64> = w0.iter().collect();
	assert_eq!(
		w1.iter().collect::<std::collections::HashSet<_>>(),
		expected
	);
	assert_eq!(
		r0.iter().collect::<std::collections::HashSet<_>>(),
		expected
	);
	assert_eq!(
		r1.iter().collect::<std::collections::HashSet<_>>(),
		expected
	);

	let mut expected_vals: Vec<u64> = expected.iter().copied().collect();
	expected_vals.sort_unstable();
	assert_eq!(expected_vals, vec![2, 3, 4, 5, 6]);

	Ok(())
}

// Late writer catches up with existing state then contributes
#[tokio::test]
async fn late_writer_catchup() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w0 = mosaik::collections::Set::<u64>::writer(&n0, store_id);
	timeout_s(10, w0.when().online()).await?;

	// build up some state
	let ver = timeout_s(2, w0.extend(vec![1, 2, 3, 4, 5])).await??;
	timeout_s(2, w0.when().reaches(ver)).await?;

	// late-joining writer
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w1 = mosaik::collections::Set::<u64>::writer(&n1, store_id);
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, w1.when().reaches(ver)).await?;

	// late writer should see existing state
	assert_eq!(w1.len(), 5);

	// late writer can now insert
	let v1 = timeout_s(2, w1.insert(99)).await??;
	timeout_s(2, w0.when().reaches(v1)).await?;

	assert_eq!(w0.len(), 6);
	assert!(w0.contains(&99));
	assert!(w1.contains(&99));

	Ok(())
}

// Writer reads its own writes
#[tokio::test]
async fn writer_reads_own_writes() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w = mosaik::collections::Set::<String>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	let ver = timeout_s(2, w.insert("hello".into())).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	assert_eq!(w.len(), 1);
	assert!(w.contains("hello"));
	assert!(!w.contains("missing"));

	let ver = timeout_s(2, w.insert("foo".into())).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	let snap: std::collections::HashSet<String> = w.iter().collect();
	assert_eq!(snap.len(), 2);
	assert!(snap.contains("hello"));
	assert!(snap.contains("foo"));

	Ok(())
}

// Remove all elements one by one
#[tokio::test]
async fn remove_all_elements() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Set::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Set::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.extend(vec![1, 2, 3])).await??;
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
	let ver = timeout_s(2, w.insert(4)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);
	assert!(r.contains(&4));

	Ok(())
}
