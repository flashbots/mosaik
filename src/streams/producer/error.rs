#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("I/O error: {0}")]
	Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ProducerError<D> {
	#[error("Producer is closed")]
	Closed(Option<D>),

	/// No active consumers to send the datum to
	/// or none of the active consumers subscriptions match
	/// the datum criteria.
	#[error("No active consumers for stream")]
	NoConsumers(D),
}
