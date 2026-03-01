use {clap::Parser, tracing_subscriber::EnvFilter};

mod cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	tracing_subscriber::fmt()
		.with_env_filter(EnvFilter::from_default_env())
		.with_writer(std::io::stderr)
		.init();

	let opts = cli::CliOpts::parse();
	tracing::debug!("CLI options: {opts:#?}");

	opts.run().await
}
