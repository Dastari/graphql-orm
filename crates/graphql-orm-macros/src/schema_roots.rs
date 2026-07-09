use super::*;
use crate::backend::{BackendKind, backend_dialect_expr, backend_marker_tokens, resolve_backend};
use crate::entity::{
    resolver_auth_mode_value_tokens, schema_policy_tokens, validate_resolver_auth_mode,
    validate_schema_policy,
};

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let args = match syn::parse::<SchemaRootsArgs>(input) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };

    let query_custom_ops = &args.query_custom_ops;
    let entities = &args.entities;
    let backend = match resolve_backend(
        args.backend.as_deref(),
        proc_macro2::Span::mixed_site(),
        "schema_roots!",
    ) {
        Ok(backend) => backend,
        Err(err) => return err.to_compile_error().into(),
    };
    let backend_marker = backend_marker_tokens(backend);
    let backend_dialect = backend_dialect_expr(backend);
    let span = proc_macro2::Span::mixed_site();
    if cfg!(any(
        all(feature = "sqlite", feature = "postgres"),
        all(feature = "sqlite", feature = "mssql"),
        all(feature = "postgres", feature = "mssql")
    )) && args.schema_policy.is_none()
    {
        return quote! {
            compile_error!("schema_roots! requires `schema_policy: \"...\"` when multiple graphql-orm backend features are enabled");
        }
        .into();
    }
    let schema_policy_const = schema_policy_tokens(args.schema_policy.as_deref());
    let schema_auth_data = match args.auth.as_deref() {
        Some(auth) => match resolver_auth_mode_value_tokens(auth, span) {
            Ok(schema_auth_mode) => quote! {
                let builder = builder.data(::graphql_orm::graphql::auth::ResolverAuthConfig::new(
                    #schema_auth_mode
                ));
            },
            Err(err) => return err.to_compile_error().into(),
        },
        None => quote! {},
    };
    let schema_policy_read_only =
        matches!(args.schema_policy.as_deref(), Some("external_read_only"));

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
    let generated_mutation_allowlist: std::collections::BTreeSet<String> = args
        .generated_mutation_allowlist
        .iter()
        .map(ToString::to_string)
        .collect();
    let generated_mutation_denylist: std::collections::BTreeSet<String> = args
        .generated_mutation_denylist
        .iter()
        .map(ToString::to_string)
        .collect();
    let mutation_types: Vec<proc_macro2::TokenStream> = entities
        .iter()
        .filter(|entity| match args.generated_mutations {
            GeneratedMutationExposure::All => true,
            GeneratedMutationExposure::None => false,
            GeneratedMutationExposure::Allowlist => {
                generated_mutation_allowlist.contains(&entity.to_string())
            }
            GeneratedMutationExposure::Denylist => {
                !generated_mutation_denylist.contains(&entity.to_string())
            }
        })
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

    if backend == BackendKind::Mssql || schema_policy_read_only {
        if !args.extra_mutation_types.is_empty() {
            return quote! {
                compile_error!("graphql-orm schema policy is read-only; extra mutation root types are not supported");
            }
            .into();
        }
        if !args.extra_subscription_types.is_empty() {
            return quote! {
                compile_error!("graphql-orm schema policy is read-only; extra subscription root types are not supported");
            }
            .into();
        }

        let schema_loader_data: Vec<proc_macro2::TokenStream> = entities
            .iter()
            .map(|entity| {
                quote! {
                    let builder = builder.data(
                        ::graphql_orm::async_graphql::dataloader::DataLoader::new(
                            ::graphql_orm::graphql::loaders::RelationLoader::<#entity, #backend_marker>::new(database.clone()),
                            ::graphql_orm::tokio::spawn,
                        )
                    );
                }
            })
            .collect();
        let entity_metadata_items: Vec<proc_macro2::TokenStream> = entities
            .iter()
            .map(|entity| {
                quote! {
                    <#entity as ::graphql_orm::graphql::orm::Entity>::metadata()
                }
            })
            .collect();
        let entity_rls_items: Vec<proc_macro2::TokenStream> = entities
            .iter()
            .map(|entity| {
                quote! {
                    <#entity as ::graphql_orm::graphql::orm::DatabaseRls>::rls_metadata()
                }
            })
            .collect();

        return quote! {
            #query_root

            pub type MutationRoot = ::graphql_orm::async_graphql::EmptyMutation;
            pub type SubscriptionRoot = ::graphql_orm::async_graphql::EmptySubscription;
            pub type AppSchema = ::graphql_orm::async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

            pub const GRAPHQL_ORM_SCHEMA_POLICY: Option<::graphql_orm::graphql::orm::SchemaPolicy> = #schema_policy_const;

            pub fn schema_builder(
                database: ::graphql_orm::db::Database<#backend_marker>,
            ) -> ::graphql_orm::async_graphql::SchemaBuilder<QueryRoot, MutationRoot, SubscriptionRoot> {
                schema_builder_with_limits(
                    database,
                    ::graphql_orm::graphql::orm::SchemaLimits::default(),
                )
            }

            pub fn schema_builder_with_limits(
                database: ::graphql_orm::db::Database<#backend_marker>,
                limits: ::graphql_orm::graphql::orm::SchemaLimits,
            ) -> ::graphql_orm::async_graphql::SchemaBuilder<QueryRoot, MutationRoot, SubscriptionRoot> {
                let builder = ::graphql_orm::async_graphql::Schema::build(
                    QueryRoot::default(),
                    ::graphql_orm::async_graphql::EmptyMutation,
                    ::graphql_orm::async_graphql::EmptySubscription,
                )
                .data(database.clone());
                let builder = limits.apply(builder);
                #schema_auth_data
                #(#schema_loader_data)*
                builder
            }

            pub fn graphql_orm_entity_metadata(
            ) -> Vec<&'static ::graphql_orm::graphql::orm::EntityMetadata> {
                vec![
                    #(#entity_metadata_items),*
                ]
            }

            pub fn graphql_orm_backup_entities(
            ) -> Vec<::graphql_orm::graphql::orm::EntityBackupDescriptor> {
                ::graphql_orm::graphql::orm::backup_descriptors_from_entities(
                    &graphql_orm_entity_metadata(),
                )
            }

            pub fn graphql_orm_schema_snapshot(
                migration_version: impl Into<String>,
            ) -> ::graphql_orm::graphql::orm::GraphqlOrmSchemaSnapshot {
                ::graphql_orm::graphql::orm::schema_snapshot_from_entities(
                    #backend_dialect,
                    migration_version,
                    &graphql_orm_entity_metadata(),
                )
            }

            pub fn graphql_orm_schema_target(
            ) -> ::graphql_orm::graphql::orm::SchemaTarget {
                ::graphql_orm::graphql::orm::SchemaTarget::from_entities(
                    &graphql_orm_entity_metadata(),
                    &[
                        #(#entity_rls_items),*
                    ],
                )
            }
        }
        .into();
    }

    let mutation_root_is_empty = mutation_custom_ops.is_none() && mutation_types.is_empty();
    let (mutation_root, mutation_root_value) = if mutation_root_is_empty {
        (
            quote! {
                pub type MutationRoot = ::graphql_orm::async_graphql::EmptyMutation;
            },
            quote! {
                ::graphql_orm::async_graphql::EmptyMutation
            },
        )
    } else {
        (
            emit_chunked_merged(
                "Mutation",
                mutation_custom_ops,
                &mutation_types,
                async_graphql_merged_object_derive(),
            ),
            quote! {
                MutationRoot::default()
            },
        )
    };
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
                    ::graphql_orm::graphql::loaders::RelationLoader::<#entity, #backend_marker>::new(database.clone()),
                        ::graphql_orm::tokio::spawn,
                    )
                );
            }
        })
        .collect();
    let entity_metadata_items: Vec<proc_macro2::TokenStream> = entities
        .iter()
        .map(|entity| {
            quote! {
                <#entity as ::graphql_orm::graphql::orm::Entity>::metadata()
            }
        })
        .collect();
    let entity_rls_items: Vec<proc_macro2::TokenStream> = entities
        .iter()
        .map(|entity| {
            quote! {
                <#entity as ::graphql_orm::graphql::orm::DatabaseRls>::rls_metadata()
            }
        })
        .collect();

    quote! {
        #query_root
        #mutation_root
        #subscription_root

        pub type AppSchema = ::graphql_orm::async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

        pub const GRAPHQL_ORM_SCHEMA_POLICY: Option<::graphql_orm::graphql::orm::SchemaPolicy> = #schema_policy_const;

        pub fn schema_builder(
            database: ::graphql_orm::db::Database<#backend_marker>,
        ) -> ::graphql_orm::async_graphql::SchemaBuilder<QueryRoot, MutationRoot, SubscriptionRoot> {
            schema_builder_with_limits(
                database,
                ::graphql_orm::graphql::orm::SchemaLimits::default(),
            )
        }

        pub fn schema_builder_with_limits(
            database: ::graphql_orm::db::Database<#backend_marker>,
            limits: ::graphql_orm::graphql::orm::SchemaLimits,
        ) -> ::graphql_orm::async_graphql::SchemaBuilder<QueryRoot, MutationRoot, SubscriptionRoot> {
            let builder = ::graphql_orm::async_graphql::Schema::build(
                QueryRoot::default(),
                #mutation_root_value,
                SubscriptionRoot::default(),
            )
            .data(database.clone());
            let builder = limits.apply(builder);
            #schema_auth_data
            #(#schema_loader_data)*
            builder
        }

        pub fn graphql_orm_entity_metadata(
        ) -> Vec<&'static ::graphql_orm::graphql::orm::EntityMetadata> {
            vec![
                #(#entity_metadata_items),*
            ]
        }

        pub fn graphql_orm_backup_entities(
        ) -> Vec<::graphql_orm::graphql::orm::EntityBackupDescriptor> {
            ::graphql_orm::graphql::orm::backup_descriptors_from_entities(
                &graphql_orm_entity_metadata(),
            )
        }

        pub fn graphql_orm_schema_snapshot(
            migration_version: impl Into<String>,
        ) -> ::graphql_orm::graphql::orm::GraphqlOrmSchemaSnapshot {
            ::graphql_orm::graphql::orm::schema_snapshot_from_entities(
                #backend_dialect,
                migration_version,
                &graphql_orm_entity_metadata(),
            )
        }

        pub fn graphql_orm_schema_target(
        ) -> ::graphql_orm::graphql::orm::SchemaTarget {
            ::graphql_orm::graphql::orm::SchemaTarget::from_entities(
                &graphql_orm_entity_metadata(),
                &[
                    #(#entity_rls_items),*
                ],
            )
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
    backend: Option<String>,
    schema_policy: Option<String>,
    auth: Option<String>,
    generated_mutations: GeneratedMutationExposure,
    query_custom_ops: Vec<Ident>,
    entities: Vec<Ident>,
    extra_mutation_types: Vec<Ident>,
    extra_query_types: Vec<Ident>,
    extra_subscription_types: Vec<Ident>,
    generated_mutation_allowlist: Vec<Ident>,
    generated_mutation_denylist: Vec<Ident>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GeneratedMutationExposure {
    All,
    None,
    Allowlist,
    Denylist,
}

impl GeneratedMutationExposure {
    fn parse(lit: syn::LitStr) -> syn::Result<Self> {
        let span = lit.span();
        match lit.value().as_str() {
            "all" => Ok(Self::All),
            "none" => Ok(Self::None),
            "allowlist" => Ok(Self::Allowlist),
            "denylist" => Ok(Self::Denylist),
            _ => Err(syn::Error::new(
                span,
                "generated_mutations must be one of \"all\", \"none\", \"allowlist\", or \"denylist\"",
            )),
        }
    }
}

impl Parse for SchemaRootsArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        fn parse_list(content: ParseStream) -> syn::Result<Vec<Ident>> {
            let list = Punctuated::<Ident, Token![,]>::parse_terminated(content)?;
            Ok(list.into_iter().collect())
        }

        fn reject_duplicate<T>(value: &Option<T>, label: &Ident) -> syn::Result<()> {
            if value.is_some() {
                return Err(syn::Error::new(
                    label.span(),
                    format!("duplicate `{}`", label),
                ));
            }
            Ok(())
        }

        let mut backend = None;
        let mut schema_policy = None;
        let mut auth = None;
        let mut generated_mutations = None;
        let mut query_custom_ops = None;
        let mut entities = None;
        let mut extra_mutation_types = None;
        let mut extra_query_types = None;
        let mut extra_subscription_types = None;
        let mut generated_mutation_allowlist = None;
        let mut generated_mutation_allowlist_span = None;
        let mut generated_mutation_denylist = None;
        let mut generated_mutation_denylist_span = None;

        while !input.is_empty() {
            let label: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            if label == "backend" {
                reject_duplicate(&backend, &label)?;
                let lit: syn::LitStr = input.parse()?;
                backend = Some(lit.value());
            } else if label == "schema_policy" {
                reject_duplicate(&schema_policy, &label)?;
                let lit: syn::LitStr = input.parse()?;
                let value = lit.value();
                validate_schema_policy(&value, lit.span())?;
                schema_policy = Some(value);
            } else if label == "auth" {
                reject_duplicate(&auth, &label)?;
                let lit: syn::LitStr = input.parse()?;
                let value = lit.value();
                validate_resolver_auth_mode(&value, lit.span())?;
                auth = Some(value);
            } else if label == "generated_mutations" {
                reject_duplicate(&generated_mutations, &label)?;
                let lit: syn::LitStr = input.parse()?;
                generated_mutations = Some((GeneratedMutationExposure::parse(lit)?, label.span()));
            } else if label == "query_custom_ops" {
                reject_duplicate(&query_custom_ops, &label)?;
                let content;
                syn::bracketed!(content in input);
                query_custom_ops = Some(parse_list(&content)?);
            } else if label == "entities" {
                reject_duplicate(&entities, &label)?;
                let content;
                syn::bracketed!(content in input);
                entities = Some(parse_list(&content)?);
            } else if label == "extra_mutation_types" {
                reject_duplicate(&extra_mutation_types, &label)?;
                let content;
                syn::bracketed!(content in input);
                extra_mutation_types = Some(parse_list(&content)?);
            } else if label == "extra_query_types" {
                reject_duplicate(&extra_query_types, &label)?;
                let content;
                syn::bracketed!(content in input);
                extra_query_types = Some(parse_list(&content)?);
            } else if label == "extra_subscription_types" {
                reject_duplicate(&extra_subscription_types, &label)?;
                let content;
                syn::bracketed!(content in input);
                extra_subscription_types = Some(parse_list(&content)?);
            } else if label == "generated_mutation_allowlist" {
                reject_duplicate(&generated_mutation_allowlist, &label)?;
                let content;
                syn::bracketed!(content in input);
                generated_mutation_allowlist = Some(parse_list(&content)?);
                generated_mutation_allowlist_span = Some(label.span());
            } else if label == "generated_mutation_denylist" {
                reject_duplicate(&generated_mutation_denylist, &label)?;
                let content;
                syn::bracketed!(content in input);
                generated_mutation_denylist = Some(parse_list(&content)?);
                generated_mutation_denylist_span = Some(label.span());
            } else {
                return Err(syn::Error::new(
                    label.span(),
                    "expected one of `backend`, `schema_policy`, `auth`, `generated_mutations`, `query_custom_ops`, `entities`, `extra_query_types`, `extra_mutation_types`, `extra_subscription_types`, `generated_mutation_allowlist`, or `generated_mutation_denylist`",
                ));
            }

            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }

        let entities = entities.ok_or_else(|| {
            syn::Error::new(proc_macro2::Span::mixed_site(), "expected `entities`")
        })?;
        let query_custom_ops = query_custom_ops.unwrap_or_default();
        let extra_mutation_types = extra_mutation_types.unwrap_or_default();
        let extra_query_types = extra_query_types.unwrap_or_default();
        let extra_subscription_types = extra_subscription_types.unwrap_or_default();
        let (generated_mutations, generated_mutations_span) = generated_mutations.unwrap_or((
            GeneratedMutationExposure::All,
            proc_macro2::Span::mixed_site(),
        ));
        let allowlist_present = generated_mutation_allowlist.is_some();
        let denylist_present = generated_mutation_denylist.is_some();
        let generated_mutation_allowlist = generated_mutation_allowlist.unwrap_or_default();
        let generated_mutation_denylist = generated_mutation_denylist.unwrap_or_default();

        if generated_mutations != GeneratedMutationExposure::Allowlist && allowlist_present {
            return Err(syn::Error::new(
                generated_mutation_allowlist_span.unwrap_or(generated_mutations_span),
                "`generated_mutation_allowlist` requires `generated_mutations: \"allowlist\"`",
            ));
        }
        if generated_mutations != GeneratedMutationExposure::Denylist && denylist_present {
            return Err(syn::Error::new(
                generated_mutation_denylist_span.unwrap_or(generated_mutations_span),
                "`generated_mutation_denylist` requires `generated_mutations: \"denylist\"`",
            ));
        }
        if generated_mutations == GeneratedMutationExposure::Allowlist
            && generated_mutation_allowlist.is_empty()
        {
            return Err(syn::Error::new(
                generated_mutations_span,
                "`generated_mutations: \"allowlist\"` requires a non-empty `generated_mutation_allowlist`",
            ));
        }
        if generated_mutations == GeneratedMutationExposure::Denylist
            && generated_mutation_denylist.is_empty()
        {
            return Err(syn::Error::new(
                generated_mutations_span,
                "`generated_mutations: \"denylist\"` requires a non-empty `generated_mutation_denylist`",
            ));
        }

        let entity_names: std::collections::BTreeSet<String> =
            entities.iter().map(ToString::to_string).collect();
        for entity in &generated_mutation_allowlist {
            if !entity_names.contains(&entity.to_string()) {
                return Err(syn::Error::new(
                    entity.span(),
                    "`generated_mutation_allowlist` entries must also be listed in `entities`",
                ));
            }
        }
        for entity in &generated_mutation_denylist {
            if !entity_names.contains(&entity.to_string()) {
                return Err(syn::Error::new(
                    entity.span(),
                    "`generated_mutation_denylist` entries must also be listed in `entities`",
                ));
            }
        }

        Ok(SchemaRootsArgs {
            backend,
            schema_policy,
            auth,
            generated_mutations,
            query_custom_ops,
            entities,
            extra_mutation_types,
            extra_query_types,
            extra_subscription_types,
            generated_mutation_allowlist,
            generated_mutation_denylist,
        })
    }
}
