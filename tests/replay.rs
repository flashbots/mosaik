use {mosaik::prelude::*, std::collections::HashMap};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Data1 {
	pub a: String,
	pub b: u64,
}

#[tokio::test]
async fn accumulator_api_design() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id.clone()).await?;
	let p0_1 = n0.produce::<Data1>().keyed_by(|d| d.b);

	let mut acc_p0_1 =
		Accumulated::new(p0_1, |acc: &mut HashMap<u64, Data1>, datum| {
			acc.extend([datum.into()]);
		});

	let n1 = Network::new(network_id.clone()).await?;
	let mut c1_1 = n1.consume::<Data1>().keyed_by(|d| d.b);

	full_manual_disco(&[&n0, &n1]);

	acc_p0_1.as_ref().status().subscribed().await;
	c1_1.status().subscribed().await;
	tracing::info!("Both sides subscribed");

	for i in 0..10 {
		acc_p0_1
			.send(Data1 {
				a: format!("Message #{i}"),
				b: i + 3,
			})
			.await?;
	}

	for i in 0..10 {
		let recv_c1_1 = c1_1.next().await;
		let Some(datum) = recv_c1_1 else {
			panic!("Expected datum received None");
		};

		let (key, datum) = datum.into();

		assert_eq!(key, i + 3);
		assert_eq!(datum, Data1 {
			a: format!("Message #{i}"),
			b: i + 3
		});
	}

	tracing::info!("Data received correctly");

	for i in 0..10 {
		let stored = acc_p0_1
			.state()
			.get(&(i + 3))
			.expect("Expected datum to be in accumulator");

		assert_eq!(stored, &Data1 {
			a: format!("Message #{i}"),
			b: i + 3
		});
	}

	tracing::info!("Data stored correctly in producer accumulator");

	Ok(())
}

fn full_manual_disco(peers: &[&Network]) {
	for a in peers {
		for b in peers {
			if a.local().id() != b.local().id() {
				a.discovery().catalog().insert(b.local().info());
			}
		}
	}
}
