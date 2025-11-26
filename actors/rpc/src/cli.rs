use shared::cli::CliNetOpts;

#[derive(Debug, Clone, clap::Parser)]
pub struct Opts {
	#[clap(flatten)]
	pub network: CliNetOpts,

	/// HTTP JSON-RPC endpoint listen address.
	#[clap(long, default_value = "0.0.0.0:8545")]
	pub listen_addr: String,
}

#[allow(clippy::derivable_impls)]
impl Default for Opts {
	fn default() -> Self {
		let opts = Opts {
			network: CliNetOpts::default(),
			..clap::Parser::parse()
		};

		opts
	}
}

fn default_work_dir() -> String {
	let path_suffix = format!("rpc-{}", nanoid::nanoid!(6));
	let tempdir = std::env::temp_dir().join(path_suffix);
	tempdir.to_str().expect("invalid temp dir path").to_string()
}
