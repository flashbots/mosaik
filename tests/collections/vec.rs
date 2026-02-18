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

	let w = mosaik::collections::Vec::<u64>::new(&n0, store_id);
	let r = mosaik::collections::Vec::<u64>::reader(&n1, store_id);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// -- empty state --
	assert!(r.is_empty());
	assert_eq!(r.len(), 0);
	assert_eq!(r.front(), None);
	assert_eq!(r.back(), None);
	assert_eq!(r.head(), None);
	assert_eq!(r.last(), None);
	assert_eq!(r.get(0), None);
	assert!(!r.contains(&0));
	assert_eq!(r.index_of(&0), None);
	assert_eq!(r.iter().count(), 0);

	// -- push_back --
	let ver = timeout_s(2, w.push_back(10)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);
	assert!(!r.is_empty());
	assert_eq!(r.get(0), Some(10));
	assert_eq!(r.front(), Some(10));
	assert_eq!(r.back(), Some(10));
	assert_eq!(r.head(), Some(10)); // alias of front
	assert_eq!(r.last(), Some(10)); // alias of back
	assert!(r.contains(&10));
	assert_eq!(r.index_of(&10), Some(0));

	// -- push_front --
	let ver = timeout_s(2, w.push_front(5)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	// vec is now [5, 10]
	assert_eq!(r.len(), 2);
	assert_eq!(r.front(), Some(5));
	assert_eq!(r.back(), Some(10));
	assert_eq!(r.get(0), Some(5));
	assert_eq!(r.get(1), Some(10));

	// -- extend --
	let ver = timeout_s(2, w.extend(vec![20, 30, 40])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	// vec is now [5, 10, 20, 30, 40]
	assert_eq!(r.len(), 5);
	assert_eq!(r.get(2), Some(20));
	assert_eq!(r.get(3), Some(30));
	assert_eq!(r.get(4), Some(40));

	// -- insert at index --
	let ver = timeout_s(2, w.insert(2, 15)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	// vec is now [5, 10, 15, 20, 30, 40]
	assert_eq!(r.len(), 6);
	assert_eq!(r.get(2), Some(15));
	assert_eq!(r.get(3), Some(20));

	// -- contains & index_of --
	assert!(r.contains(&15));
	assert!(!r.contains(&99));
	assert_eq!(r.index_of(&15), Some(2));
	assert_eq!(r.index_of(&99), None);

	// -- iter --
	let snapshot: std::vec::Vec<u64> = r.iter().collect();
	assert_eq!(snapshot, vec![5, 10, 15, 20, 30, 40]);

	// -- swap --
	let ver = timeout_s(2, w.swap(0, 5)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	// vec is now [40, 10, 15, 20, 30, 5]
	assert_eq!(r.front(), Some(40));
	assert_eq!(r.back(), Some(5));

	// -- pop_back --
	let ver = timeout_s(2, w.pop_back()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	// vec is now [40, 10, 15, 20, 30]
	assert_eq!(r.len(), 5);
	assert_eq!(r.back(), Some(30));

	// -- pop_front --
	let ver = timeout_s(2, w.pop_front()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	// vec is now [10, 15, 20, 30]
	assert_eq!(r.len(), 4);
	assert_eq!(r.front(), Some(10));

	// -- remove by index --
	let ver = timeout_s(2, w.remove(1)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	// vec is now [10, 20, 30]
	assert_eq!(r.len(), 3);
	assert_eq!(r.get(1), Some(20));

	// -- truncate --
	let ver = timeout_s(2, w.truncate(2)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	// vec is now [10, 20]
	assert_eq!(r.len(), 2);
	assert_eq!(r.get(2), None);

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
	let w = mosaik::collections::Vec::<u64>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	// build a non-trivial history: push 0..99
	for i in 0u64..100 {
		timeout_s(2, w.push_back(i + 5)).await??;
	}

	let ver = timeout_s(2, w.extend([10, 20, 30, 40, 50, 60, 70])).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	// late-joining reader
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let r = mosaik::collections::Vec::<u64>::reader(&n1, store_id);
	timeout_s(10, r.when().online()).await?;
	timeout_s(10, r.when().reaches(ver)).await?;

	assert_eq!(r.len(), 107);
	for i in 0u64..100 {
		assert_eq!(r.get(i), Some(i + 5));
	}
	for i in 0u64..7 {
		assert_eq!(r.get(100 + i), Some((i + 1) * 10));
	}

	// writer can continue operating after reader caught up
	let ver = timeout_s(2, w.remove(50)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 106);
	assert_eq!(r.get(50), Some(56));

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

	let w = mosaik::collections::Vec::<u64>::writer(&n_w, store_id);
	let r1 = mosaik::collections::Vec::<u64>::reader(&n_r1, store_id);
	let r2 = mosaik::collections::Vec::<u64>::reader(&n_r2, store_id);
	let r3 = mosaik::collections::Vec::<u64>::reader(&n_r3, store_id);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r1.when().online()).await?;
	timeout_s(10, r2.when().online()).await?;
	timeout_s(10, r3.when().online()).await?;

	// push a batch
	let ver = timeout_s(2, w.extend(vec![1, 2, 3, 4, 5])).await??;

	// all readers converge
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert_eq!(r.len(), 5);
		let snap: std::vec::Vec<u64> = r.iter().collect();
		assert_eq!(snap, vec![1, 2, 3, 4, 5]);
	}

	// more mutations replicate to all
	let ver = timeout_s(2, w.push_front(0)).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert_eq!(r.front(), Some(0));
		assert_eq!(r.len(), 6);
	}

	let ver = timeout_s(2, w.pop_back()).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert_eq!(r.len(), 5);
		assert_eq!(r.back(), Some(4));
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

	let w0 = mosaik::collections::Vec::<u64>::writer(&n0, store_id);
	let w1 = mosaik::collections::Vec::<u64>::writer(&n1, store_id);
	let r = mosaik::collections::Vec::<u64>::reader(&n2, store_id);

	timeout_s(10, w0.when().online()).await?;
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// both writers push values (sequentially to avoid races in the test itself)
	let v0 = timeout_s(2, w0.push_back(100)).await??;
	timeout_s(2, r.when().reaches(v0)).await?;

	let v1 = timeout_s(2, w1.push_back(200)).await??;
	timeout_s(2, r.when().reaches(v1)).await?;

	let v2 = timeout_s(2, w0.push_back(300)).await??;
	timeout_s(2, r.when().reaches(v2)).await?;

	assert_eq!(r.len(), 3);
	// all three values are present; the total order is decided by raft consensus
	let snap: std::vec::Vec<u64> = r.iter().collect();
	assert!(snap.contains(&100));
	assert!(snap.contains(&200));
	assert!(snap.contains(&300));

	// a writer that extends
	let v3 = timeout_s(2, w1.extend(vec![400, 500])).await??;
	timeout_s(2, r.when().reaches(v3)).await?;

	assert_eq!(r.len(), 5);
	assert!(r.contains(&400));
	assert!(r.contains(&500));

	timeout_s(2, w0.when().reaches(v3)).await?;
	timeout_s(2, w1.when().reaches(v3)).await?;

	// all three nodes agree on the snapshot
	let snap_w0: std::vec::Vec<u64> = w0.iter().collect();
	let snap_w1: std::vec::Vec<u64> = w1.iter().collect();
	let snap_r: std::vec::Vec<u64> = r.iter().collect();
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
	let w = mosaik::collections::Vec::<u64>::writer(&n_w, store_id);
	timeout_s(10, w.when().online()).await?;

	// build history
	for i in 0u64..50 {
		timeout_s(2, w.push_back(i)).await??;
	}
	let mid_ver = timeout_s(2, w.push_back(999)).await??;
	timeout_s(2, w.when().reaches(mid_ver)).await?;

	// first late reader
	let n_r1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n_w, &n_r1])).await??;
	let r1 = mosaik::collections::Vec::<u64>::reader(&n_r1, store_id);
	timeout_s(10, r1.when().online()).await?;
	timeout_s(10, r1.when().reaches(mid_ver)).await?;
	assert_eq!(r1.len(), 51);
	assert_eq!(r1.back(), Some(999));

	// add more entries while first reader is alive
	for i in 50u64..80 {
		timeout_s(2, w.push_back(i)).await??;
	}
	let late_ver = timeout_s(2, w.push_back(1000)).await??;
	timeout_s(2, w.when().reaches(late_ver)).await?;

	// second late reader joins even later
	let n_r2 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n_w, &n_r1, &n_r2])).await??;
	let r2 = mosaik::collections::Vec::<u64>::reader(&n_r2, store_id);
	timeout_s(10, r2.when().online()).await?;
	timeout_s(10, r2.when().reaches(late_ver)).await?;

	// first reader should also converge to latest
	timeout_s(10, r1.when().reaches(late_ver)).await?;

	assert_eq!(r1.len(), 82);
	assert_eq!(r2.len(), 82);
	assert_eq!(r1.back(), Some(1000));
	assert_eq!(r2.back(), Some(1000));

	// snapshots match
	let snap1: std::vec::Vec<u64> = r1.iter().collect();
	let snap2: std::vec::Vec<u64> = r2.iter().collect();
	assert_eq!(snap1, snap2);

	Ok(())
}

// push_front / push_back ordering preserved
#[tokio::test]
async fn push_front_back_ordering() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Vec::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Vec::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// alternating push_front and push_back
	timeout_s(2, w.push_back(3)).await??; // [3]
	timeout_s(2, w.push_front(2)).await??; // [2, 3]
	timeout_s(2, w.push_back(4)).await??; // [2, 3, 4]
	timeout_s(2, w.push_front(1)).await??; // [1, 2, 3, 4]
	let ver = timeout_s(2, w.push_back(5)).await??; // [1, 2, 3, 4, 5]

	timeout_s(2, r.when().reaches(ver)).await?;
	let snap: std::vec::Vec<u64> = r.iter().collect();
	assert_eq!(snap, vec![1, 2, 3, 4, 5]);

	Ok(())
}

// pop_front / pop_back until empty
#[tokio::test]
async fn pop_until_empty() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Vec::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Vec::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.extend(vec![1, 2, 3, 4])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 4);

	// pop front: removes 1
	let ver = timeout_s(2, w.pop_front()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.front(), Some(2));

	// pop back: removes 4
	let ver = timeout_s(2, w.pop_back()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.back(), Some(3));

	// pop remaining [2, 3]
	let ver = timeout_s(2, w.pop_front()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	let ver = timeout_s(2, w.pop_back()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	assert!(r.is_empty());

	// pop on empty is a no-op (should not panic)
	let ver = timeout_s(2, w.pop_back()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());

	let ver = timeout_s(2, w.pop_front()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());

	Ok(())
}

// swap: normal, same-index, out-of-bounds (no-op)
#[tokio::test]
async fn swap_variations() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Vec::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Vec::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.extend(vec![10, 20, 30])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// normal swap
	let ver = timeout_s(2, w.swap(0, 2)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.iter().collect::<std::vec::Vec<_>>(), vec![30, 20, 10]);

	// swap same index — no-op
	let ver = timeout_s(2, w.swap(1, 1)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.iter().collect::<std::vec::Vec<_>>(), vec![30, 20, 10]);

	// out-of-bounds swap — no-op, should not panic
	let ver = timeout_s(2, w.swap(0, 999)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.iter().collect::<std::vec::Vec<_>>(), vec![30, 20, 10]);

	Ok(())
}

// insert at various positions
#[tokio::test]
async fn insert_at_positions() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Vec::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Vec::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// insert into empty — becomes sole element
	let ver = timeout_s(2, w.insert(0, 50)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.iter().collect::<std::vec::Vec<_>>(), vec![50]);

	// insert at front
	let ver = timeout_s(2, w.insert(0, 10)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.iter().collect::<std::vec::Vec<_>>(), vec![10, 50]);

	// insert at end (index == len)
	let ver = timeout_s(2, w.insert(2, 90)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.iter().collect::<std::vec::Vec<_>>(), vec![10, 50, 90]);

	// insert in the middle
	let ver = timeout_s(2, w.insert(1, 30)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.iter().collect::<std::vec::Vec<_>>(), vec![10, 30, 50, 90]);

	Ok(())
}

// truncate edge cases
#[tokio::test]
async fn truncate_edge_cases() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Vec::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Vec::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.extend(vec![1, 2, 3, 4, 5])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// truncate to a length larger than current — no-op
	let ver = timeout_s(2, w.truncate(100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 5);

	// truncate to exact current length — no-op
	let ver = timeout_s(2, w.truncate(5)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 5);

	// truncate to smaller
	let ver = timeout_s(2, w.truncate(3)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);
	assert_eq!(r.iter().collect::<std::vec::Vec<_>>(), vec![1, 2, 3]);

	// truncate to 0 — empties the vec
	let ver = timeout_s(2, w.truncate(0)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());

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

	let w = mosaik::collections::Vec::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Vec::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.extend(vec![1, 2, 3])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);

	let ver = timeout_s(2, w.clear()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());

	// rebuild after clear — state should be fresh
	let ver = timeout_s(2, w.push_back(100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 1);
	assert_eq!(r.get(0), Some(100));

	Ok(())
}

// extend with empty vec — no version bump needed, no crash
#[tokio::test]
async fn extend_empty() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Vec::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Vec::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.push_back(1)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// extending with empty should succeed and return a version
	let _ver_empty =
		timeout_s(2, w.extend(std::vec::Vec::<u64>::new())).await??;
	// vec should still have just the one element
	assert_eq!(r.len(), 1);

	// subsequent operations still work
	let ver = timeout_s(2, w.push_back(2)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 2);

	Ok(())
}

// remove out-of-bounds is a safe no-op
#[tokio::test]
async fn remove_out_of_bounds() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Vec::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Vec::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.extend(vec![1, 2, 3])).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	// remove at index beyond length — no-op
	let ver = timeout_s(2, w.remove(100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.len(), 3);
	assert_eq!(r.iter().collect::<std::vec::Vec<_>>(), vec![1, 2, 3]);

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

	let w0 = mosaik::collections::Vec::<u64>::writer(&n0, store_id);
	let w1 = mosaik::collections::Vec::<u64>::writer(&n1, store_id);
	let r0 = mosaik::collections::Vec::<u64>::reader(&n2, store_id);
	let r1 = mosaik::collections::Vec::<u64>::reader(&n3, store_id);

	timeout_s(10, w0.when().online()).await?;
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, r0.when().online()).await?;
	timeout_s(10, r1.when().online()).await?;

	// writer 0 pushes some values
	let v = timeout_s(2, w0.extend(vec![10, 20, 30])).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	// writer 1 pushes more
	let v = timeout_s(2, w1.extend(vec![40, 50])).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	assert_eq!(r0.len(), 5);
	assert_eq!(r1.len(), 5);

	// writer 0 removes from front
	let v = timeout_s(2, w0.pop_front()).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	// writer 1 pushes one more
	let v = timeout_s(2, w1.push_back(60)).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	// all nodes agree
	let expected: std::vec::Vec<u64> = w0.iter().collect();
	assert_eq!(w1.iter().collect::<std::vec::Vec<_>>(), expected);
	assert_eq!(r0.iter().collect::<std::vec::Vec<_>>(), expected);
	assert_eq!(r1.iter().collect::<std::vec::Vec<_>>(), expected);
	assert_eq!(expected, vec![20, 30, 40, 50, 60]);

	Ok(())
}

// Late writer catches up with existing state then contributes
#[tokio::test]
async fn late_writer_catchup() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w0 = mosaik::collections::Vec::<u64>::writer(&n0, store_id);
	timeout_s(10, w0.when().online()).await?;

	// build up some state
	let ver = timeout_s(2, w0.extend(vec![1, 2, 3, 4, 5])).await??;
	timeout_s(2, w0.when().reaches(ver)).await?;

	// late-joining writer
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w1 = mosaik::collections::Vec::<u64>::writer(&n1, store_id);
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, w1.when().reaches(ver)).await?;

	// late writer should see existing state
	assert_eq!(w1.len(), 5);

	// late writer can now push
	let v1 = timeout_s(2, w1.push_back(99)).await??;
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
	let w = mosaik::collections::Vec::<String>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	let ver = timeout_s(2, w.push_back("hello".into())).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	assert_eq!(w.len(), 1);
	assert_eq!(w.get(0), Some("hello".into()));
	assert!(w.contains(&"hello".into()));
	assert_eq!(w.index_of(&"hello".into()), Some(0));
	assert_eq!(w.front(), Some("hello".into()));
	assert_eq!(w.back(), Some("hello".into()));

	let ver = timeout_s(2, w.push_back("world".into())).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	let snap: std::vec::Vec<String> = w.iter().collect();
	assert_eq!(snap, vec!["hello".to_string(), "world".to_string()]);

	Ok(())
}
