use super::try_decode_hex;

enum CollectionMode {
	Full,
	ReaderOnly,
	WriterOnly,
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
						<#collection_type as #krate::collections::CollectionFromDef>::reader(
							network,
							#store_id_expr,
						)
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
							<#collection_type as #krate::collections::CollectionFromDef>::reader(
								network,
								#store_id_expr,
							)
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
						<#collection_type as #krate::collections::CollectionFromDef>::writer(
							network,
							#store_id_expr,
						)
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
							<#collection_type as #krate::collections::CollectionFromDef>::writer(
								network,
								#store_id_expr,
							)
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
