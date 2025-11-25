use super::Datum;

pub struct Accumulator<D: Datum> {
	_marker: core::marker::PhantomData<D>,
}
