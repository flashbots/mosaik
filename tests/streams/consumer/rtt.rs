use {
	super::*,
	crate::utils::{discover_all, sleep_s, timeout_s},
	core::time::Duration,
	futures::{SinkExt, StreamExt},
	mosaik::*,
};

/// Verifies that a consumer with a generous RTT requirement subscribes
/// to local producers (whose RTT is effectively zero).
#[tokio::test]
async fn require_rtt_accepts_fast_peers() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;

	let mut p0 = n0.streams().produce::<Data1>();

	// Consumer requires producers with RTT < 5s — trivially satisfied
	// for local connections.
	let mut c1 = n1
		.streams()
		.consumer::<Data1>()
		.require(|peer| peer.rtt_below(Duration::from_secs(5)))
		.build();

	discover_all([&n0, &n1]).await?;

	// The consumer pings the producer before subscribing to measure
	// RTT. Local RTT is sub-millisecond, so it passes the threshold.
	timeout_s(5, c1.when().subscribed()).await?;

	// Verify data flows
	p0.send(Data1::new("rtt-ok")).await?;
	let msg = timeout_s(5, c1.next()).await?.unwrap();
	assert_eq!(msg, Data1::new("rtt-ok"));

	Ok(())
}

/// Verifies that a consumer with an impossibly low RTT requirement
/// never subscribes to producers. The consumer pings the producer
/// before subscribing, discovers the RTT exceeds the threshold, and
/// skips the subscription entirely — no wasteful connect/disconnect
/// cycle.
#[tokio::test]
async fn require_rtt_rejects_slow_peers() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;

	let _p0 = n0.streams().produce::<Data1>();

	// Consumer requires RTT < 1ns — impossible for any real connection.
	let c1 = n1
		.streams()
		.consumer::<Data1>()
		.require(|peer| peer.rtt_below(Duration::from_nanos(1)))
		.build();

	discover_all([&n0, &n1]).await?;

	// Give the consumer time to discover the producer and probe RTT.
	sleep_s(3).await;

	// The consumer should never subscribe — the ping probe reveals
	// that RTT exceeds the threshold before a connection is attempted.
	assert_eq!(
		c1.producers().count(),
		0,
		"consumer should not have subscribed due to RTT requirement"
	);

	Ok(())
}
