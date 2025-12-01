use super::Datum;

mod builder;
mod status;

/// Public API
pub use {builder::Builder, status::Status};

pub struct Consumer<D: Datum> {
	_marker: std::marker::PhantomData<D>,
}
