#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("stream consumer has been terminated")]
	Terminated,
}
