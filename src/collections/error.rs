#[derive(Debug, thiserror::Error)]
pub enum InsertError<V> {
	#[error("Offline")]
	Offline(V),

	#[error("Network is down")]
	NetworkDown,
}

#[derive(Debug, thiserror::Error)]
pub enum InsertManyError<V> {
	#[error("Offline")]
	Offline(Vec<V>),

	#[error("Network is down")]
	NetworkDown,
}

#[derive(Debug, thiserror::Error)]
pub enum RemoveError<K> {
	#[error("Offline")]
	Offline(K),

	#[error("Network is down")]
	NetworkDown,
}
