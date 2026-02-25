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

	let w = mosaik::collections::Register::<u64>::new(&n0, store_id);
	let r = mosaik::collections::Register::<u64>::reader(&n1, store_id);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// -- empty state --
	assert!(r.is_empty());
	assert_eq!(r.read(), None);
	assert_eq!(r.get(), None);

	// -- write --
	let ver = timeout_s(2, w.write(42)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(!r.is_empty());
	assert_eq!(r.read(), Some(42));
	assert_eq!(r.get(), Some(42));

	// -- overwrite --
	let ver = timeout_s(2, w.write(99)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.read(), Some(99));

	// -- set (alias) --
	let ver = timeout_s(2, w.set(123)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.read(), Some(123));

	// -- clear --
	let ver = timeout_s(2, w.clear()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());
	assert_eq!(r.read(), None);

	Ok(())
}

// Late-joining reader catches up via snapshot
#[tokio::test]
async fn catchup_single_reader() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w = mosaik::collections::Register::<u64>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	// build a non-trivial history: overwrite many times
	for i in 0u64..100 {
		timeout_s(2, w.write(i)).await??;
	}

	let ver = timeout_s(2, w.write(999)).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	// late-joining reader
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let r = mosaik::collections::Register::<u64>::reader(&n1, store_id);
	timeout_s(10, r.when().online()).await?;
	timeout_s(10, r.when().reaches(ver)).await?;

	// register only holds the latest value
	assert_eq!(r.read(), Some(999));

	// writer can continue operating after reader caught up
	let ver = timeout_s(2, w.write(1000)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.read(), Some(1000));

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

	let w = mosaik::collections::Register::<u64>::writer(&n_w, store_id);
	let r1 = mosaik::collections::Register::<u64>::reader(&n_r1, store_id);
	let r2 = mosaik::collections::Register::<u64>::reader(&n_r2, store_id);
	let r3 = mosaik::collections::Register::<u64>::reader(&n_r3, store_id);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r1.when().online()).await?;
	timeout_s(10, r2.when().online()).await?;
	timeout_s(10, r3.when().online()).await?;

	// write a value
	let ver = timeout_s(2, w.write(42)).await??;

	// all readers converge
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert_eq!(r.read(), Some(42));
	}

	// overwrite replicates to all
	let ver = timeout_s(2, w.write(100)).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert_eq!(r.read(), Some(100));
	}

	// clear replicates to all
	let ver = timeout_s(2, w.clear()).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert!(r.is_empty());
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

	let w0 = mosaik::collections::Register::<u64>::writer(&n0, store_id);
	let w1 = mosaik::collections::Register::<u64>::writer(&n1, store_id);
	let r = mosaik::collections::Register::<u64>::reader(&n2, store_id);

	timeout_s(10, w0.when().online()).await?;
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// both writers write values (sequentially to avoid races in the test itself)
	let v0 = timeout_s(2, w0.write(10)).await??;
	timeout_s(2, r.when().reaches(v0)).await?;
	assert_eq!(r.read(), Some(10));

	let v1 = timeout_s(2, w1.write(20)).await??;
	timeout_s(2, r.when().reaches(v1)).await?;
	assert_eq!(r.read(), Some(20));

	let v2 = timeout_s(2, w0.write(30)).await??;
	timeout_s(2, r.when().reaches(v2)).await?;
	assert_eq!(r.read(), Some(30));

	timeout_s(2, w0.when().reaches(v2)).await?;
	timeout_s(2, w1.when().reaches(v2)).await?;

	// all three nodes agree on the value
	assert_eq!(w0.read(), w1.read());
	assert_eq!(w0.read(), r.read());

	Ok(())
}

// Multiple late-joining readers catch up independently
#[tokio::test]
async fn catchup_multiple_readers() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n_w = Network::new(network_id).await?;
	let w = mosaik::collections::Register::<u64>::writer(&n_w, store_id);
	timeout_s(10, w.when().online()).await?;

	// build history
	for i in 0u64..50 {
		timeout_s(2, w.write(i)).await??;
	}
	let mid_ver = timeout_s(2, w.write(999)).await??;
	timeout_s(2, w.when().reaches(mid_ver)).await?;

	// first late reader
	let n_r1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n_w, &n_r1])).await??;
	let r1 = mosaik::collections::Register::<u64>::reader(&n_r1, store_id);
	timeout_s(10, r1.when().online()).await?;
	timeout_s(10, r1.when().reaches(mid_ver)).await?;
	assert_eq!(r1.read(), Some(999));

	// more writes while first reader is alive
	for i in 50u64..80 {
		timeout_s(2, w.write(i)).await??;
	}
	let late_ver = timeout_s(2, w.write(1000)).await??;
	timeout_s(2, w.when().reaches(late_ver)).await?;

	// second late reader joins even later
	let n_r2 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n_w, &n_r1, &n_r2])).await??;
	let r2 = mosaik::collections::Register::<u64>::reader(&n_r2, store_id);
	timeout_s(10, r2.when().online()).await?;
	timeout_s(10, r2.when().reaches(late_ver)).await?;

	// first reader should also converge to latest
	timeout_s(10, r1.when().reaches(late_ver)).await?;

	assert_eq!(r1.read(), Some(1000));
	assert_eq!(r2.read(), Some(1000));

	Ok(())
}

// Overwriting repeatedly always keeps only the latest value
#[tokio::test]
async fn overwrite_keeps_latest() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Register::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Register::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.write(1)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.read(), Some(1));

	let ver = timeout_s(2, w.write(2)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.read(), Some(2));

	let ver = timeout_s(2, w.write(3)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.read(), Some(3));

	// only the final value is present, not a history
	assert!(!r.is_empty());

	Ok(())
}

// Clear then write again
#[tokio::test]
async fn clear_then_write() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Register::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Register::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.write(42)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.read(), Some(42));

	let ver = timeout_s(2, w.clear()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());

	// write after clear — state should be fresh
	let ver = timeout_s(2, w.write(100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.read(), Some(100));
	assert!(!r.is_empty());

	Ok(())
}

// Clear on empty register is a safe no-op
#[tokio::test]
async fn clear_empty() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Register::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Register::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// clear on empty should succeed and not panic
	let ver = timeout_s(2, w.clear()).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_empty());

	// subsequent operations still work
	let ver = timeout_s(2, w.write(1)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.read(), Some(1));

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

	let w0 = mosaik::collections::Register::<u64>::writer(&n0, store_id);
	let w1 = mosaik::collections::Register::<u64>::writer(&n1, store_id);
	let r0 = mosaik::collections::Register::<u64>::reader(&n2, store_id);
	let r1 = mosaik::collections::Register::<u64>::reader(&n3, store_id);

	timeout_s(10, w0.when().online()).await?;
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, r0.when().online()).await?;
	timeout_s(10, r1.when().online()).await?;

	// writer 0 sets a value
	let v = timeout_s(2, w0.write(10)).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	assert_eq!(r0.read(), Some(10));
	assert_eq!(r1.read(), Some(10));

	// writer 1 overwrites
	let v = timeout_s(2, w1.write(20)).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	assert_eq!(r0.read(), Some(20));
	assert_eq!(r1.read(), Some(20));

	// writer 0 clears
	let v = timeout_s(2, w0.clear()).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;

	// writer 1 writes again
	let v = timeout_s(2, w1.write(30)).await??;
	timeout_s(2, r0.when().reaches(v)).await?;
	timeout_s(2, r1.when().reaches(v)).await?;
	timeout_s(2, w0.when().reaches(v)).await?;

	// all nodes agree
	assert_eq!(w0.read(), Some(30));
	assert_eq!(w1.read(), Some(30));
	assert_eq!(r0.read(), Some(30));
	assert_eq!(r1.read(), Some(30));

	Ok(())
}

// Late writer catches up with existing state then contributes
#[tokio::test]
async fn late_writer_catchup() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w0 = mosaik::collections::Register::<u64>::writer(&n0, store_id);
	timeout_s(10, w0.when().online()).await?;

	// build up some state
	let ver = timeout_s(2, w0.write(42)).await??;
	timeout_s(2, w0.when().reaches(ver)).await?;

	// late-joining writer
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w1 = mosaik::collections::Register::<u64>::writer(&n1, store_id);
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, w1.when().reaches(ver)).await?;

	// late writer should see existing state
	assert_eq!(w1.read(), Some(42));

	// late writer can now write
	let v1 = timeout_s(2, w1.write(99)).await??;
	timeout_s(2, w0.when().reaches(v1)).await?;

	assert_eq!(w0.read(), Some(99));
	assert_eq!(w1.read(), Some(99));

	Ok(())
}

// Writer reads its own writes
#[tokio::test]
async fn writer_reads_own_writes() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w = mosaik::collections::Register::<String>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	let ver = timeout_s(2, w.write("hello".into())).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	assert_eq!(w.read(), Some("hello".into()));
	assert!(!w.is_empty());

	let ver = timeout_s(2, w.write("world".into())).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	assert_eq!(w.read(), Some("world".into()));

	Ok(())
}

// Write, clear, write cycle multiple times
#[tokio::test]
async fn write_clear_cycle() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Register::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Register::<u64>::reader(&n1, store_id);
	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	for i in 0u64..10 {
		let ver = timeout_s(2, w.write(i)).await??;
		timeout_s(2, r.when().reaches(ver)).await?;
		assert_eq!(r.read(), Some(i));

		let ver = timeout_s(2, w.clear()).await??;
		timeout_s(2, r.when().reaches(ver)).await?;
		assert!(r.is_empty());
	}

	// final write after all the cycles
	let ver = timeout_s(2, w.write(999)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.read(), Some(999));

	Ok(())
}
