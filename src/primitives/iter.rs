use super::Variant;

/// A Quality of Life helper trait to accept either a single item or an iterator
/// of items.
pub trait IntoIterOrSingle<T, V = Variant<0>> {
	fn iterator(self) -> impl IntoIterator<Item = T>;
}

impl<T> IntoIterOrSingle<T, Variant<0>> for T {
	fn iterator(self) -> impl IntoIterator<Item = T> {
		std::iter::once(self)
	}
}

impl<T, I: IntoIterator<Item = T>> IntoIterOrSingle<T, Variant<2>> for I {
	fn iterator(self) -> impl IntoIterator<Item = T> {
		self
	}
}
