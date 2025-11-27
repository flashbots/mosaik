#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("Bind error: {0}")]
	Bind(#[from] iroh::endpoint::BindError),
}
