#[ctor::ctor]
fn init_test_logging() {
	use tracing_subscriber::{filter::filter_fn, prelude::*};

	// Avoid re-initializing if a global default subscriber is already set
	if tracing::dispatcher::has_been_set() {
		return;
	}

	if let Ok(v) = std::env::var("TEST_TRACE") {
		let level = match v.as_str() {
			"true" | "debug" | "on" => tracing::Level::DEBUG,
			"trace" => tracing::Level::TRACE,
			"info" => tracing::Level::INFO,
			"warn" => tracing::Level::WARN,
			"error" => tracing::Level::ERROR,
			_ => return,
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

		// disable disabling noisy modules tracing
		let unmute = std::env::var("TEST_TRACE_UNMUTE")
			.map(|val| val == "1" || val == "on" || val == "yes")
			.unwrap_or(false);

		let _ = tracing_subscriber::registry()
			.with(tracing_subscriber::fmt::layer())
			.with(filter_fn(move |metadata| {
				metadata.level() <= &level
					&& (unmute
						|| !muted
							.iter()
							.any(|prefix| metadata.target().starts_with(prefix)))
			}))
			.try_init();

		// Set a custom panic hook that prints and aborts
		let default_hook = std::panic::take_hook();
		std::panic::set_hook(Box::new(move |panic_info| {
			default_hook(panic_info);
			tracing::error!("panic: {panic_info}");
			std::process::abort();
		}));
	}
}
