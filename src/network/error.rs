#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("Missing network ID")]
	MissingNetworkId,

	#[error("Bind error: {0}")]
	Bind(#[from] iroh::endpoint::BindError),
}
