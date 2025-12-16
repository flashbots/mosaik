#![allow(unused)]

mod fut;
mod time;
mod tracing;

pub use {fut::*, time::*};

pub async fn discover_all(
	networks: impl IntoIterator<Item = &mosaik::Network>,
) -> anyhow::Result<()> {
	let networks = networks.into_iter().collect::<Vec<_>>();
	for i in 0..networks.len() {
		for j in 0..networks.len() {
			if i != j {
				networks[i]
					.discovery()
					.sync_with(networks[j].local().id())
					.await?;
			}
		}
	}
	Ok(())
}
