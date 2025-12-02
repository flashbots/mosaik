#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("I/O error: {0}")]
	Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum SendError<D> {
	#[error("Producer is closed")]
	Closed(Option<D>),
}
