//! Formatting utilities for primitives.

#![allow(unused)]

use {
	core::fmt,
	derive_more::{AsRef, Deref},
};

/// Trait for wrapper types that format a value.
trait FmtWrapper<T> {
	fn wrap(value: T) -> Self;
}

/// A wrapper type that pretty-prints the inner value.
#[derive(Deref, AsRef)]
pub struct Pretty<'a, T>(pub &'a T);

/// A wrapper type that formats the inner value as a shortened hex string.
pub struct Short<T>(pub T);
impl<T: AsRef<[u8]>> fmt::Display for Short<T> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let s = self.0.as_ref();
		if s.len() <= 8 {
			write!(f, "{}", hex::encode(s))
		} else {
			write!(f, "{}", hex::encode(&s[0..8]),)
		}
	}
}

impl<T> FmtWrapper<T> for Short<T> {
	fn wrap(value: T) -> Self {
		Short(value)
	}
}

/// A wrapper type that formats the inner string as an abbreviated hex string
pub struct Abbreviated<T>(pub T);
impl<T: AsRef<[u8]>> fmt::Display for Abbreviated<T> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let s = self.0.as_ref();
		if s.len() <= 16 {
			write!(f, "{}", &hex::encode(s))
		} else {
			write!(
				f,
				"{}..{}",
				&hex::encode(&s[0..8]),
				&hex::encode(&s[s.len() - 8..])
			)
		}
	}
}

impl<T> FmtWrapper<T> for Abbreviated<T> {
	fn wrap(value: T) -> Self {
		Abbreviated(value)
	}
}

/// A wrapper type that redacts the inner value when formatted.
pub struct Redacted<T>(pub T);
impl<T> fmt::Display for Redacted<T> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "<redacted>")
	}
}

impl<T> fmt::Debug for Redacted<T> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "<redacted>")
	}
}

impl<T> FmtWrapper<T> for Redacted<T> {
	fn wrap(value: T) -> Self {
		Redacted(value)
	}
}

/// A wrapper type that formats an iterator of items using a specified wrapper.
///
/// # Example
/// ```ignore
/// let hashes: Vec<[u8; 32]> = vec![...];
/// println!("{}", FmtIter::<_, Short<_>>(&hashes));
/// println!("{}", FmtIter::<_, Abbreviated<_>>(&hashes));
/// ```
pub struct FmtIter<W, I>(pub I, core::marker::PhantomData<W>);

impl<I, W> FmtIter<W, I> {
	pub fn new(iter: I) -> Self {
		FmtIter(iter, core::marker::PhantomData)
	}
}

impl<I, T, W> fmt::Display for FmtIter<W, I>
where
	I: IntoIterator<Item = T> + Clone,
	W: FmtWrapper<T> + fmt::Display,
{
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "[")?;
		for (i, item) in self.0.clone().into_iter().enumerate() {
			if i > 0 {
				write!(f, ", ")?;
			}
			write!(f, "{}", W::wrap(item))?;
		}
		write!(f, "]")
	}
}
