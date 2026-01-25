use {
	crate::{discovery::Discovery, network::LocalNode, store::Config},
	std::sync::Arc,
};

pub struct Handle {}

pub struct WorkerLoop {}

impl WorkerLoop {
	pub fn spawn(
		_local: LocalNode,
		_discovery: Discovery,
		_config: Config,
	) -> Arc<Handle> {
		Arc::new(Handle {})
	}
}
