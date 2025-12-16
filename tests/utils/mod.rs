#![allow(unused)]

mod fut;
mod time;
mod tracing;

pub use {fut::*, time::*};

pub async fn discover_all(networks: &[&mosaik::Network]) -> anyhow::Result<()> {
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
