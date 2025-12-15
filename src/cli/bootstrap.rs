use {super::args::CliOpts, clap::Args, mosaik::NetworkId, tracing::info};

#[derive(Args, Debug)]
pub struct Command {
	/// Network ID to connect to
	#[clap(short, long)]
	pub network: NetworkId,
}

impl Command {
	pub async fn execute(&self, _opts: &CliOpts) -> anyhow::Result<()> {
		info!("Starting bootstrap node on network {:?}", self.network);
		async {}.await;
		Ok(())
	}
}
