#[derive(Debug, thiserror::Error)]
pub enum Error<D> {
	/// The producer sink is closed and cannot accept new datums.
	#[error("Producer is closed")]
	Closed(Option<D>),

	/// The producer buffer is full and cannot accept new datums at this
	/// time until some of the buffered datums are consumed and sent to consumers.
	#[error("Producer is busy")]
	Full(D),

	/// The producer is offline due to not having enough active consumers
	/// that meet its minimum subscription requirement.
	///
	/// Online conditions can be configured via the
	/// [`network.streams().producer().online_when(..)`].
	///
	/// By default, producers are online when they have at least one active
	/// consumer.
	#[error("Producer is temporarily offline")]
	Offline(D),
}

impl<D: PartialEq> PartialEq for Error<D> {
	fn eq(&self, other: &Self) -> bool {
		match (self, other) {
			(Error::Closed(a), Error::Closed(b)) => a == b,
			(Error::Full(a), Error::Full(b))
			| (Error::Offline(a), Error::Offline(b)) => a == b,
			_ => false,
		}
	}
}

impl<D: Clone> Clone for Error<D> {
	fn clone(&self) -> Self {
		match self {
			Error::Closed(d) => Error::Closed(d.clone()),
			Error::Full(d) => Error::Full(d.clone()),
			Error::Offline(d) => Error::Offline(d.clone()),
		}
	}
}
