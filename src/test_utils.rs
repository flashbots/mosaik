#[cfg(feature = "test-utils")]
#[ctor::ctor]
fn init_test_logging() {
	use tracing_subscriber::{filter::filter_fn, prelude::*};
	if let Ok(v) = std::env::var("TEST_TRACE") {
		let level = match v.as_str() {
			"true" | "debug" | "on" => tracing::Level::DEBUG,
			"trace" => tracing::Level::TRACE,
			"info" => tracing::Level::INFO,
			"warn" => tracing::Level::WARN,
			"error" => tracing::Level::ERROR,
			_ => return,
		};

		let prefix_blacklist: &[&'static str] = &[];

		tracing_subscriber::registry()
			.with(tracing_subscriber::fmt::layer())
			.with(filter_fn(move |metadata| {
				metadata.level() <= &level
					&& !prefix_blacklist
						.iter()
						.any(|prefix| metadata.target().starts_with(prefix))
			}))
			.init();

		std::panic::set_hook(Box::new(|panic_info| {
			tracing::error!("Fatal panic: {panic_info:?}");
			std::process::exit(-1);
		}));
	}
}
