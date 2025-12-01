use super::Datum;

pub struct Producer<D: Datum>(pub(crate) std::marker::PhantomData<D>);
