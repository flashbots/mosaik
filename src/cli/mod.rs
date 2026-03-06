use clap::{Parser, Subcommand};

pub mod beacon;
pub mod dht;

#[derive(Debug, Parser)]
#[clap(version)]
pub struct CliOpts {
	#[clap(subcommand)]
	pub command: CliCommand,
}

impl CliOpts {
	pub async fn run(&self) -> anyhow::Result<()> {
		match &self.command {
			CliCommand::Dht(cmd) => cmd.run(self).await,
			CliCommand::Beacon(cmd) => cmd.run(self).await,
		}
	}
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum CliCommand {
	/// Query Mainline DHT bootstrap entries
	Dht(dht::DhtCommand),

	/// Start a Mosaik Network Bootstrap Node
	Beacon(beacon::BeaconCommand),
}
