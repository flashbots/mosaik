use super::try_decode_hex;

enum CollectionMode {
	Full,
	ReaderOnly,
	WriterOnly,
}

struct CollectionConfigEntry {
	key: syn::Ident,
	value: syn::Expr,
}

pub struct CollectionInput {
	krate: proc_macro2::TokenStream,
	attrs: Vec<syn::Attribute>,
	vis: syn::Visibility,
	mode: CollectionMode,
	name: syn::Ident,
	generics: syn::Generics,
	collection_type: syn::Type,
	store_id: StoreIdInput,
	config: Vec<CollectionConfigEntry>,
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

		// Parse optional config entries: key: expr, ...
		let mut config = Vec::new();
		if input.peek(syn::Token![,]) {
			input.parse::<syn::Token![,]>()?;

			while input.peek(syn::Ident) {
				let key: syn::Ident = input.parse()?;
				input.parse::<syn::Token![:]>()?;
				let value: syn::Expr = input.parse()?;

				config.push(CollectionConfigEntry { key, value });

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
			collection_type,
			store_id,
			config,
		})
	}
}

impl CollectionInput {
	#[allow(clippy::too_many_lines)]
	pub fn expand(self) -> proc_macro2::TokenStream {
		let Self {
			krate,
			attrs,
			vis,
			mode,
			name,
			generics,
			collection_type,
			store_id,
			config,
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

		// Build config calls from parsed entries.
		let mut config_calls = Vec::new();
		for entry in config {
			let key_str = entry.key.to_string();
			let value = &entry.value;

			let call = match key_str.as_str() {
				"require_ticket" => {
					quote::quote! { .require_ticket(#value) }
				}
				_ => {
					return syn::Error::new(
						entry.key.span(),
						format!("unknown collection config key `{key_str}`"),
					)
					.to_compile_error();
				}
			};

			config_calls.push(call);
		}

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

		// Generate reader/writer call expressions. When config entries
		// are present we build a `CollectionConfig` and use the
		// `_with_config` path; otherwise the simpler default path.
		let (reader_call, writer_call) = if config_calls.is_empty() {
			(
				quote::quote! {
					<#collection_type as #krate::collections::CollectionFromDef>::reader(
						network,
						#store_id_expr,
					)
				},
				quote::quote! {
					<#collection_type as #krate::collections::CollectionFromDef>::writer(
						network,
						#store_id_expr,
					)
				},
			)
		} else {
			(
				quote::quote! {
					<#collection_type as #krate::collections::CollectionFromDef>::reader_with_config(
						network,
						#store_id_expr,
						#krate::collections::CollectionConfig::default()
							#(#config_calls)*,
					)
				},
				quote::quote! {
					<#collection_type as #krate::collections::CollectionFromDef>::writer_with_config(
						network,
						#store_id_expr,
						#krate::collections::CollectionConfig::default()
							#(#config_calls)*,
					)
				},
			)
		};

		let reader_impl = if matches!(mode, CollectionMode::WriterOnly) {
			// Writer-only mode: still generate reader access, but as
			// inherent `pub(crate)` methods so they are only usable
			// within the defining crate.
			quote::quote! {
				impl #impl_generics #name #ty_generics #where_clause {
					/// Creates a reader for this collection.
					///
					/// This is only available within the crate that
					/// defines this collection.
					pub(crate) fn reader(
						network: &#krate::Network,
					) -> <#collection_type as #krate::collections::CollectionFromDef>::Reader {
						#reader_call
					}

					/// Creates a reader and waits for it to come online.
					///
					/// This is only available within the crate that
					/// defines this collection.
					pub(crate) fn online_reader(
						network: &#krate::Network,
					) -> impl Future<
						Output = <#collection_type as #krate::collections::CollectionFromDef>::Reader,
					> + Send + Sync + 'static {
						let reader = Self::reader(network);
						async move {
							reader.when().online().await;
							reader
						}
					}
				}
			}
		} else {
			quote::quote! {
				const _: () = {
					impl #impl_generics #krate::collections::CollectionReader
						for #name #ty_generics #where_clause
					{
						type Reader = <#collection_type as #krate::collections::CollectionFromDef>::Reader;

						fn reader(network: &#krate::Network) -> Self::Reader {
							#reader_call
						}

						fn online_reader(network: &#krate::Network) -> impl Future<Output = Self::Reader> + Send + Sync + 'static {
							let reader = Self::reader(network);
							async move {
								reader.when().online().await;
								reader
							}
						}
					}
				};
			}
		};

		let writer_impl = if matches!(mode, CollectionMode::ReaderOnly) {
			// Reader-only mode: still generate writer access, but as
			// inherent `pub(crate)` methods so they are only usable
			// within the defining crate.
			quote::quote! {
				impl #impl_generics #name #ty_generics #where_clause {
					/// Creates a writer for this collection.
					///
					/// This is only available within the crate that
					/// defines this collection.
					pub(crate) fn writer(
						network: &#krate::Network,
					) -> <#collection_type as #krate::collections::CollectionFromDef>::Writer {
						#writer_call
					}

					/// Creates a writer and waits for it to come online.
					///
					/// This is only available within the crate that
					/// defines this collection.
					pub(crate) fn online_writer(
						network: &#krate::Network,
					) -> impl Future<
						Output = <#collection_type as #krate::collections::CollectionFromDef>::Writer,
					> + Send + Sync + 'static {
						let writer = Self::writer(network);
						async move {
							writer.when().online().await;
							writer
						}
					}
				}
			}
		} else {
			quote::quote! {
				const _: () = {
					impl #impl_generics #krate::collections::CollectionWriter
						for #name #ty_generics #where_clause
					{
						type Writer = <#collection_type as #krate::collections::CollectionFromDef>::Writer;

						fn writer(network: &#krate::Network) -> Self::Writer {
							#writer_call
						}

						fn online_writer(
							network: &#krate::Network,
						) -> impl Future<Output = Self::Writer> + Send + Sync + 'static {
							let writer = Self::writer(network);

							async move {
								writer.when().online().await;
								writer
							}
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
