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
    // Opt-in review gate: every listed entity must record a policy decision
    // in its declaration. A const assertion names the entity that has none.
    let policy_decision_checks = if args.require_policies {
        let checks = entities.iter().map(|entity| {
            let message = format!(
                "schema_roots! `require_policies: true`: entity `{entity}` declares no policy decision; add `read_policy` or `write_policy` to its entity attribute, or mark it deliberately open with `policy = \"open\"` (or public with `auth = \"none\"`)"
            );
            quote! {
                const _: () = ::std::assert!(
                    <#entity as ::graphql_orm::graphql::orm::Entity>::POLICY_DECISION_DECLARED,
                    #message
                );
            }
        });
        quote! { #(#checks)* }
    } else {
        quote! {}
    };
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
    // A subgraph whose only Federation participation is an unresolvable stub
    // has no generated key, so async-graphql never enables federation by
    // itself. This opt-in adds `_service` and the federation SDL export that
    // composition needs.
    let federation_enable = if args.federation {
        quote! { let builder = builder.enable_federation(); }
    } else {
        quote! {}
    };
    let schema_policy_read_only =
        matches!(args.schema_policy.as_deref(), Some("external_read_only"));
    let backend_writable =
        backend != BackendKind::Mssql || args.schema_policy.as_deref() == Some("external_writable");

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
        .chain(
            args.described_query_types
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
        .chain(&args.described_mutation_types)
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
    let generated_mutation_is_exposed = |entity: &Ident| {
        if !backend_writable || schema_policy_read_only {
            return false;
        }
        match args.generated_mutations {
            GeneratedMutationExposure::All => true,
            GeneratedMutationExposure::None => false,
            GeneratedMutationExposure::Allowlist => {
                generated_mutation_allowlist.contains(&entity.to_string())
            }
            GeneratedMutationExposure::Denylist => {
                !generated_mutation_denylist.contains(&entity.to_string())
            }
        }
    };
    let mutation_types: Vec<proc_macro2::TokenStream> = entities
        .iter()
        .filter(|entity| generated_mutation_is_exposed(entity))
        .map(|entity| {
            let name = syn::Ident::new(&format!("{}Mutations", entity), span);
            quote! { #name }
        })
        .collect();
    let operation_metadata_groups = entities
        .iter()
        .map(|entity| {
            let mutations_exposed = generated_mutation_is_exposed(entity);
            let subscriptions_exposed = true;
            quote! {
                (
                    <#entity as ::graphql_orm::graphql::orm::GraphqlOperationMetadata>
                        ::generated_graphql_operations(),
                    #mutations_exposed,
                    #subscriptions_exposed,
                )
            }
        })
        .collect::<Vec<_>>();
    // The schema's maximum page limit is fixed at compile time: the declared
    // `max_page_limit`, or the default of 100. The catalogue's pageable
    // relationship lists publish it, the `Database` is checked against it
    // when the schema is built, and it bounds the relation complexity
    // estimate.
    let declared_page_maximum = args.max_page_limit.unwrap_or(100);
    let page_maximum_const = quote! {
        /// The maximum page limit of this schema: the one declared with
        /// `schema_roots!(max_page_limit: ...)`, or 100. The semantic
        /// catalogue publishes it, and the schema refuses a `Database`
        /// configured with a different one.
        pub const GRAPHQL_ORM_MAX_PAGE_LIMIT: ::std::option::Option<u32> =
            ::std::option::Option::Some(#declared_page_maximum);
    };
    let page_maximum_check = quote! {
        ::graphql_orm::graphql::orm::admit_declared_page_maximum(
            &database,
            GRAPHQL_ORM_MAX_PAGE_LIMIT,
        );
    };
    let semantic_entity_items = entities
        .iter()
        .map(|entity| {
            quote! {
                ::graphql_orm::graphql::orm::with_declared_page_maximum(
                    <#entity as ::graphql_orm::graphql::orm::Entity>
                        ::graphql_semantic_metadata()
                        .expect("GraphQLEntity must emit semantic metadata")
                        .clone(),
                    GRAPHQL_ORM_MAX_PAGE_LIMIT,
                )
            }
        })
        .collect::<Vec<_>>();
    let semantic_type_items = args
        .semantic_types
        .iter()
        .map(|semantic_type| {
            quote! {
                <#semantic_type as ::graphql_orm::graphql::orm::GraphqlSemanticObjectMetadata>
                    ::graphql_semantic_object()
                    .clone()
            }
        })
        .collect::<Vec<_>>();
    let semantic_custom_operation_items = args
        .semantic_custom_operations
        .iter()
        .chain(&args.described_query_types)
        .chain(&args.described_mutation_types)
        .chain(&args.described_subscription_types)
        .map(|root| {
            quote! {
                <#root as ::graphql_orm::graphql::orm::GraphqlCustomOperationMetadata>
                    ::graphql_custom_operations()
                    .iter()
                    .cloned()
            }
        })
        .collect::<Vec<_>>();
    let described_result_type_items = args
        .described_query_types
        .iter()
        .chain(&args.described_mutation_types)
        .chain(&args.described_subscription_types)
        .map(|root| {
            quote! {
                <#root as ::graphql_orm::graphql::orm::GraphqlCustomOperationMetadata>
                    ::graphql_custom_result_types()
                    .iter()
                    .cloned()
            }
        })
        .collect::<Vec<_>>();
    let backend_name = backend.name();
    let interpretation_entities = entities
        .iter()
        .map(|entity| {
            quote! {
                ::graphql_orm::graphql::orm::InterpretationEntity::of::<#entity, #backend_marker>(
                    #entity::__GOM_INTERPRETATION_SOURCE,
                )
            }
        })
        .collect::<Vec<_>>();
    let client_metadata_helper = if args.client_metadata {
        let sources = entities.iter().map(|entity| {
            quote! {
                ::graphql_orm::graphql::orm::ClientEntitySource::of::<#entity, #backend_marker>(
                    #entity::__GOM_CHANGED_ROOT_NAME,
                    #entity::__GOM_CHANGED_STARTED_SIGNAL,
                )
            }
        });
        quote! {
            /// Returns the client-facing companion metadata of
            /// [`graphql_orm_semantic_catalog`]: the fields each generated
            /// `Changed` stream can carry, each declared lifetime field and
            /// each public relationship's key mapping. Export it at build
            /// time with `GraphqlClientMetadata::export_json`.
            ///
            /// The metadata is descriptive. It grants no resolver, field, row,
            /// database or provider authority, and the server never enforces it.
            pub fn graphql_orm_client_metadata(
            ) -> &'static ::graphql_orm::graphql::orm::GraphqlClientMetadata {
                static METADATA: ::std::sync::OnceLock<
                    ::graphql_orm::graphql::orm::GraphqlClientMetadata
                > = ::std::sync::OnceLock::new();
                METADATA.get_or_init(|| {
                    ::graphql_orm::graphql::orm::compose_client_metadata(
                        graphql_orm_semantic_catalog(),
                        &[#(#sources),*],
                    )
                })
            }
        }
    } else {
        quote! {}
    };
    let operation_catalog_helper = quote! {
        #client_metadata_helper

        /// Returns this schema's interpretation fingerprint: a value that
        /// changes when a release may read existing cursors, keys or `NodeId`
        /// values differently, and stays the same across restarts and
        /// replicas otherwise. `salt` is an optional host value, such as a
        /// cursor-key version; only its SHA-256 enters the fingerprint.
        /// See `graphql_orm::graphql::orm::interpretation_fingerprint` for
        /// every input.
        pub fn graphql_orm_interpretation_fingerprint(
            salt: ::std::option::Option<&str>,
        ) -> ::std::string::String {
            ::graphql_orm::graphql::orm::interpretation_fingerprint(
                #backend_name,
                &[#(#interpretation_entities),*],
                salt,
            )
        }

        /// Returns deterministic generated resolver metadata for this schema root.
        ///
        /// The catalog resolves generated root exposure after mutation and
        /// read-only policy but is discovery metadata only; it does not
        /// authorize GraphQL execution.
        ///
        /// # Panics
        ///
        /// Panics on a repeated entity identity during catalogue composition at
        /// schema build, naming both Rust types.
        pub fn graphql_orm_operation_catalog(
        ) -> &'static ::graphql_orm::graphql::orm::GraphqlOperationCatalog {
            static CATALOG: ::std::sync::OnceLock<
                ::graphql_orm::graphql::orm::GraphqlOperationCatalog
            > = ::std::sync::OnceLock::new();
            CATALOG.get_or_init(|| {
                let groups: &[(
                    &'static [
                        ::graphql_orm::graphql::orm::GeneratedGraphqlOperationDescriptor
                    ],
                    bool,
                    bool,
                )] = &[
                    #(#operation_metadata_groups),*
                ];
                ::graphql_orm::graphql::orm::GraphqlOperationCatalog::compose(
                    groups.iter().copied(),
                )
            })
        }

        /// Returns the canonical public GraphQL semantic catalogue for this
        /// generated schema root.
        ///
        /// The catalogue is descriptive, fingerprinted metadata. It grants no
        /// resolver, field, row, database, or provider authority.
        pub fn graphql_orm_semantic_catalog(
        ) -> &'static ::graphql_orm::graphql::orm::GraphqlSemanticCatalog {
            static CATALOG: ::std::sync::OnceLock<
                ::graphql_orm::graphql::orm::GraphqlSemanticCatalog
            > = ::std::sync::OnceLock::new();
            CATALOG.get_or_init(|| {
                ::graphql_orm::graphql::orm::GraphqlSemanticCatalog::compose_with_custom(
                    ::std::iter::empty::<
                        ::graphql_orm::graphql::orm::GraphqlEntitySemanticMetadata
                    >()
                        .chain(vec![#(#semantic_entity_items),*])
                        .chain(vec![#(#semantic_type_items),*])
                        #(.chain(#described_result_type_items))*,
                    graphql_orm_operation_catalog(),
                    ::std::iter::empty()
                        #(.chain(#semantic_custom_operation_items))*
                ).expect("macro-generated GraphQL semantic catalogue must validate")
            })
        }
    };

    // Reject, at compile time, a handwritten root field with the name of a
    // generated operation or of another handwritten root's field: a merged
    // root would serve only one of them.
    let root_field_sources = |constant: &str, exposed: &dyn Fn(&Ident) -> bool| {
        let constant = syn::Ident::new(constant, span);
        entities
            .iter()
            .filter(|entity| exposed(entity))
            .map(|entity| {
                quote! {
                    ::graphql_orm::graphql::orm::RootFieldSource {
                        owner: stringify!(#entity),
                        fields: #entity::#constant,
                    }
                }
            })
            .collect::<Vec<_>>()
    };
    let handwritten_sources = |roots: &[&proc_macro2::TokenStream]| {
        roots
            .iter()
            .map(|root| {
                quote! {
                    ::graphql_orm::graphql::orm::RootFieldSource {
                        owner: stringify!(#root),
                        fields: <#root as ::graphql_orm::graphql::orm::GraphqlRootFieldNames>
                            ::ROOT_FIELD_NAMES,
                    }
                }
            })
            .collect::<Vec<_>>()
    };
    let generated_queries = root_field_sources("__GOM_QUERY_ROOT_FIELDS", &|_| true);
    let generated_mutations =
        root_field_sources("__GOM_MUTATION_ROOT_FIELDS", &generated_mutation_is_exposed);
    let generated_subscriptions = root_field_sources("__GOM_SUBSCRIPTION_ROOT_FIELDS", &|_| true);
    let mutation_opt_outs = entities
        .iter()
        .filter(|entity| generated_mutation_is_exposed(entity))
        .map(|entity| quote! { #entity::__GOM_MUTATIONS_OPTED_OUT })
        .collect::<Vec<_>>();
    let subscription_opt_outs = entities
        .iter()
        .map(|entity| quote! { #entity::__GOM_SUBSCRIPTIONS_OPTED_OUT })
        .collect::<Vec<_>>();
    let handwritten_queries = handwritten_sources(&custom_op_types.iter().collect::<Vec<_>>());
    let handwritten_mutations =
        handwritten_sources(&extra_mutation_type_streams.iter().collect::<Vec<_>>());
    let described_subscription_streams = args
        .extra_subscription_types
        .iter()
        .chain(&args.described_subscription_types)
        .map(|root| quote! { #root })
        .collect::<Vec<_>>();
    let handwritten_subscriptions =
        handwritten_sources(&described_subscription_streams.iter().collect::<Vec<_>>());
    // The collision check trusts each handwritten root's declared field
    // names. When the schema is built, compare them with the fields the type
    // really exposes, so a manual declaration that omits one is refused.
    let handwritten_query_roots = &custom_op_types;
    let handwritten_mutation_roots = &extra_mutation_type_streams;
    let handwritten_subscription_roots = &described_subscription_streams;
    let root_field_declaration_check = quote! {
        #(
            ::graphql_orm::graphql::orm::assert_declared_object_root_fields::<#handwritten_query_roots>(
                stringify!(#handwritten_query_roots),
                <#handwritten_query_roots as ::graphql_orm::graphql::orm::GraphqlRootFieldNames>::ROOT_FIELD_NAMES,
            );
        )*
        #(
            ::graphql_orm::graphql::orm::assert_declared_object_root_fields::<#handwritten_mutation_roots>(
                stringify!(#handwritten_mutation_roots),
                <#handwritten_mutation_roots as ::graphql_orm::graphql::orm::GraphqlRootFieldNames>::ROOT_FIELD_NAMES,
            );
        )*
        #(
            ::graphql_orm::graphql::orm::assert_declared_subscription_root_fields::<#handwritten_subscription_roots>(
                stringify!(#handwritten_subscription_roots),
                <#handwritten_subscription_roots as ::graphql_orm::graphql::orm::GraphqlRootFieldNames>::ROOT_FIELD_NAMES,
            );
        )*
    };
    let root_field_collision_check = quote! {
        #[allow(long_running_const_eval)]
        const _: () = {
            ::graphql_orm::graphql::orm::assert_unique_root_fields(
                "Query",
                &[#(#generated_queries),*],
                &[#(#handwritten_queries),*],
            );
            ::graphql_orm::graphql::orm::assert_unique_root_fields(
                "Mutation",
                &[#(#generated_mutations),*],
                &[#(#handwritten_mutations),*],
            );
            ::graphql_orm::graphql::orm::assert_unique_root_fields(
                "Subscription",
                &[#(#generated_subscriptions),*],
                &[#(#handwritten_subscriptions),*],
            );
        };
    };

    let extra_subscription_type_streams: Vec<proc_macro2::TokenStream> = args
        .extra_subscription_types
        .iter()
        .chain(&args.described_subscription_types)
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

    if !backend_writable || schema_policy_read_only {
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
                            ::graphql_orm::graphql::loaders::RelationLoader::<#entity, #backend_marker>::__generated(database.clone()),
                            ::graphql_orm::tokio::spawn,
                        )
                    );
                    // Federation entity resolvers batch their representations
                    // here. The loader is registered for every entity so that
                    // adding a key never changes the schema wiring.
                    let builder = builder.data(
                        ::graphql_orm::async_graphql::dataloader::DataLoader::new(
                            ::graphql_orm::graphql::loaders::FederationKeyLoader::<#entity, #backend_marker>::new(database.clone()),
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

        let readonly_subscription_root = if subscription_types.is_empty() {
            quote! { pub type SubscriptionRoot = ::graphql_orm::async_graphql::EmptySubscription; }
        } else {
            emit_chunked_merged(
                "Subscription",
                None,
                &subscription_types,
                quote! { ::graphql_orm::async_graphql::MergedSubscription },
            )
        };
        let (subscription_selection, subscription_schema_type) = select_schema_root(
            "Subscription",
            quote! { ::graphql_orm::async_graphql::EmptySubscription },
            &generated_subscriptions,
            &subscription_opt_outs,
            false,
        );
        return quote! {
            #policy_decision_checks
            #query_root
            #operation_catalog_helper
            #root_field_collision_check

            pub type MutationRoot = ::graphql_orm::async_graphql::EmptyMutation;
            #readonly_subscription_root
            #subscription_selection
            pub type AppSchema = ::graphql_orm::async_graphql::Schema<QueryRoot, MutationRoot, #subscription_schema_type>;
            type __GomSchemaBuilder = ::graphql_orm::async_graphql::SchemaBuilder<QueryRoot, MutationRoot, #subscription_schema_type>;

            pub const GRAPHQL_ORM_SCHEMA_POLICY: Option<::graphql_orm::graphql::orm::SchemaPolicy> = #schema_policy_const;

            #page_maximum_const

            pub fn schema_builder(
                database: ::graphql_orm::db::Database<#backend_marker>,
            ) -> __GomSchemaBuilder {
                schema_builder_with_limits(
                    database,
                    ::graphql_orm::graphql::orm::SchemaLimits::default(),
                )
            }

            /// Build the schema, panicking if an exposed Keyset root's entity is
            /// declared `ReadVisibilityCapability::ResidualOnly` by the row policy.
            pub fn schema_builder_with_limits(
                database: ::graphql_orm::db::Database<#backend_marker>,
                limits: ::graphql_orm::graphql::orm::SchemaLimits,
            ) -> __GomSchemaBuilder {
                match try_schema_builder_with_limits(database, limits) {
                    Ok(builder) => builder,
                    Err(error) => panic!("{error}"),
                }
            }

            /// Build the schema, or return the Keyset read-policy contradiction.
            ///
            /// # Panics
            ///
            /// Panics on a repeated entity identity during catalogue composition at
            /// schema build, naming both Rust types. This is not returned as a
            /// `KeysetReadPolicyError`.
            pub fn try_schema_builder(
                database: ::graphql_orm::db::Database<#backend_marker>,
            ) -> ::std::result::Result<
                __GomSchemaBuilder,
                ::graphql_orm::graphql::orm::KeysetReadPolicyError,
            > {
                try_schema_builder_with_limits(
                    database,
                    ::graphql_orm::graphql::orm::SchemaLimits::default(),
                )
            }

            /// Build the schema with limits, or return the Keyset read-policy contradiction.
            ///
            /// # Panics
            ///
            /// Panics on a repeated entity identity during catalogue composition at
            /// schema build, naming both Rust types. This is not returned as a
            /// `KeysetReadPolicyError`.
            /// It also panics when a handwritten root exposes a field its
            /// `GraphqlRootFieldNames` declaration omits.
            pub fn try_schema_builder_with_limits(
                database: ::graphql_orm::db::Database<#backend_marker>,
                limits: ::graphql_orm::graphql::orm::SchemaLimits,
            ) -> ::std::result::Result<
                __GomSchemaBuilder,
                ::graphql_orm::graphql::orm::KeysetReadPolicyError,
            > {
                #root_field_declaration_check
                #page_maximum_check
                database.validate_keyset_read_policies(
                    graphql_orm_operation_catalog(),
                    &graphql_orm_entity_metadata(),
                )?;
                let builder = ::graphql_orm::async_graphql::Schema::build(
                    QueryRoot::default(),
                    ::graphql_orm::async_graphql::EmptyMutation,
                    ::std::default::Default::default(),
                )
                .data(database.clone())
            .data(::graphql_orm::graphql::orm::DeclaredPageMaximum(GRAPHQL_ORM_MAX_PAGE_LIMIT))
            .extension(::graphql_orm::graphql::orm::PageMaximumValidation)
            .extension(::graphql_orm::graphql::errors::UncodedCallbackMessages(
                database.exposes_uncoded_callback_messages(),
            ))
            .extension(::graphql_orm::graphql::input_validation::GeneratedInputValidation)
            .extension(::graphql_orm::graphql::subscriptions::GeneratedSubscriptionAdmission(
                ::std::iter::IntoIterator::into_iter(
                    &[#(#entities::__GOM_SUBSCRIPTION_ROOT_FIELDS),*] as &[&[&str]],
                ).flat_map(|fields| fields.iter().copied()).collect()
            ));
                let builder = limits.apply(builder);
                #federation_enable
                #schema_auth_data
                #(#schema_loader_data)*
                Ok(builder)
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

    let mutation_root = if mutation_custom_ops.is_none() && mutation_types.is_empty() {
        quote! { pub type MutationRoot = ::graphql_orm::async_graphql::EmptyMutation; }
    } else {
        emit_chunked_merged(
            "Mutation",
            mutation_custom_ops,
            &mutation_types,
            async_graphql_merged_object_derive(),
        )
    };
    let subscription_root = if subscription_custom_ops.is_none() && subscription_types.is_empty() {
        quote! { pub type SubscriptionRoot = ::graphql_orm::async_graphql::EmptySubscription; }
    } else {
        emit_chunked_merged(
            "Subscription",
            subscription_custom_ops,
            &subscription_types,
            quote! { ::graphql_orm::async_graphql::MergedSubscription },
        )
    };
    let (mutation_selection, mutation_schema_type) = select_schema_root(
        "Mutation",
        quote! { ::graphql_orm::async_graphql::EmptyMutation },
        &generated_mutations,
        &mutation_opt_outs,
        mutation_custom_ops.is_some(),
    );
    let (subscription_selection, subscription_schema_type) = select_schema_root(
        "Subscription",
        quote! { ::graphql_orm::async_graphql::EmptySubscription },
        &generated_subscriptions,
        &subscription_opt_outs,
        subscription_custom_ops.is_some(),
    );
    let schema_loader_data: Vec<proc_macro2::TokenStream> = entities
        .iter()
        .map(|entity| {
            quote! {
                let builder = builder.data(
                    ::graphql_orm::async_graphql::dataloader::DataLoader::new(
                    ::graphql_orm::graphql::loaders::RelationLoader::<#entity, #backend_marker>::__generated(database.clone()),
                        ::graphql_orm::tokio::spawn,
                    )
                );
                // Federation entity resolvers batch their representations here.
                // The loader is registered for every entity so that adding a
                // key never changes the schema wiring.
                let builder = builder.data(
                    ::graphql_orm::async_graphql::dataloader::DataLoader::new(
                        ::graphql_orm::graphql::loaders::FederationKeyLoader::<#entity, #backend_marker>::new(database.clone()),
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
        #policy_decision_checks
        #query_root
        #mutation_root
        #subscription_root
        #mutation_selection
        #subscription_selection
        #operation_catalog_helper
        #root_field_collision_check

        pub type AppSchema = ::graphql_orm::async_graphql::Schema<QueryRoot, #mutation_schema_type, #subscription_schema_type>;
        type __GomSchemaBuilder = ::graphql_orm::async_graphql::SchemaBuilder<QueryRoot, #mutation_schema_type, #subscription_schema_type>;

        pub const GRAPHQL_ORM_SCHEMA_POLICY: Option<::graphql_orm::graphql::orm::SchemaPolicy> = #schema_policy_const;

            #page_maximum_const

        pub fn schema_builder(
            database: ::graphql_orm::db::Database<#backend_marker>,
        ) -> __GomSchemaBuilder {
            schema_builder_with_limits(
                database,
                ::graphql_orm::graphql::orm::SchemaLimits::default(),
            )
        }

        /// Build the schema, panicking if an exposed Keyset root's entity is
        /// declared `ReadVisibilityCapability::ResidualOnly` by the row policy.
        pub fn schema_builder_with_limits(
            database: ::graphql_orm::db::Database<#backend_marker>,
            limits: ::graphql_orm::graphql::orm::SchemaLimits,
        ) -> __GomSchemaBuilder {
            match try_schema_builder_with_limits(database, limits) {
                Ok(builder) => builder,
                Err(error) => panic!("{error}"),
            }
        }

        /// Build the schema, or return the Keyset read-policy contradiction.
        ///
        /// # Panics
        ///
        /// Panics on a repeated entity identity during catalogue composition at
        /// schema build, naming both Rust types. This is not returned as a
        /// `KeysetReadPolicyError`.
        pub fn try_schema_builder(
            database: ::graphql_orm::db::Database<#backend_marker>,
        ) -> ::std::result::Result<
            __GomSchemaBuilder,
            ::graphql_orm::graphql::orm::KeysetReadPolicyError,
        > {
            try_schema_builder_with_limits(
                database,
                ::graphql_orm::graphql::orm::SchemaLimits::default(),
            )
        }

        /// Build the schema with limits, or return the Keyset read-policy contradiction.
        ///
        /// # Panics
        ///
        /// Panics on a repeated entity identity during catalogue composition at
        /// schema build, naming both Rust types. This is not returned as a
        /// `KeysetReadPolicyError`.
        /// It also panics when a handwritten root exposes a field its
        /// `GraphqlRootFieldNames` declaration omits.
        pub fn try_schema_builder_with_limits(
            database: ::graphql_orm::db::Database<#backend_marker>,
            limits: ::graphql_orm::graphql::orm::SchemaLimits,
        ) -> ::std::result::Result<
            __GomSchemaBuilder,
            ::graphql_orm::graphql::orm::KeysetReadPolicyError,
        > {
            #root_field_declaration_check
                #page_maximum_check
            database.validate_keyset_read_policies(
                graphql_orm_operation_catalog(),
                &graphql_orm_entity_metadata(),
            )?;
            let builder = ::graphql_orm::async_graphql::Schema::build(
                QueryRoot::default(),
                ::std::default::Default::default(),
                ::std::default::Default::default(),
            )
            .data(database.clone())
            .data(::graphql_orm::graphql::orm::DeclaredPageMaximum(GRAPHQL_ORM_MAX_PAGE_LIMIT))
            .extension(::graphql_orm::graphql::orm::PageMaximumValidation)
            .extension(::graphql_orm::graphql::errors::UncodedCallbackMessages(
                database.exposes_uncoded_callback_messages(),
            ))
            .extension(::graphql_orm::graphql::input_validation::GeneratedInputValidation)
            .extension(::graphql_orm::graphql::subscriptions::GeneratedSubscriptionAdmission(
                ::std::iter::IntoIterator::into_iter(
                    &[#(#entities::__GOM_SUBSCRIPTION_ROOT_FIELDS),*] as &[&[&str]],
                ).flat_map(|fields| fields.iter().copied()).collect()
            ));
            let builder = limits.apply(builder);
            #federation_enable
            #schema_auth_data
            #(#schema_loader_data)*
            Ok(builder)
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
    let graphql_name = name;
    quote! {
        #(#chunk_defs)*
        #[derive(#derive_macro, Default)]
        #[graphql(name = #graphql_name)]
        pub struct #root_name(
            #(#root_chunk_idents),*
        );
    }
}

// Keep the emitted tuple structs intact. Rust evaluates the contributing
// entities' associated constants, including for derives in other crates. The
// schema uses the original struct when any field remains, or async-graphql's
// empty root when the derives removed every generated field.
fn select_schema_root(
    name: &str,
    empty: proc_macro2::TokenStream,
    sources: &[proc_macro2::TokenStream],
    opt_outs: &[proc_macro2::TokenStream],
    has_custom_ops: bool,
) -> (proc_macro2::TokenStream, proc_macro2::TokenStream) {
    let root = quote::format_ident!("{name}Root", span = proc_macro2::Span::mixed_site());
    if has_custom_ops || sources.is_empty() {
        return (quote! {}, quote! { #root });
    }

    let selected = quote::format_ident!(
        "__GomSchema{name}Root",
        span = proc_macro2::Span::mixed_site()
    );
    let definition = quote! {
        type #selected = <::graphql_orm::graphql::orm::GeneratedRootSelection<
            {
                !(#(#opt_outs)||*)
                    || ::graphql_orm::graphql::orm::has_generated_root_fields(&[#(#sources),*])
            },
        > as ::graphql_orm::graphql::orm::SelectGeneratedRoot<#root, #empty>>::Root;
    };
    (definition, quote! { #selected })
}

struct SchemaRootsArgs {
    backend: Option<String>,
    /// Force `enable_federation()` for a subgraph with no resolvable key.
    federation: bool,
    schema_policy: Option<String>,
    auth: Option<String>,
    /// Require an explicit policy decision on every listed entity.
    require_policies: bool,
    /// Declared maximum page limit of the `Database` this schema serves.
    max_page_limit: Option<u32>,
    /// Emit `graphql_orm_client_metadata()` (#340, #670, #672).
    client_metadata: bool,
    generated_mutations: GeneratedMutationExposure,
    query_custom_ops: Vec<Ident>,
    entities: Vec<Ident>,
    extra_mutation_types: Vec<Ident>,
    extra_query_types: Vec<Ident>,
    extra_subscription_types: Vec<Ident>,
    described_query_types: Vec<Ident>,
    described_mutation_types: Vec<Ident>,
    described_subscription_types: Vec<Ident>,
    semantic_custom_operations: Vec<Ident>,
    semantic_types: Vec<Ident>,
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
        let mut federation = None;
        let mut schema_policy = None;
        let mut auth = None;
        let mut require_policies = None;
        let mut max_page_limit = None;
        let mut client_metadata = None;
        let mut generated_mutations = None;
        let mut query_custom_ops = None;
        let mut entities = None;
        let mut extra_mutation_types = None;
        let mut extra_query_types = None;
        let mut extra_subscription_types = None;
        let mut described_query_types = None;
        let mut described_mutation_types = None;
        let mut described_subscription_types = None;
        let mut semantic_custom_operations = None;
        let mut semantic_types = None;
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
            } else if label == "federation" {
                reject_duplicate(&federation, &label)?;
                let lit: syn::LitBool = input.parse()?;
                federation = Some(lit.value);
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
            } else if label == "require_policies" {
                reject_duplicate(&require_policies, &label)?;
                let lit: syn::LitBool = input.parse()?;
                require_policies = Some(lit.value);
            } else if label == "client_metadata" {
                reject_duplicate(&client_metadata, &label)?;
                let lit: syn::LitBool = input.parse()?;
                client_metadata = Some(lit.value);
            } else if label == "max_page_limit" {
                reject_duplicate(&max_page_limit, &label)?;
                let lit: syn::LitInt = input.parse()?;
                let maximum = lit.base10_parse::<u32>()?;
                if maximum == 0 || i32::try_from(maximum).is_err() {
                    return Err(syn::Error::new(
                        lit.span(),
                        "`max_page_limit` must be a positive integer that fits a GraphQL Int",
                    ));
                }
                max_page_limit = Some(maximum);
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
            } else if label == "described_query_types" {
                reject_duplicate(&described_query_types, &label)?;
                let content;
                syn::bracketed!(content in input);
                described_query_types = Some(parse_list(&content)?);
            } else if label == "described_mutation_types" {
                reject_duplicate(&described_mutation_types, &label)?;
                let content;
                syn::bracketed!(content in input);
                described_mutation_types = Some(parse_list(&content)?);
            } else if label == "described_subscription_types" {
                reject_duplicate(&described_subscription_types, &label)?;
                let content;
                syn::bracketed!(content in input);
                described_subscription_types = Some(parse_list(&content)?);
            } else if label == "semantic_custom_operations" {
                reject_duplicate(&semantic_custom_operations, &label)?;
                let content;
                syn::bracketed!(content in input);
                semantic_custom_operations = Some(parse_list(&content)?);
            } else if label == "semantic_types" {
                reject_duplicate(&semantic_types, &label)?;
                let content;
                syn::bracketed!(content in input);
                semantic_types = Some(parse_list(&content)?);
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
                    "expected one of `backend`, `schema_policy`, `auth`, `require_policies`, `max_page_limit`, `client_metadata`, `federation`, `generated_mutations`, `query_custom_ops`, `entities`, `extra_query_types`, `extra_mutation_types`, `extra_subscription_types`, `described_query_types`, `described_mutation_types`, `described_subscription_types`, `semantic_custom_operations`, `semantic_types`, `generated_mutation_allowlist`, or `generated_mutation_denylist`",
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
        let described_query_types = described_query_types.unwrap_or_default();
        let described_mutation_types = described_mutation_types.unwrap_or_default();
        let described_subscription_types = described_subscription_types.unwrap_or_default();
        let semantic_custom_operations = semantic_custom_operations.unwrap_or_default();
        let semantic_types = semantic_types.unwrap_or_default();
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

        let composed_custom_types = query_custom_ops
            .iter()
            .map(|entity| format!("{entity}CustomOperations"))
            .chain(extra_query_types.iter().map(ToString::to_string))
            .chain(extra_mutation_types.iter().map(ToString::to_string))
            .chain(extra_subscription_types.iter().map(ToString::to_string))
            .chain(described_query_types.iter().map(ToString::to_string))
            .chain(described_mutation_types.iter().map(ToString::to_string))
            .chain(described_subscription_types.iter().map(ToString::to_string))
            .collect::<std::collections::BTreeSet<_>>();
        let described_types = described_query_types
            .iter()
            .chain(&described_mutation_types)
            .chain(&described_subscription_types)
            .map(ToString::to_string)
            .collect::<std::collections::BTreeSet<_>>();
        let legacy_extra_types = extra_query_types
            .iter()
            .chain(&extra_mutation_types)
            .chain(&extra_subscription_types)
            .map(ToString::to_string)
            .collect::<std::collections::BTreeSet<_>>();
        if described_types.len()
            != described_query_types.len()
                + described_mutation_types.len()
                + described_subscription_types.len()
        {
            return Err(syn::Error::new(
                proc_macro2::Span::mixed_site(),
                "a described custom root type may be composed exactly once",
            ));
        }
        if described_types
            .iter()
            .any(|described| legacy_extra_types.contains(described))
        {
            return Err(syn::Error::new(
                proc_macro2::Span::mixed_site(),
                "described custom roots must not be repeated in an extra root list",
            ));
        }
        let mut declared_semantic_roots = std::collections::BTreeSet::new();
        for root in &semantic_custom_operations {
            if !declared_semantic_roots.insert(root.to_string()) {
                return Err(syn::Error::new(
                    root.span(),
                    "duplicate `semantic_custom_operations` entry",
                ));
            }
            if !composed_custom_types.contains(&root.to_string()) {
                return Err(syn::Error::new(
                    root.span(),
                    "`semantic_custom_operations` entries must also be composed through a custom root list",
                ));
            }
            if described_types.contains(&root.to_string()) {
                return Err(syn::Error::new(
                    root.span(),
                    "described custom roots supply semantic metadata automatically and must not be repeated in `semantic_custom_operations`",
                ));
            }
        }

        Ok(SchemaRootsArgs {
            backend,
            federation: federation.unwrap_or(false),
            schema_policy,
            auth,
            require_policies: require_policies.unwrap_or(false),
            max_page_limit,
            client_metadata: client_metadata.unwrap_or(false),
            generated_mutations,
            query_custom_ops,
            entities,
            extra_mutation_types,
            extra_query_types,
            extra_subscription_types,
            described_query_types,
            described_mutation_types,
            described_subscription_types,
            semantic_custom_operations,
            semantic_types,
            generated_mutation_allowlist,
            generated_mutation_denylist,
        })
    }
}
