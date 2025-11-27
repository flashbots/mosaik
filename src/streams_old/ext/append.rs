/// A trait for tuple appending:
/// L + R = flat tuple (L..., R)
pub trait Append<R> {
	type Out;
	fn append(self, r: R) -> Self::Out;
}

/// Base case A + B = (A, B)
impl<A, B> Append<B> for (A,) {
	type Out = (A, B);

	fn append(self, b: B) -> Self::Out {
		let (a,) = self;
		(a, b)
	}
}

macro_rules! impl_append {
  // (A,B) + C -> (A,B,C)
  (($($head:ident),+), $last:ident) => {
    impl<$($head,)+ $last> Append<$last> for ($($head,)+)
    {
      type Out = ($($head,)+ $last);

      fn append(self, last: $last) -> Self::Out {
        #[expect(non_snake_case)]
        let ($($head,)+) = self;
        ($($head,)+ last)
      }
    }
  };
}

impl_append!((A, B), C);
impl_append!((A, B, C), D);
impl_append!((A, B, C, D), E);
impl_append!((A, B, C, D, E), F);
impl_append!((A, B, C, D, E, F), G);
impl_append!((A, B, C, D, E, F, G), H);
impl_append!((A, B, C, D, E, F, G, H), I);
impl_append!((A, B, C, D, E, F, G, H, I), J);
impl_append!((A, B, C, D, E, F, G, H, I, J), K);
impl_append!((A, B, C, D, E, F, G, H, I, J, K), L);
impl_append!((A, B, C, D, E, F, G, H, I, J, K, L), M);
impl_append!((A, B, C, D, E, F, G, H, I, J, K, L, M), N);
impl_append!((A, B, C, D, E, F, G, H, I, J, K, L, M, N), O);
impl_append!((A, B, C, D, E, F, G, H, I, J, K, L, M, N, O), P);

#[cfg(test)]
mod tests {
	use super::Append;

	#[test]
	fn test_append_base_case() {
		let a = (1u32,);
		let b = 2u16;
		let result = a.append(b);
		assert_eq!(result, (1u32, 2u16));
	}

	#[test]
	fn test_append_to_2_tuple() {
		let a = (1u32, 2u16);
		let b = 3u8;
		let result = a.append(b);
		assert_eq!(result, (1u32, 2u16, 3u8));
	}

	#[test]
	fn test_append_to_7_tuple() {
		let a = (1, 2, 3, 4, 5, 6, 7);
		let b = 8;
		let result = a.append(b);
		assert_eq!(result, (1, 2, 3, 4, 5, 6, 7, 8));
	}
}
