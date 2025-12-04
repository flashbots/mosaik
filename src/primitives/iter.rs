use super::Variant;

/// A Quality of Life helper trait to accept either a single item or an iterator
/// of items.
pub trait IntoIterOrSingle<T, V = Variant<0>> {
	fn iterator(self) -> impl IntoIterator<Item = T>;
}

impl<T, U: Into<T>> IntoIterOrSingle<T, Variant<0>> for U {
	fn iterator(self) -> impl IntoIterator<Item = T> {
		std::iter::once(self.into())
	}
}

impl<T, U, I> IntoIterOrSingle<T, Variant<1>> for I
where
	U: Into<T>,
	I: IntoIterator<Item = U>,
{
	fn iterator(self) -> impl IntoIterator<Item = T> {
		self.into_iter().map(Into::into)
	}
}
