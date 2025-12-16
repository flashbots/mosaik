#[derive(Debug, thiserror::Error)]
pub enum Error<D> {
	#[error("Producer is closed")]
	Closed(Option<D>),

	/// No active consumers to send the datum to
	/// or none of the active consumers subscriptions match
	/// the datum criteria.
	#[error("No active consumers for stream")]
	NoConsumers(D),
}
