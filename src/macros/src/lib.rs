use proc_macro::TokenStream;

/// Internal proc macro that computes `UniqueId` bytes at compile time.
///
/// Takes a string literal and returns a `[u8; 32]` array expression:
/// - If the string is exactly 64 hex characters, it is decoded directly.
/// - Otherwise, the string is hashed with blake3.
///
/// This is not intended to be used directly. Use the `unique_id!` macro
/// from the `mosaik` crate instead.
#[proc_macro]
pub fn __unique_id_impl(input: TokenStream) -> TokenStream {
	let lit: syn::LitStr = syn::parse_macro_input!(input as syn::LitStr);
	let value = lit.value();

	let bytes: [u8; 32] = try_decode_hex(&value)
		.unwrap_or_else(|| *blake3::hash(value.as_bytes()).as_bytes());

	let byte_literals = bytes.iter().map(|b| {
		let b = proc_macro2::Literal::u8_suffixed(*b);
		quote::quote! { #b }
	});

	let expanded = quote::quote! {
		[#(#byte_literals),*]
	};

	expanded.into()
}

/// Attempts to decode a hex string into exactly 32 bytes.
/// Returns `None` if the string is not valid 64-character hex.
fn try_decode_hex(s: &str) -> Option<[u8; 32]> {
	if s.len() != 64 {
		return None;
	}

	let mut bytes = [0u8; 32];
	for (i, byte) in bytes.iter_mut().enumerate() {
		let high = hex_digit(s.as_bytes()[i * 2])?;
		let low = hex_digit(s.as_bytes()[i * 2 + 1])?;
		*byte = (high << 4) | low;
	}
	Some(bytes)
}

const fn hex_digit(c: u8) -> Option<u8> {
	match c {
		b'0'..=b'9' => Some(c - b'0'),
		b'a'..=b'f' => Some(c - b'a' + 10),
		b'A'..=b'F' => Some(c - b'A' + 10),
		_ => None,
	}
}
