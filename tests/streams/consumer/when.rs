use {
	super::*,
	crate::utils::{poll_once, timeout_s},
	core::task::Poll,
	mosaik::*,
};

#[tokio::test]
async fn smoke() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;

	tracing::debug!("n0: {}", n0.local().id());
	tracing::debug!("n1: {}", n1.local().id());
	tracing::debug!("n2: {}", n2.local().id());

	// discover peers
	n1.discovery().dial(n0.local().id()).await;
	n2.discovery().dial(n0.local().id()).await;

	// 1 consumer, 0 producers
	let c1 = n1.streams().consume::<Data1>();

	let mut condition1 = c1.when().subscribed();
	let mut condition2 = c1.when().subscribed().to_at_least(2);
	let mut condition3 = condition2.clone().not(); // inverse of condition2
	let condition4 = c1.when().unsubscribed(); // inverse of condition1

	assert!(!condition1.is_condition_met());
	assert!(!condition2.is_condition_met());
	assert!(condition3.is_condition_met());
	assert!(condition4.is_condition_met());

	assert_eq!(poll_once(&mut condition1), Poll::Pending);
	assert_eq!(poll_once(&mut condition2), Poll::Pending);
	assert_eq!(poll_once(&mut condition3.clone()), Poll::Ready(()));
	assert_eq!(poll_once(&mut condition4.clone()), Poll::Ready(()));

	// 1 consumer, 1 producer
	let _p0 = n0.streams().produce::<Data1>();

	// should resolve because we have 1 producer now
	timeout_s(3, &mut condition1)
		.await
		.expect("timeout waiting for condition1");
	tracing::debug!("consumer subscribed with at least 1 producer");

	assert!(condition1.is_condition_met());
	assert!(!condition2.is_condition_met());
	assert!(condition3.is_condition_met());
	assert_eq!(poll_once(&mut condition2), Poll::Pending);

	// 1 consumer, 2 producers
	let _p2 = n2.streams().produce::<Data1>();

	// should resolve because we have 2 producers now
	timeout_s(3, &mut condition2)
		.await
		.expect("timeout waiting for condition2");
	tracing::debug!("consumer subscribed with at least 2 producers");

	assert!(condition1.is_condition_met());
	assert!(condition2.is_condition_met());
	assert!(!condition3.is_condition_met());
	assert!(!condition4.is_condition_met());

	// clones resolve immediately if the condition is met
	let mut cond1_clone = condition1.clone();
	assert_eq!(poll_once(&mut cond1_clone), Poll::Ready(()));

	let mut cond2_clone = condition2.clone();
	assert_eq!(poll_once(&mut cond2_clone), Poll::Ready(()));

	// if we drop one producer, condition2 should go back to pending
	// and its inverse (condition3) should resolve
	assert!(condition1.is_condition_met());
	assert!(condition2.is_condition_met());
	assert!(!condition3.is_condition_met());
	assert!(!condition4.is_condition_met());
	assert_eq!(poll_once(&mut condition3.clone()), Poll::Pending);

	tracing::debug!("dropping n2");
	drop(n2);

	// should resolve because we are back to 1 producer
	timeout_s(3, &mut condition3)
		.await
		.expect("timeout waiting for inverse condition2");

	tracing::debug!("consumer no longer subscribed with at least 2 producers");

	assert!(!condition2.is_condition_met());
	assert!(condition3.is_condition_met());
	assert_eq!(poll_once(&mut condition2.clone()), Poll::Pending);

	assert!(condition3.is_condition_met());
	assert_eq!(poll_once(&mut condition3.clone()), Poll::Ready(()));

	// if we add another producer, condition2 should resolve again because its
	// criteria is met, and condition3 should go back to pending
	let n3 = Network::new(network_id).await?;
	tracing::debug!("n3: {}", n3.local().id());
	n3.discovery().dial(n0.local().id()).await;

	// add producer on n3 for Data1 stream
	let _p3 = n3.streams().produce::<Data1>();

	timeout_s(3, &mut condition2)
		.await
		.expect("timeout waiting for condition2 after n3 producer");
	tracing::debug!("consumer subscribed with at least 2 producers again");

	assert!(condition2.is_condition_met());
	assert!(!condition3.is_condition_met());
	assert_eq!(poll_once(&mut condition2.clone()), Poll::Ready(()));
	assert_eq!(poll_once(&mut condition3.clone()), Poll::Pending);

	Ok(())
}
