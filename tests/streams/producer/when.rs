use {
	super::*,
	crate::utils::{discover_all, poll_once, timeout_s},
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

	let p1 = n1.streams().produce::<Data1>();
	p1.when().online().await;

	n0.online().await;
	n1.online().await;
	n2.online().await;

	// discover peers
	discover_all([&n0, &n1, &n2]).await?;

	let mut condition1 = p1.when().subscribed();
	let mut condition2 = p1.when().subscribed().minimum_of(2);
	let mut condition3 = condition2.clone().unmet(); // inverse of condition2
	let condition4 = p1.when().unsubscribed(); // inverse of condition1

	assert_eq!(poll_once(&mut condition1), Poll::Pending);
	assert_eq!(poll_once(&mut condition2), Poll::Pending);
	assert_eq!(poll_once(&mut condition3.clone()), Poll::Ready(()));
	assert_eq!(poll_once(&mut condition4.clone()), Poll::Ready(()));

	// 1 consumer, 1 producer
	let c0 = n0.streams().consume::<Data1>();
	c0.when().online().await;

	// should resolve because we have 1 consumer now
	timeout_s(3, &mut condition1)
		.await
		.expect("timeout waiting for condition1");
	tracing::debug!("producer has at least 1 subscriber");

	assert!(condition1.is_condition_met());
	assert!(!condition2.is_condition_met());
	assert!(condition3.is_condition_met());
	assert_eq!(poll_once(&mut condition2), Poll::Pending);

	// 1 producer, 2 consumers
	let c2 = n2.streams().consume::<Data1>();
	c2.when().online().await;

	// should resolve because we have 2 consumers now
	timeout_s(3, &mut condition2)
		.await
		.expect("timeout waiting for condition2");
	tracing::debug!("producer has at least 2 subscribers");

	assert!(condition1.is_condition_met());
	assert!(condition2.is_condition_met());
	assert!(!condition3.is_condition_met());
	assert!(!condition4.is_condition_met());

	// clones resolve immediately if the condition is met
	let mut cond1_clone = condition1.clone();
	assert_eq!(poll_once(&mut cond1_clone), Poll::Ready(()));

	let mut cond2_clone = condition2.clone();
	assert_eq!(poll_once(&mut cond2_clone), Poll::Ready(()));

	// if we drop one consumer, condition2 should go back to pending
	// and its inverse (condition3) should resolve
	assert!(condition1.is_condition_met());
	assert!(condition2.is_condition_met());
	assert!(!condition3.is_condition_met());
	assert!(!condition4.is_condition_met());
	assert_eq!(poll_once(&mut condition3.clone()), Poll::Pending);

	tracing::debug!("dropping n2");
	drop(n2);

	// should resolve because we are back to 1 consumer
	timeout_s(3, &mut condition3)
		.await
		.expect("timeout waiting for inverse condition2");
	tracing::debug!("producer back to having 1 subscriber");

	assert!(!condition2.is_condition_met());
	assert!(condition3.is_condition_met());
	assert_eq!(poll_once(&mut condition2.clone()), Poll::Pending);
	assert_eq!(poll_once(&mut condition3.clone()), Poll::Ready(()));

	// if we add another consumer, condition2 should resolve again because its
	// criteria is met, and condition3 should go back to pending
	let n3 = Network::new(network_id).await?;
	tracing::debug!("n3: {}", n3.local().id());
	discover_all([&n0, &n1, &n3]).await?;

	// add consumer on n3 for Data1 stream
	let p3 = n3.streams().consume::<Data1>();
	p3.when().online().await;

	timeout_s(3, &mut condition2)
		.await
		.expect("timeout waiting for condition2 after n3 consumer");
	tracing::debug!("producer subscribed with at least 2 consumers again");

	assert!(condition2.is_condition_met());
	assert!(!condition3.is_condition_met());
	assert_eq!(poll_once(&mut condition2.clone()), Poll::Ready(()));
	assert_eq!(poll_once(&mut condition3.clone()), Poll::Pending);

	Ok(())
}
