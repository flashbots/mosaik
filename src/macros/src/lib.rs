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
	attrs: Vec<syn::Attribute>,
	vis: syn::Visibility,
	mode: CollectionMode,
	name: syn::Ident,
	generics: syn::Generics,
	collection_type: syn::Type,
	store_id: StoreIdInput,
}

enum StoreIdInput {
	/// A string literal — hashed at compile time by the proc macro.
	Literal(syn::LitStr),
	/// An expression (e.g. a constant path like `MY_STORE_ID`).
	Expr(syn::Expr),
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

		// Parse optional outer attributes (e.g. #[doc = "..."]).
		let attrs = input.call(syn::Attribute::parse_outer)?;

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
		let store_id = if input.peek(syn::LitStr) {
			StoreIdInput::Literal(input.parse()?)
		} else {
			StoreIdInput::Expr(input.parse()?)
		};

		Ok(Self {
			krate,
			attrs,
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
			attrs,
			vis,
			mode,
			name,
			generics,
			collection_type,
			store_id,
		} = &self;

		let store_id_expr = match store_id {
			StoreIdInput::Literal(lit) => {
				// Compute store ID bytes at compile time.
				let id_str = lit.value();
				let bytes: [u8; 32] = try_decode_hex(&id_str)
					.unwrap_or_else(|| *blake3::hash(id_str.as_bytes()).as_bytes());
				let byte_lits =
					bytes.iter().map(|b| proc_macro2::Literal::u8_suffixed(*b));
				quote::quote! {
					#krate::collections::StoreId::from_bytes([#(#byte_lits),*])
				}
			}
			StoreIdInput::Expr(expr) => {
				quote::quote! { #expr }
			}
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
			#(#attrs)*
			#[allow(non_camel_case_types)]
			#vis struct #name #generics #struct_body

			#reader_impl
			#writer_impl
		}
	}
}

/// Internal proc macro for the `stream!` macro.
///
/// This is not intended to be used directly. Use the `stream!` macro
/// from the `mosaik` crate instead.
#[proc_macro]
pub fn __stream_impl(input: TokenStream) -> TokenStream {
	let input = syn::parse_macro_input!(input as StreamInput);
	input.expand().into()
}

enum StreamMode {
	Full,
	ProducerOnly,
	ConsumerOnly,
}

/// Which side a config entry targets.
enum ConfigSide {
	/// Inferred from the key name or applied to both.
	Inferred,
	Producer,
	Consumer,
}

struct StreamConfigEntry {
	side: ConfigSide,
	key: syn::Ident,
	value: syn::Expr,
}

struct StreamInput {
	krate: proc_macro2::TokenStream,
	attrs: Vec<syn::Attribute>,
	vis: syn::Visibility,
	mode: StreamMode,
	name: syn::Ident,
	generics: syn::Generics,
	datum_type: syn::Type,
	stream_id: Option<StreamIdInput>,
	config: Vec<StreamConfigEntry>,
}

enum StreamIdInput {
	/// A string literal — hashed at compile time by the proc macro.
	Literal(syn::LitStr),
	/// An expression (e.g. a constant path like `MY_STREAM_ID`).
	Expr(syn::Expr),
}

impl syn::parse::Parse for StreamInput {
	fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
		// Parse @crate_path;
		input.parse::<syn::Token![@]>()?;
		let mut krate = proc_macro2::TokenStream::new();
		while !input.peek(syn::Token![;]) {
			let tt: proc_macro2::TokenTree = input.parse()?;
			krate.extend(core::iter::once(tt));
		}
		input.parse::<syn::Token![;]>()?;

		// Parse optional outer attributes (e.g. #[doc = "..."]).
		let attrs = input.call(syn::Attribute::parse_outer)?;

		let vis: syn::Visibility = input.parse()?;

		// Check for producer/consumer mode keyword.
		let mode = if input.peek(syn::Ident) && input.peek2(syn::Ident) {
			let kw: syn::Ident = input.fork().parse()?;
			if kw == "producer" {
				input.parse::<syn::Ident>()?;
				StreamMode::ProducerOnly
			} else if kw == "consumer" {
				input.parse::<syn::Ident>()?;
				StreamMode::ConsumerOnly
			} else {
				StreamMode::Full
			}
		} else {
			StreamMode::Full
		};

		let name: syn::Ident = input.parse()?;
		let generics: syn::Generics = input.parse()?;
		input.parse::<syn::Token![=]>()?;
		let datum_type: syn::Type = input.parse()?;

		// Parse optional stream id and config entries.
		let mut stream_id = None;
		let mut config = Vec::new();

		if input.peek(syn::Token![,]) {
			input.parse::<syn::Token![,]>()?;

			// Determine if the next tokens are a stream id or a config
			// entry. Config entries are `ident :` (but not `ident ::`),
			// or side-prefixed `producer/consumer ident :`. String
			// literals are always stream ids. Anything else that is not
			// a config entry is parsed as an expression stream id.
			let is_config_start = if input.peek(syn::Ident)
				&& input.peek2(syn::Token![:])
				&& !input.peek2(syn::Token![::])
			{
				true
			} else if input.peek(syn::Ident) && input.peek2(syn::Ident) {
				// Check for side-prefixed config: `producer ident :`
				let fork = input.fork();
				let prefix: syn::Ident = fork.parse().unwrap();
				(prefix == "producer" || prefix == "consumer")
					&& fork.peek(syn::Ident)
					&& fork.peek2(syn::Token![:])
					&& !fork.peek2(syn::Token![::])
			} else {
				false
			};

			if input.peek(syn::LitStr) {
				stream_id = Some(StreamIdInput::Literal(input.parse()?));

				// Consume trailing comma if present.
				if input.peek(syn::Token![,]) {
					input.parse::<syn::Token![,]>()?;
				}
			} else if !is_config_start && !input.is_empty() {
				stream_id = Some(StreamIdInput::Expr(input.parse()?));

				// Consume trailing comma if present.
				if input.peek(syn::Token![,]) {
					input.parse::<syn::Token![,]>()?;
				}
			}

			// Parse config entries: [producer|consumer] key: expr
			while input.peek(syn::Ident) {
				// Check for optional side prefix.
				let side = if input.peek(syn::Ident) && input.peek2(syn::Ident) {
					let fork = input.fork();
					let prefix: syn::Ident = fork.parse()?;
					if (prefix == "producer" || prefix == "consumer")
						&& !fork.peek(syn::Token![=])
						&& !fork.peek(syn::Token![:])
					{
						let prefix: syn::Ident = input.parse()?;
						if prefix == "producer" {
							ConfigSide::Producer
						} else {
							ConfigSide::Consumer
						}
					} else {
						ConfigSide::Inferred
					}
				} else {
					ConfigSide::Inferred
				};

				let key: syn::Ident = input.parse()?;
				input.parse::<syn::Token![:]>()?;
				let value: syn::Expr = input.parse()?;

				config.push(StreamConfigEntry { side, key, value });

				// Consume trailing comma if present.
				if input.peek(syn::Token![,]) {
					input.parse::<syn::Token![,]>()?;
				}
			}
		}

		Ok(Self {
			krate,
			attrs,
			vis,
			mode,
			name,
			generics,
			datum_type,
			stream_id,
			config,
		})
	}
}

impl StreamInput {
	#[allow(clippy::too_many_lines)]
	fn expand(self) -> proc_macro2::TokenStream {
		let Self {
			krate,
			attrs,
			vis,
			mode,
			name,
			generics,
			datum_type,
			stream_id,
			config,
		} = &self;

		// Build optional .with_stream_id(...) call.
		let stream_id_call = match stream_id {
			Some(StreamIdInput::Literal(lit)) => {
				let id_str = lit.value();
				let bytes: [u8; 32] = try_decode_hex(&id_str)
					.unwrap_or_else(|| *blake3::hash(id_str.as_bytes()).as_bytes());
				let byte_lits =
					bytes.iter().map(|b| proc_macro2::Literal::u8_suffixed(*b));
				Some(quote::quote! {
					.with_stream_id(
						#krate::StreamId::from_bytes(
							[#(#byte_lits),*]
						)
					)
				})
			}
			Some(StreamIdInput::Expr(expr)) => Some(quote::quote! {
				.with_stream_id(#expr)
			}),
			None => None,
		};

		// Partition config entries into producer and consumer calls.
		let mut producer_calls = Vec::new();
		let mut consumer_calls = Vec::new();

		for entry in config {
			let key = &entry.key;
			let value = &entry.value;
			let key_str = key.to_string();

			let call = match key_str.as_str() {
				"accept_if" => quote::quote! { .accept_if(#value) },
				"online_when" => {
					quote::quote! { .online_when(#value) }
				}
				"subscribe_if" => {
					quote::quote! { .subscribe_if(#value) }
				}
				"max_consumers" => {
					quote::quote! { .with_max_consumers(#value) }
				}
				"buffer_size" => {
					quote::quote! { .with_buffer_size(#value) }
				}
				"disconnect_lagging" => {
					quote::quote! { .disconnect_lagging(#value) }
				}
				"criteria" => {
					quote::quote! { .with_criteria(#value) }
				}
				"backoff" => {
					quote::quote! { .with_backoff(#value) }
				}
				_ => {
					return syn::Error::new(
						key.span(),
						format!("unknown stream config key `{key_str}`"),
					)
					.to_compile_error();
				}
			};

			// Route based on explicit side or infer from key.
			match &entry.side {
				ConfigSide::Producer => {
					producer_calls.push(call);
				}
				ConfigSide::Consumer => {
					consumer_calls.push(call);
				}
				ConfigSide::Inferred => {
					match key_str.as_str() {
						// Producer-only keys.
						"accept_if" | "max_consumers" | "buffer_size"
						| "disconnect_lagging" => {
							producer_calls.push(call);
						}
						// Consumer-only keys.
						"subscribe_if" | "criteria" | "backoff" => {
							consumer_calls.push(call);
						}
						// Ambiguous keys apply to both.
						"online_when" => {
							producer_calls.push(call.clone());
							consumer_calls.push(call);
						}
						_ => unreachable!(),
					}
				}
			}
		}

		let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

		// Build struct body.
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
			quote::quote! {
				(core::marker::PhantomData<#phantom>);
			}
		};

		let producer_impl = if matches!(mode, StreamMode::ConsumerOnly) {
			quote::quote! {}
		} else {
			quote::quote! {
				const _: () = {
					impl #impl_generics
						#krate::streams::StreamProducer
						for #name #ty_generics #where_clause
					{
						type Producer =
							#krate::streams::Producer<#datum_type>;

						fn producer(
							network: &#krate::Network,
						) -> Self::Producer {
							match network
								.streams()
								.producer::<#datum_type>()
								#stream_id_call
								#(#producer_calls)*
								.build()
							{
								Ok(p) => p,
								Err(
									#krate::streams::producer
										::BuilderError
										::AlreadyExists(p),
								) => p,
							}
						}
					}
				};
			}
		};

		let consumer_impl = if matches!(mode, StreamMode::ProducerOnly) {
			quote::quote! {}
		} else {
			quote::quote! {
				const _: () = {
					impl #impl_generics
						#krate::streams::StreamConsumer
						for #name #ty_generics #where_clause
					{
						type Consumer =
							#krate::streams::Consumer<#datum_type>;

						fn consumer(
							network: &#krate::Network,
						) -> Self::Consumer {
							network
								.streams()
								.consumer::<#datum_type>()
								#stream_id_call
								#(#consumer_calls)*
								.build()
						}
					}
				};
			}
		};

		quote::quote! {
			#(#attrs)*
			#[allow(non_camel_case_types)]
			#vis struct #name #generics #struct_body

			#producer_impl
			#consumer_impl
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
