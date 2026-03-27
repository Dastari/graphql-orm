use super::*;

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let args = match syn::parse::<SchemaRootsArgs>(input) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };

    let query_custom_ops = &args.query_custom_ops;
    let entities = &args.entities;

    let span = proc_macro2::Span::mixed_site();
    let custom_op_types: Vec<proc_macro2::TokenStream> = query_custom_ops
        .iter()
        .map(|entity| {
            let name = syn::Ident::new(&format!("{}CustomOperations", entity), span);
            quote! { #name }
        })
        .chain(
            args.extra_query_types
                .iter()
                .map(|entity| quote! { #entity }),
        )
        .collect();
    let query_types: Vec<proc_macro2::TokenStream> = entities
        .iter()
        .map(|entity| {
            let name = syn::Ident::new(&format!("{}Queries", entity), span);
            quote! { #name }
        })
        .collect();

    let extra_mutation_type_streams: Vec<proc_macro2::TokenStream> = args
        .extra_mutation_types
        .iter()
        .map(|entity| quote! { #entity })
        .collect();
    let mutation_custom_ops = if extra_mutation_type_streams.is_empty() {
        None
    } else {
        Some(extra_mutation_type_streams.as_slice())
    };
    let mutation_types: Vec<proc_macro2::TokenStream> = entities
        .iter()
        .map(|entity| {
            let name = syn::Ident::new(&format!("{}Mutations", entity), span);
            quote! { #name }
        })
        .collect();

    let extra_subscription_type_streams: Vec<proc_macro2::TokenStream> = args
        .extra_subscription_types
        .iter()
        .map(|entity| quote! { #entity })
        .collect();
    let subscription_custom_ops = if extra_subscription_type_streams.is_empty() {
        None
    } else {
        Some(extra_subscription_type_streams.as_slice())
    };
    let subscription_types: Vec<proc_macro2::TokenStream> = entities
        .iter()
        .map(|entity| {
            let name = syn::Ident::new(&format!("{}Subscriptions", entity), span);
            quote! { #name }
        })
        .collect();

    let query_custom_chunk = if custom_op_types.is_empty() {
        None
    } else {
        Some(custom_op_types.as_slice())
    };
    let query_root = emit_chunked_merged(
        "Query",
        query_custom_chunk,
        &query_types,
        async_graphql_merged_object_derive(),
    );
    let mutation_root = emit_chunked_merged(
        "Mutation",
        mutation_custom_ops,
        &mutation_types,
        async_graphql_merged_object_derive(),
    );
    let subscription_root = emit_chunked_merged_subscription(
        "Subscription",
        subscription_custom_ops,
        &subscription_types,
    );
    let schema_loader_data: Vec<proc_macro2::TokenStream> = entities
        .iter()
        .map(|entity| {
            quote! {
                let builder = builder.data(
                    ::graphql_orm::async_graphql::dataloader::DataLoader::new(
                        ::graphql_orm::graphql::loaders::RelationLoader::<#entity>::new(database.clone()),
                        ::graphql_orm::tokio::spawn,
                    )
                );
            }
        })
        .collect();

    quote! {
        #query_root
        #mutation_root
        #subscription_root

        pub type AppSchema = ::graphql_orm::async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

        pub fn schema_builder(
            database: ::graphql_orm::db::Database,
        ) -> ::graphql_orm::async_graphql::SchemaBuilder<QueryRoot, MutationRoot, SubscriptionRoot> {
            let builder = ::graphql_orm::async_graphql::Schema::build(
                QueryRoot::default(),
                MutationRoot::default(),
                SubscriptionRoot::default(),
            )
            .data(database.clone());
            #(#schema_loader_data)*
            builder
        }
    }
    .into()
}

fn async_graphql_merged_object_derive() -> proc_macro2::TokenStream {
    quote! { ::graphql_orm::async_graphql::MergedObject }
}

fn emit_chunked_merged(
    name: &str,
    custom_ops: Option<&[proc_macro2::TokenStream]>,
    types: &[proc_macro2::TokenStream],
    derive_macro: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let chunk_size = 12usize;
    let mut chunk_defs = Vec::new();
    let mut root_chunk_idents = Vec::new();

    // Custom ops chunk (Query only)
    if let Some(ops) = custom_ops {
        if !ops.is_empty() {
            let chunk_name = syn::Ident::new(
                &format!("{}RootCustomOpsChunk", name),
                proc_macro2::Span::mixed_site(),
            );
            let def = quote! {
                #[derive(#derive_macro, Default)]
                pub struct #chunk_name(
                    #(#ops),*
                );
            };
            chunk_defs.push(def);
            root_chunk_idents.push(chunk_name);
        }
    }

    // Entity type chunks
    for (i, chunk_types) in types.chunks(chunk_size).enumerate() {
        let chunk_name = syn::Ident::new(
            &format!("{}RootChunk{}", name, i),
            proc_macro2::Span::mixed_site(),
        );
        let def = quote! {
            #[derive(#derive_macro, Default)]
            pub struct #chunk_name(
                #(#chunk_types),*
            );
        };
        chunk_defs.push(def);
        root_chunk_idents.push(chunk_name);
    }

    let root_name = syn::Ident::new(&format!("{}Root", name), proc_macro2::Span::mixed_site());
    let root_def = quote! {
        #[derive(#derive_macro, Default)]
        pub struct #root_name(
            #(#root_chunk_idents),*
        );
    };

    quote! {
        #(#chunk_defs)*
        #root_def
    }
}

fn emit_chunked_merged_subscription(
    name: &str,
    custom_ops: Option<&[proc_macro2::TokenStream]>,
    types: &[proc_macro2::TokenStream],
) -> proc_macro2::TokenStream {
    let derive_macro = quote! { ::graphql_orm::async_graphql::MergedSubscription };
    emit_chunked_merged(name, custom_ops, types, derive_macro)
}

struct SchemaRootsArgs {
    query_custom_ops: Vec<Ident>,
    entities: Vec<Ident>,
    extra_mutation_types: Vec<Ident>,
    extra_query_types: Vec<Ident>,
    extra_subscription_types: Vec<Ident>,
}

impl Parse for SchemaRootsArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        fn parse_list(content: ParseStream) -> syn::Result<Vec<Ident>> {
            let list = Punctuated::<Ident, Token![,]>::parse_terminated(content)?;
            Ok(list.into_iter().collect())
        }

        // query_custom_ops: [ ... ],
        let label: Ident = input.parse()?;
        if label != "query_custom_ops" {
            return Err(syn::Error::new(label.span(), "expected `query_custom_ops`"));
        }
        input.parse::<Token![:]>()?;
        let content;
        syn::bracketed!(content in input);
        let query_custom_ops = parse_list(&content)?;
        let _: Option<Token![,]> = input.parse().ok();

        // entities: [ ... ]
        let label: Ident = input.parse()?;
        if label != "entities" {
            return Err(syn::Error::new(label.span(), "expected `entities`"));
        }
        input.parse::<Token![:]>()?;
        let content;
        syn::bracketed!(content in input);
        let entities = parse_list(&content)?;
        let _: Option<Token![,]> = input.parse().ok();

        // optional extra_mutation_types, extra_query_types, extra_subscription_types
        let mut extra_mutation_types = Vec::new();
        let mut extra_query_types = Vec::new();
        let mut extra_subscription_types = Vec::new();
        while input.peek(Ident) {
            let label: Ident = input.parse()?;
            if label == "extra_mutation_types" {
                input.parse::<Token![:]>()?;
                let content;
                syn::bracketed!(content in input);
                extra_mutation_types = parse_list(&content)?;
                let _: Option<Token![,]> = input.parse().ok();
            } else if label == "extra_query_types" {
                input.parse::<Token![:]>()?;
                let content;
                syn::bracketed!(content in input);
                extra_query_types = parse_list(&content)?;
                let _: Option<Token![,]> = input.parse().ok();
            } else if label == "extra_subscription_types" {
                input.parse::<Token![:]>()?;
                let content;
                syn::bracketed!(content in input);
                extra_subscription_types = parse_list(&content)?;
                let _: Option<Token![,]> = input.parse().ok();
            } else {
                return Err(syn::Error::new(
                    label.span(),
                    "expected `extra_mutation_types`, `extra_query_types`, or `extra_subscription_types`",
                ));
            }
        }

        Ok(SchemaRootsArgs {
            query_custom_ops,
            entities,
            extra_mutation_types,
            extra_query_types,
            extra_subscription_types,
        })
    }
}
