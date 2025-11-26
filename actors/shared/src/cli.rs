use {
	clap::ArgAction,
	mosaik::prelude::*,
	tracing::Level,
	tracing_subscriber::{
		Layer,
		filter::filter_fn,
		layer::SubscriberExt,
		util::SubscriberInitExt,
	},
};

#[derive(clap::Parser, Clone, Debug)]
pub struct CliNetOpts {
	/// Network ID to connect to.
	#[clap(short, long, default_value = "devnet-1")]
	pub network_id: NetworkId,

	/// Enable Optimism L2, otherwise Ethereum L1 is used.
	#[clap(long)]
	pub optimism: bool,

	/// Optional bootstrap peer ids to connect to.
	#[clap(long, short = 'p')]
	pub bootstrap: Vec<EndpointId>,

	/// Optional secret key for node identity.
	/// If not provided, a random identity will be generated.
	#[clap(long, short)]
	pub secret_key: Option<SecretKey>,

	#[clap(
		short,
		action = ArgAction::Count,
		env = "QUARK_LOGGING",
		help = "Use verbose output (-vv very verbose)",
		global = true
	)]
	pub verbose: u8,
}

impl Default for CliNetOpts {
	fn default() -> Self {
		let opts: Self = clap::Parser::parse();
		opts.configure_logging();
		opts
	}
}

impl CliNetOpts {
	pub fn configure_logging(&self) {
		let level = match self.verbose {
			1 => Level::DEBUG,
			2 => Level::TRACE,
			_ => Level::INFO,
		};

		let prefix_blacklist: &[&'static str] = &["netlink_packet_route"];

		tracing_subscriber::registry()
			.with(
				tracing_subscriber::fmt::layer()
					.with_writer(std::io::stdout)
					.with_filter(filter_fn(move |metadata| {
						!prefix_blacklist
							.iter()
							.any(|prefix| metadata.target().starts_with(prefix))
							&& metadata.level() <= &level
					})),
			)
			.init();
	}

	pub fn mode_name(&self) -> &'static str {
		if self.optimism {
			"Optimism"
		} else {
			"Ethereum Mainnet"
		}
	}
}
