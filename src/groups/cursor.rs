use {
	core::{
		cmp::Ordering,
		fmt::{self},
		ops::RangeInclusive,
	},
	derive_more::{Add, Display, From, Into, Sub},
	serde::{Deserialize, Serialize},
};

macro_rules! impl_pos_type {
	($(#[$meta:meta])* $name:ident) => {
		#[derive(
				Debug, Clone, Copy, Display, Default, PartialEq, PartialOrd,
				Eq, Ord, Hash, Serialize, Deserialize, Into, Add, Sub,
		)]
		$(#[$meta])*
		pub struct $name(pub u64);

		impl $name {
			pub const fn zero() -> Self { Self(0) }
			pub const fn one() -> Self { Self(1) }
			pub const fn is_zero(&self) -> bool { self.0 == 0 }
			#[must_use] pub const fn prev(&self) -> Self { Self(self.0.saturating_sub(1)) }
			#[must_use] pub const fn next(&self) -> Self { Self(self.0.saturating_add(1)) }
			pub fn as_usize(&self) -> usize {
				usize::try_from(self.0).expect(concat!(stringify!($name), " value overflowed usize"))
			}
			#[must_use]
			pub fn distance(&self, other: &Self) -> u64 {
				if self > other {
					self.0 - other.0
				} else {
					other.0 - self.0
				}
			}
		}

		impl From<usize> for $name {
			fn from(value: usize) -> Self {
				Self(value as u64)
			}
		}

		impl From<&usize> for $name {
			fn from(value: &usize) -> Self {
				Self(*value as u64)
			}
		}

		impl core::ops::Add<usize> for $name {
			type Output = Self;
			fn add(self, rhs: usize) -> Self {
				Self(self.0 + rhs as u64)
			}
		}

		impl core::ops::Sub<usize> for $name {
			type Output = Self;
			fn sub(self, rhs: usize) -> Self {
				Self(self.0.saturating_sub(rhs as u64))
			}
		}

		impl From<$name> for usize {
			fn from(value: $name) -> Self {
				usize::try_from(value.0)
					.expect(concat!(stringify!($name), " value overflowed usize"))
			}
		}

		impl From<&$name> for usize {
			fn from(value: &$name) -> Self {
				usize::try_from(value.0)
					.expect(concat!(stringify!($name), " value overflowed usize"))
			}
		}

		impl core::fmt::Display for $crate::primitives::Pretty<'_, core::ops::RangeInclusive<$name>> {
			fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
				write!(f, "[{}..{}]", self.start(), self.end())
			}
		}

		impl_pos_type!(@unsigned $name: u8, u16, u32, u64);
    impl_pos_type!(@signed $name: i8, i16, i32, i64);
	};

	(@unsigned $name:ident: $($t:ty),+) => {
		$(
			const _: () = {
				#[automatically_derived]
				impl From<$t> for $name {
					fn from(value: $t) -> Self {
						Self(u64::from(value))
					}
				}

				#[automatically_derived]
				impl From<&$t> for $name {
					fn from(value: &$t) -> Self {
						Self(u64::from(*value))
					}
				}

				#[automatically_derived]
				impl PartialEq<$t> for $name {
					fn eq(&self, other: &$t) -> bool {
						self.0 == u64::from(*other)
					}
				}

				#[automatically_derived]
				impl PartialEq<$t> for &$name {
					fn eq(&self, other: &$t) -> bool {
						self.0 == u64::from(*other)
					}
				}

				#[automatically_derived]
				impl core::ops::Add<$t> for $name {
					type Output = Self;
					fn add(self, rhs: $t) -> $name {
						$name(self.0 + u64::from(rhs))
					}
				}

				#[automatically_derived]
				impl core::ops::Sub<$t> for $name {
					type Output = Self;
					fn sub(self, rhs: $t) -> $name {
						$name(self.0.saturating_sub(u64::from(rhs)))
					}
				}

				#[automatically_derived]
				impl core::ops::Add<&$t> for $name {
					type Output = Self;
					fn add(self, rhs: &$t) -> $name {
						$name(self.0 + u64::from(*rhs))
					}
				}

				#[automatically_derived]
				impl PartialEq<&$t> for $name {
					fn eq(&self, other: &&$t) -> bool {
						self.0 == u64::from(**other)
					}
				}

				#[automatically_derived]
				impl PartialOrd<$t> for $name {
					fn partial_cmp(&self, other: &$t) -> Option<Ordering> {
						self.0.partial_cmp(&u64::from(*other))
					}
				}

				#[automatically_derived]
				impl PartialOrd<$t> for &$name {
					fn partial_cmp(&self, other: &$t) -> Option<Ordering> {
						self.0.partial_cmp(&u64::from(*other))
					}
				}

				#[automatically_derived]
				impl PartialOrd<&$t> for $name {
					fn partial_cmp(&self, other: &&$t) -> Option<Ordering> {
						self.0.partial_cmp(&u64::from(**other))
					}
				}
			};
		)+
    };

    (@signed $name:ident: $($t:ty),+) => {
			$(
				const _: () = {
					#[automatically_derived]
					#[allow(clippy::cast_sign_loss)]
					impl From<$t> for $name {
						fn from(value: $t) -> Self {
							if value < 0 { Self(0) }
							else { Self(value as u64) }
						}
					}

					#[automatically_derived]
					#[allow(clippy::cast_sign_loss)]
					impl From<&$t> for $name {
						fn from(value: &$t) -> Self {
							if *value < 0 { Self(0) }
							else { Self(*value as u64) }
						}
					}

					#[automatically_derived]
					impl PartialEq<$t> for $name {
						#[allow(clippy::cast_sign_loss)]
						fn eq(&self, other: &$t) -> bool {
							let other = i64::from(*other);
							if other < 0 { false}
							else { self.0 == other as u64 }
						}
					}

					#[automatically_derived]
					impl PartialEq<$t> for &$name {
						#[allow(clippy::cast_sign_loss)]
						fn eq(&self, other: &$t) -> bool {
							let other = i64::from(*other);
							if other < 0 { false}
							else { self.0 == other as u64 }
						}
					}

					#[automatically_derived]
					impl PartialEq<&$t> for $name {
						#[allow(clippy::cast_sign_loss)]
						fn eq(&self, other: &&$t) -> bool {
							let other = i64::from(**other);
							if other < 0 { false}
							else { self.0 == other as u64 }
						}
					}

					#[automatically_derived]
					impl PartialOrd<$t> for $name {
						#[allow(clippy::cast_sign_loss)]
						fn partial_cmp(&self, other: &$t) -> Option<Ordering> {
							let other = i64::from(*other);
							if other < 0 {
								// self (u64) is always >= 0, so self > negative
								Some(Ordering::Greater)
							} else {
								self.0.partial_cmp(&(other as u64))
							}
						}
					}

					#[automatically_derived]
					impl PartialOrd<$t> for &$name {
						#[allow(clippy::cast_sign_loss)]
						fn partial_cmp(&self, other: &$t) -> Option<Ordering> {
							let other = i64::from(*other);
							if other < 0 {
								// self (u64) is always >= 0, so self > negative
								Some(Ordering::Greater)
							} else {
								self.0.partial_cmp(&(other as u64))
							}
						}
					}

					#[automatically_derived]
					impl PartialOrd<&$t> for $name {
						#[allow(clippy::cast_sign_loss)]
						fn partial_cmp(&self, other: &&$t) -> Option<Ordering> {
							let other = i64::from(**other);
							if other < 0 {
								// self (u64) is always >= 0, so self > negative
								Some(Ordering::Greater)
							} else {
								self.0.partial_cmp(&(other as u64))
							}
						}
					}
				};
			)+
    };
}

impl_pos_type!(
	/// Raft term, increases monotonically with every new leader election.
	Term);

impl_pos_type!(
	/// Raft log index increases monotonically with every new log entry.
	Index);

/// Range of log indices.
///
/// We always use inclusive ranges for log indices.
pub type IndexRange = RangeInclusive<Index>;

/// Progress of the log.
#[derive(
	Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, From, Into,
)]
pub struct Cursor(pub Term, pub Index);

impl Default for Cursor {
	fn default() -> Self {
		Self::zero()
	}
}

impl Cursor {
	pub const fn new(term: Term, index: Index) -> Self {
		Self(term, index)
	}

	pub const fn zero() -> Self {
		Self(Term::zero(), Index::zero())
	}

	pub const fn is_zero(&self) -> bool {
		self.term().is_zero() && self.index().is_zero()
	}

	pub const fn term(&self) -> Term {
		self.0
	}

	pub const fn index(&self) -> Index {
		self.1
	}

	pub fn is_behind(&self, other: &Self) -> bool {
		self.cmp(other) == Ordering::Less
	}
}

impl PartialOrd for Cursor {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

impl PartialEq<Index> for Cursor {
	fn eq(&self, other: &Index) -> bool {
		self.index() == *other
	}
}

impl PartialEq<Term> for Cursor {
	fn eq(&self, other: &Term) -> bool {
		self.term() == *other
	}
}

impl PartialOrd<Index> for Cursor {
	fn partial_cmp(&self, other: &Index) -> Option<Ordering> {
		Some(self.index().cmp(other))
	}
}

impl PartialOrd<Term> for Cursor {
	fn partial_cmp(&self, other: &Term) -> Option<Ordering> {
		Some(self.term().cmp(other))
	}
}

impl PartialEq<Cursor> for Index {
	fn eq(&self, other: &Cursor) -> bool {
		*self == other.index()
	}
}

impl PartialOrd<Cursor> for Index {
	fn partial_cmp(&self, other: &Cursor) -> Option<Ordering> {
		Some(self.cmp(&other.index()))
	}
}

impl Ord for Cursor {
	fn cmp(&self, other: &Self) -> Ordering {
		self
			.term()
			.cmp(&other.term())
			.then(self.index().cmp(&other.index()))
	}
}

impl fmt::Debug for Cursor {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "Cursor(t:{},i:{})", self.term(), self.index())
	}
}

impl fmt::Display for Cursor {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "(t:{},i:{})", self.term(), self.index())
	}
}
