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

/// Internal proc macro for the `collection!` macro.
///
/// This is not intended to be used directly. Use the `collection!` macro
/// from the `mosaik` crate instead.
#[proc_macro]
pub fn __collection_impl(input: TokenStream) -> TokenStream {
	let input = syn::parse_macro_input!(input as CollectionInput);
	input.expand().into()
}

enum CollectionMode {
	Full,
	ReaderOnly,
	WriterOnly,
}

struct CollectionInput {
	krate: proc_macro2::TokenStream,
	vis: syn::Visibility,
	mode: CollectionMode,
	name: syn::Ident,
	generics: syn::Generics,
	collection_type: syn::Type,
	store_id: syn::LitStr,
}

impl syn::parse::Parse for CollectionInput {
	fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
		// Parse @crate_path;
		input.parse::<syn::Token![@]>()?;
		let mut krate = proc_macro2::TokenStream::new();
		while !input.peek(syn::Token![;]) {
			let tt: proc_macro2::TokenTree = input.parse()?;
			krate.extend(core::iter::once(tt));
		}
		input.parse::<syn::Token![;]>()?;

		let vis: syn::Visibility = input.parse()?;

		// Check for reader/writer mode keyword.
		// If two consecutive identifiers appear, the first is a mode keyword.
		let mode = if input.peek(syn::Ident) && input.peek2(syn::Ident) {
			let kw: syn::Ident = input.parse()?;
			if kw == "reader" {
				CollectionMode::ReaderOnly
			} else if kw == "writer" {
				CollectionMode::WriterOnly
			} else {
				return Err(syn::Error::new(
					kw.span(),
					"expected `reader` or `writer`",
				));
			}
		} else {
			CollectionMode::Full
		};

		let name: syn::Ident = input.parse()?;
		let generics: syn::Generics = input.parse()?;
		input.parse::<syn::Token![=]>()?;
		let collection_type: syn::Type = input.parse()?;
		input.parse::<syn::Token![,]>()?;
		let store_id: syn::LitStr = input.parse()?;

		Ok(Self {
			krate,
			vis,
			mode,
			name,
			generics,
			collection_type,
			store_id,
		})
	}
}

impl CollectionInput {
	fn expand(self) -> proc_macro2::TokenStream {
		let Self {
			krate,
			vis,
			mode,
			name,
			generics,
			collection_type,
			store_id,
		} = &self;

		// Compute store ID bytes at compile time.
		let id_str = store_id.value();
		let bytes: [u8; 32] = try_decode_hex(&id_str)
			.unwrap_or_else(|| *blake3::hash(id_str.as_bytes()).as_bytes());
		let byte_lits = bytes.iter().map(|b| proc_macro2::Literal::u8_suffixed(*b));
		let store_id_expr = quote::quote! {
			#krate::collections::StoreId::from_bytes([#(#byte_lits),*])
		};

		let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

		// Build struct body: unit struct if no generics, tuple with
		// PhantomData otherwise.
		let type_params: Vec<_> =
			generics.type_params().map(|tp| &tp.ident).collect();
		let struct_body = if type_params.is_empty() {
			quote::quote! { ; }
		} else {
			let phantom = if type_params.len() == 1 {
				let p = &type_params[0];
				quote::quote! { fn() -> #p }
			} else {
				quote::quote! { fn() -> (#(#type_params),*) }
			};
			quote::quote! { (core::marker::PhantomData<#phantom>); }
		};

		let reader_impl = if matches!(mode, CollectionMode::WriterOnly) {
			quote::quote! {}
		} else {
			quote::quote! {
				const _: () = {
					impl #impl_generics #krate::collections::CollectionReader
						for #name #ty_generics #where_clause
					{
						type Reader = <#collection_type as #krate::collections::CollectionFromDef>::Reader;

						fn reader(network: &#krate::Network) -> Self::Reader {
							<#collection_type as #krate::collections::CollectionFromDef>::reader(
								network,
								#store_id_expr,
							)
						}
					}
				};
			}
		};

		let writer_impl = if matches!(mode, CollectionMode::ReaderOnly) {
			quote::quote! {}
		} else {
			quote::quote! {
				const _: () = {
					impl #impl_generics #krate::collections::CollectionWriter
						for #name #ty_generics #where_clause
					{
						type Writer = <#collection_type as #krate::collections::CollectionFromDef>::Writer;

						fn writer(network: &#krate::Network) -> Self::Writer {
							<#collection_type as #krate::collections::CollectionFromDef>::writer(
								network,
								#store_id_expr,
							)
						}
					}
				};
			}
		};

		quote::quote! {
			#[allow(non_camel_case_types)]
			#vis struct #name #generics #struct_body

			#reader_impl
			#writer_impl
		}
	}
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
