use {
	super::{InsertError, InsertManyError, RemoveError, StoreId, Version, When},
	crate::{Datum, Network},
	core::marker::PhantomData,
};

pub struct Set<T: Datum> {
	when: When,
	_p: PhantomData<T>,
}

impl<T: Datum> Set<T> {
	pub const fn writer(
		_network: &crate::network::Network,
		_store_id: crate::UniqueId,
	) -> Self {
		Self {
			when: todo!(),
			_p: PhantomData,
		}
	}

	pub const fn reader(_network: &Network, _store_id: StoreId) -> SetReader<T> {
		SetReader {
			when: todo!(),
			_p: PhantomData,
		}
	}

	pub async fn insert(&self, value: T) -> Result<Version, InsertError<T>> {
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

	pub fn contains(&self, _value: &T) -> bool {
		todo!()
	}

	pub const fn when(&self) -> &When {
		&self.when
	}
}

pub struct SetReader<T: Datum> {
	when: When,
	_p: PhantomData<T>,
}

impl<T: Datum> SetReader<T> {
	pub const fn contains(&self, _value: &T) -> bool {
		let _ = self;
		false
	}

	pub fn version(&self) -> Version {
		todo!()
	}

	pub const fn when(&self) -> &When {
		&self.when
	}
}
