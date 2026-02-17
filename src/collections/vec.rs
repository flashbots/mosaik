use {
	super::{InsertError, InsertManyError, RemoveError, StoreId, Version, When},
	crate::{Datum, Network},
	core::marker::PhantomData,
};

pub struct Vec<T: Datum> {
	when: When,
	_p: PhantomData<T>,
}

impl<T: Datum> Vec<T> {
	pub const fn writer(
		_network: &crate::network::Network,
		_store_id: crate::UniqueId,
	) -> Self {
		Self {
			when: todo!(),
			_p: PhantomData,
		}
	}

	pub const fn reader(_network: &Network, _store_id: StoreId) -> VecReader<T> {
		VecReader {
			when: todo!(),
			_p: PhantomData,
		}
	}

	pub async fn push(&self, value: T) -> Result<Version, InsertError<T>> {
		async { Err(InsertError::Offline(value)) }.await
	}

	pub async fn extend(
		&self,
		entries: impl IntoIterator<Item = T>,
	) -> Result<Version, InsertManyError<T>> {
		async { Err(InsertManyError::Offline(entries.into_iter().collect())) }.await
	}

	pub async fn remove(&self, index: u64) -> Result<Version, RemoveError<u64>> {
		async { Err(RemoveError::Offline(index)) }.await
	}

	pub const fn when(&self) -> &When {
		&self.when
	}
}

pub struct VecReader<T: Datum> {
	when: When,
	_p: PhantomData<T>,
}

impl<T: Datum> VecReader<T> {
	pub const fn get(&self, _index: u64) -> Option<T> {
		let _ = self;
		None
	}

	pub fn version(&self) -> Version {
		todo!()
	}

	pub const fn when(&self) -> &When {
		&self.when
	}
}
