use tokio::task::JoinError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("task join error: {0}")]
	JoinError(#[from] JoinError),

	#[error("endpoint bind error: {0}")]
	Bind(#[from] iroh::endpoint::BindError),

	#[error("Gossip protocol error: {0}")]
	Gossip(#[from] iroh_gossip::api::ApiError),

	#[error("network runloop is terminated")]
	RunloopTerminated,
}
