use {
	super::*,
	crate::utils::{discover_all, sleep_s, timeout_s},
	core::time::Duration,
	futures::{SinkExt, StreamExt},
	mosaik::*,
};

/// Verifies that a producer with a generous RTT requirement accepts
/// local consumers (whose RTT is effectively zero).
#[tokio::test]
async fn require_rtt_accepts_fast_peers() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;

	// Producer requires consumers with RTT < 5s — trivially satisfied
	// for local connections.
	let mut p0 = n0
		.streams()
		.producer::<Data1>()
		.require(|peer| peer.rtt_below(Duration::from_secs(5)))
		.build()?;

	let mut c1 = n1.streams().consume::<Data1>();

	discover_all([&n0, &n1]).await?;

	// Consumer should be accepted (the producer samples RTT from the
	// QUIC connection at accept time — local RTT is sub-millisecond).
	timeout_s(5, c1.when().subscribed()).await?;
	timeout_s(5, p0.when().subscribed().minimum_of(1)).await?;

	// Verify data flows
	p0.send(Data1::new("rtt-ok")).await?;
	let msg = timeout_s(5, c1.next()).await?.unwrap();
	assert_eq!(msg, Data1::new("rtt-ok"));

	Ok(())
}

/// Verifies that a producer with an impossibly low RTT requirement
/// rejects consumers. The producer samples RTT from the QUIC
/// connection at accept time, so there is no optimistic admission —
/// the consumer is rejected immediately because no real connection
/// has sub-nanosecond RTT.
#[tokio::test]
async fn require_rtt_rejects_slow_peers() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;

	// Producer requires RTT < 1ns — impossible for any real connection.
	let p0 = n0
		.streams()
		.producer::<Data1>()
		.require(|peer| peer.rtt_below(Duration::from_nanos(1)))
		.build()?;

	let _c1 = n1.streams().consume::<Data1>();

	discover_all([&n0, &n1]).await?;

	// Give the consumer a chance to attempt subscription.
	sleep_s(3).await;

	// The consumer should never successfully subscribe because the
	// producer rejects based on the QUIC connection RTT.
	assert_eq!(
		p0.consumers().count(),
		0,
		"consumer should have been rejected due to RTT requirement"
	);

	Ok(())
}
