use {
	super::args::CliOpts,
	tracing_subscriber::{filter::filter_fn, prelude::*},
};

/// Sets up tracing/logging for the CLI based on verbosity level.
pub fn setup(opts: &CliOpts) {
	// Determine the log level based on verbosity
	let log_level = match opts.verbose {
		0 => tracing::Level::INFO,
		1 => tracing::Level::DEBUG,
		_ => tracing::Level::TRACE,
	};

	// disable noisy modules from dependencies
	let muted: &[&'static str] = &[
		"iroh",
		"rustls",
		"igd_next",
		"hickory_",
		"hyper_util",
		"portmapper",
		"reqwest",
		"events.net.relay.connected",
		"netwatch",
		"mio",
		"acto",
		"swarm_discovery",
	];

	// optionally disable muting for noisy modules
	let unmute = std::env::var("MOSAIK_NOISY_TRACE")
		.map(|val| val == "1" || val == "on" || val == "yes")
		.unwrap_or(false);

	let _ = tracing_subscriber::registry()
		.with(tracing_subscriber::fmt::layer())
		.with(filter_fn(move |metadata| {
			metadata.level() <= &log_level
				&& (unmute
					|| !muted
						.iter()
						.any(|prefix| metadata.target().starts_with(prefix)))
		}))
		.try_init();
}
