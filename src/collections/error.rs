#[derive(Debug, thiserror::Error)]
pub enum InsertError<V> {
	#[error("Offline")]
	Offline(V),
}

#[derive(Debug, thiserror::Error)]
pub enum InsertManyError<V> {
	#[error("Offline")]
	Offline(Vec<V>),
}

#[derive(Debug, thiserror::Error)]
pub enum RemoveError<K> {
	#[error("Offline")]
	Offline(K),
}
