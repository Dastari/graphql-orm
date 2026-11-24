use proc_macro2::TokenStream;
use quote::quote;

use super::QueryContext;
use crate::backend::{BackendKind, backend_render_identifier_path};
use crate::entity::{
    is_bool_type, is_byte_vec_type, is_string_type, is_uuid_type, option_inner_type,
    parse_field_metadata,
};
use crate::naming::graphql_field_name;
use crate::operations::{PrimaryKeyField, keyset_order_fingerprint, runtime_keyset_value};

pub(in crate::operations) type KeysetPart = (syn::Ident, syn::Type, String, bool, bool);

pub(in crate::operations) struct KeysetInput<'a> {
    pub(in crate::operations) scan_connection_type: &'a syn::Ident,
    pub(in crate::operations) keyset_operation_description: &'a str,
    pub(in crate::operations) keyset_query_skip: &'a TokenStream,
    pub(in crate::operations) keyset_query_name: &'a str,
    pub(in crate::operations) keyset_list_requires_scopes: &'a TokenStream,
    pub(in crate::operations) keyset_list_scope_enforcement: &'a TokenStream,
    pub(in crate::operations) scan_operation_description: &'a str,
    pub(in crate::operations) scan_query_skip: &'a TokenStream,
    pub(in crate::operations) scan_query_name: &'a str,
    pub(in crate::operations) primary_key_fields: &'a [PrimaryKeyField],
    pub(in crate::operations) fields: &'a syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
    pub(in crate::operations) graphql_rename_fields: Option<&'a str>,
    pub(in crate::operations) serde_rename_all: Option<&'a str>,
    pub(in crate::operations) backend: BackendKind,
    pub(in crate::operations) keyset_parts: Option<&'a [KeysetPart]>,
}

pub(in crate::operations) struct KeysetEmission {
    pub(in crate::operations) repository_methods: TokenStream,
    pub(in crate::operations) transaction_trait: TokenStream,
    pub(in crate::operations) definitions: TokenStream,
    pub(in crate::operations) resolvers: TokenStream,
}

pub(in crate::operations) fn keyset(
    context: &QueryContext<'_>,
    input: KeysetInput<'_>,
) -> syn::Result<KeysetEmission> {
    let KeysetInput {
        scan_connection_type,
        keyset_operation_description,
        keyset_query_skip,
        keyset_query_name,
        keyset_list_requires_scopes,
        keyset_list_scope_enforcement,
        scan_operation_description,
        scan_query_skip,
        scan_query_name,
        primary_key_fields,
        fields,
        graphql_rename_fields,
        serde_rename_all,
        backend,
        keyset_parts,
    } = input;
    let QueryContext {
        struct_name,
        backend_marker,
        resolver_auth_mode,
        authenticated_directive,
        rls_request_guard,
        where_input,
        where_arg_name,
        where_arg_description,
        page_arg_name,
        page_arg_description,
        order_by_arg_name,
        order_by_arg_description,
        field_case_rule,
        connection_type,
        ref filter_read_authorization,
        ..
    } = *context;
    let keyset_order_input = quote::format_ident!("{}KeysetOrderByInput", struct_name);
    let keyset_order_field = quote::format_ident!("{}OrderField", struct_name);
    let runtime_keyset_available = primary_key_fields.iter().all(|field| {
        field.meta.graphql_readable()
            && !field.meta.graphql_skip
            && field.meta.transform_read.is_none()
            && runtime_keyset_value(&field.field_type).is_some()
    });
    let keyset_generated = if let Some(parts) = keyset_parts {
        let mut runtime_fields = Vec::new();
        let mut runtime_metadata = Vec::new();
        let mut runtime_values = Vec::new();
        let mut runtime_orders = Vec::new();
        let mut runtime_ordered_arms = Vec::new();
        let mut primary_columns = Vec::new();
        for field in fields {
            let meta = parse_field_metadata(field)?;
            let ident = field.ident.as_ref().expect("named field");
            let is_primary_key = primary_key_fields
                .iter()
                .any(|key| &key.field_name == ident);
            if meta.is_relation
                || meta.skip_db
                || !meta.graphql_readable()
                || meta.graphql_skip
                || meta.transform_read.is_some()
                || !(is_primary_key || (meta.sortable && meta.order))
            {
                continue;
            }
            let Some(value_tokens) = runtime_keyset_value(&field.ty) else {
                continue;
            };
            let rust_name = ident.to_string();
            let graphql_name =
                graphql_field_name(&meta, &rust_name, graphql_rename_fields, serde_rename_all);
            let column = backend_render_identifier_path(
                backend,
                meta.db_column.as_deref().unwrap_or(&rust_name),
            );
            let policy = match meta.read_policy.as_deref() {
                Some(policy) => quote! { ::std::option::Option::Some(#policy) },
                None => quote! { ::std::option::Option::None },
            };
            runtime_fields.push(quote! {
                #[graphql(name = #graphql_name)]
                pub #ident: ::std::option::Option<::graphql_orm::graphql::orm::OrderDirection>,
            });
            runtime_metadata.push(quote! { ::graphql_orm::graphql::orm::KeysetField {
                column: #column, api_name: #graphql_name, read_policy: #policy,
            } });
            runtime_values
                .push(quote! { #column => { let value = &self.#ident; ::std::option::Option::Some(#value_tokens) } });
            runtime_orders.push(quote! {
                if let ::std::option::Option::Some(direction) = item.#ident {
                    selected += 1;
                    columns.push(match direction {
                        ::graphql_orm::graphql::orm::OrderDirection::Asc => ::graphql_orm::graphql::orm::KeysetOrderColumn::asc(#column),
                        ::graphql_orm::graphql::orm::OrderDirection::Desc => ::graphql_orm::graphql::orm::KeysetOrderColumn::desc(#column),
                    });
                }
            });
            // The typed ordered form addresses the fields that are also
            // generated order-input fields.
            if meta.sortable && meta.order {
                let variant = crate::entity::order_field_variant(
                    &crate::entity::rust_ident_from_graphql_name(&graphql_name, ident.span()),
                );
                runtime_ordered_arms.push(quote! {
                    #keyset_order_field::#variant => {
                        let mut order = Self::default();
                        order.#ident = ::std::option::Option::Some(direction);
                        order
                    }
                });
            }
            if is_primary_key {
                primary_columns.push(column);
            }
        }
        let mut order_tokens = Vec::new();
        let mut cursor_tokens = Vec::new();
        let mut cursor_field_metadata = Vec::new();
        let fingerprint = keyset_order_fingerprint(parts);
        for (field, ty, column, ascending, nulls_first) in parts {
            let quoted_column = backend_render_identifier_path(backend, column);
            let source = fields
                .iter()
                .find(|candidate| candidate.ident.as_ref() == Some(field))
                .expect("keyset field was resolved during parsing");
            let meta = parse_field_metadata(source)?;
            let api_name = graphql_field_name(
                &meta,
                &field.to_string(),
                graphql_rename_fields,
                serde_rename_all,
            );
            let graphql_readable =
                meta.graphql_readable() && !meta.graphql_skip && meta.transform_read.is_none();
            let policy = match meta.read_policy.as_deref() {
                Some(policy) => quote! { ::std::option::Option::Some(#policy) },
                None => quote! { ::std::option::Option::None },
            };
            cursor_field_metadata.push(quote! { ::graphql_orm::graphql::orm::KeysetCursorField {
                column: #quoted_column, api_name: #api_name, read_policy: #policy, graphql_readable: #graphql_readable,
            } });
            let direction = if *ascending {
                quote! { ::graphql_orm::graphql::orm::KeysetOrderColumn::asc(#quoted_column) }
            } else {
                quote! { ::graphql_orm::graphql::orm::KeysetOrderColumn::desc(#quoted_column) }
            };
            let order = if *nulls_first {
                quote! { #direction.nulls_first() }
            } else {
                direction
            };
            order_tokens.push(order);
            let (inner, optional) = option_inner_type(ty)
                .map(|inner| (inner, true))
                .unwrap_or((ty, false));
            let type_name = quote! { #inner }.to_string().replace(' ', "");
            let value = if is_string_type(inner) {
                quote! { ::graphql_orm::graphql::pagination::KeysetValue::String(value.clone()) }
            } else if is_byte_vec_type(inner) {
                quote! { ::graphql_orm::graphql::pagination::KeysetValue::Bytes(value.clone()) }
            } else if is_uuid_type(inner) {
                quote! { ::graphql_orm::graphql::pagination::KeysetValue::Uuid(value.to_string()) }
            } else if is_bool_type(inner) {
                quote! { ::graphql_orm::graphql::pagination::KeysetValue::Bool(*value) }
            } else if matches!(type_name.as_str(), "f32" | "f64") {
                quote! { ::graphql_orm::graphql::pagination::KeysetValue::Float(*value as f64) }
            } else if matches!(
                type_name.as_str(),
                "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32"
            ) {
                quote! { ::graphql_orm::graphql::pagination::KeysetValue::Int(*value as i64) }
            } else {
                return Err(syn::Error::new_spanned(
                    ty,
                    "keyset fields support String, bytes, UUID, bool, integer, and float scalar types",
                ));
            };
            cursor_tokens.push(if optional {
                quote! {
                    match entity.#field.as_ref() {
                        ::std::option::Option::Some(value) => #value,
                        ::std::option::Option::None => ::graphql_orm::graphql::pagination::KeysetValue::Null,
                    }
                }
            } else {
                quote! {{ let value = &entity.#field; #value }}
            });
        }
        // Without an orderable field there is no `OrderField` enum to take.
        let runtime_ordered_method = if runtime_ordered_arms.is_empty() {
            quote! {}
        } else {
            quote! {
                        /// Build an ordered keyset sort from typed `(field, direction)`
                        /// pairs: the first pair is the primary sort key, and the cursor's
                        /// key order follows the list. The complete primary key is still
                        /// appended as the tiebreak. A field that cannot be a keyset key
                        /// is refused with `INVALID_INPUT`.
                        #[allow(unreachable_patterns)]
                        pub fn ordered(
                            fields: impl ::std::iter::IntoIterator<Item = (#keyset_order_field, ::graphql_orm::graphql::orm::OrderDirection)>,
                        ) -> ::std::result::Result<::std::vec::Vec<Self>, ::graphql_orm::graphql::errors::OrmPublicError> {
                            fields
                                .into_iter()
                                .map(|(field, direction)| {
                                    ::std::result::Result::Ok(match field {
                                        #(#runtime_ordered_arms)*
                                        _ => {
                                            return ::std::result::Result::Err(::graphql_orm::graphql::errors::OrmPublicError::with_message(
                                                ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                                                "this field cannot be a keyset sort key",
                                            ));
                                        }
                                    })
                                })
                                .collect()
                        }
            }
        };
        let runtime_order_definitions = if runtime_keyset_available {
            quote! {
                    /// Scalar-only runtime keyset order. Each list entry must select
                    /// exactly one field; NULL values sort last in either direction.
                    #[derive(::graphql_orm::async_graphql::InputObject, Default, Clone, Debug)]
                    pub struct #keyset_order_input { #(#runtime_fields)* }
                    impl #keyset_order_input {
                        #runtime_ordered_method

                        fn __gom_columns(items: ::std::vec::Vec<Self>) -> ::graphql_orm::async_graphql::Result<::std::vec::Vec<::graphql_orm::graphql::orm::KeysetOrderColumn>> {
                            let mut columns = ::std::vec::Vec::new();
                            for item in items {
                                let mut selected = 0;
                                #(#runtime_orders)*
                                if selected != 1 {
                                    return ::std::result::Result::Err(::graphql_orm::graphql::errors::OrmPublicError::new(
                                        ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput
                                    ).into_graphql_error());
                                }
                            }
                            ::std::result::Result::Ok(columns)
                        }
                    }

            }
        } else {
            quote! {}
        };
        let runtime_order_trait_items = if runtime_keyset_available {
            quote! {
                        const KEYSET_FIELDS: &'static [::graphql_orm::graphql::orm::KeysetField] = &[#(#runtime_metadata),*];
                        const KEYSET_PRIMARY_COLUMNS: &'static [&'static str] = &[#(#primary_columns),*];
                        fn keyset_value_for_column(&self, column: &str) -> ::std::option::Option<::graphql_orm::graphql::pagination::KeysetValue> {
                            match column { #(#runtime_values),*, _ => ::std::option::Option::None }
                        }

            }
        } else {
            quote! {}
        };
        let runtime_order_repository_methods = if runtime_keyset_available {
            quote! {
                    /// Fetch a bounded page with optional scalar ordering and a complete primary-key suffix.
                    pub async fn keyset_page_ordered(
                        db: &::graphql_orm::db::Database<#backend_marker>,
                        filter: #where_input,
                        order_by: ::std::option::Option<::std::vec::Vec<#keyset_order_input>>,
                        page: ::graphql_orm::graphql::pagination::KeysetPageInput,
                    ) -> ::graphql_orm::async_graphql::Result<::graphql_orm::graphql::pagination::Connection<Self>> {
                        let order = order_by.map(#keyset_order_input::__gom_columns).transpose()?;
                        ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                            .filter_with_entity_matching(&filter)
                            .fetch_authorized_keyset_ordered(db, ::std::option::Option::None, ::std::option::Option::None, page, order).await
                    }

                    /// Fetch a forward or backward page with optional scalar ordering.
                    pub async fn keyset_connection_page_ordered(
                        db: &::graphql_orm::db::Database<#backend_marker>,
                        filter: #where_input,
                        order_by: ::std::option::Option<::std::vec::Vec<#keyset_order_input>>,
                        page: ::graphql_orm::graphql::pagination::KeysetConnectionInput,
                    ) -> ::graphql_orm::async_graphql::Result<::graphql_orm::graphql::pagination::Connection<Self>> {
                        let order = order_by.map(#keyset_order_input::__gom_columns).transpose()?;
                        ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                            .filter_with_entity_matching(&filter)
                            .fetch_authorized_keyset_connection_with_order(db, ::std::option::Option::None, ::std::option::Option::None, page, order).await
                    }

            }
        } else {
            quote! {}
        };
        let scan_page_object = crate::wrapper_object::wrapper_object(quote! {
            #[derive(Clone, Debug)]
            #[graphql(rename_fields = #field_case_rule)]
            pub struct #scan_connection_type {
                pub nodes: ::std::vec::Vec<#struct_name>,
                pub page_info: ::graphql_orm::graphql::orm::ScanPageInfo,
            }
        });
        let definitions = quote! {
                #scan_page_object

                #runtime_order_definitions

                impl ::graphql_orm::graphql::orm::AuthorizedKeysetEntity for #struct_name {
                    #runtime_order_trait_items
                    const KEYSET_CURSOR_FIELDS: &'static [::graphql_orm::graphql::orm::KeysetCursorField] = &[#(#cursor_field_metadata),*];
                    const KEYSET_ORDER: &'static [::graphql_orm::graphql::orm::KeysetOrderColumn] = &[#(#order_tokens),*];
                    const KEYSET_FINGERPRINT: &'static str = #fingerprint;
                    fn keyset_values(&self) -> ::std::vec::Vec<::graphql_orm::graphql::pagination::KeysetValue> {
                        let entity = self;
                        ::std::vec![#(#cursor_tokens),*]
                    }
                }

        };
        (
            quote! {
                /// Scan bounded batches with request-independent repository policies.
                /// Install AuthorizedScanConfig on the database to opt in.
                pub async fn authorized_scan(
                    db: &::graphql_orm::db::Database<#backend_marker>,
                    filter: #where_input,
                    page: ::graphql_orm::graphql::orm::AuthorizedScanInput,
                ) -> ::graphql_orm::async_graphql::Result<::graphql_orm::graphql::orm::AuthorizedScanPage<Self>> {
                    ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                        .filter_with_entity_matching(&filter)
                        .fetch_authorized_scan(db, ::std::option::Option::None, ::std::option::Option::None, page).await
                }

                #runtime_order_repository_methods

                /// Fetch a bounded, stable keyset page using the entity's configured order.
                pub async fn keyset_page(
                    db: &::graphql_orm::db::Database<#backend_marker>,
                    filter: #where_input,
                    page: ::graphql_orm::graphql::pagination::KeysetPageInput,
                ) -> ::std::result::Result<::graphql_orm::graphql::pagination::Connection<Self>, ::graphql_orm::graphql::errors::OrmPublicError> {
                    ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                        .filter_with_entity_matching(&filter)
                        .fetch_authorized_keyset(db, ::std::option::Option::None, ::std::option::Option::None, page).await
                        .map_err(|error| ::graphql_orm::graphql::errors::OrmPublicError::from(::graphql_orm::graphql::errors::sqlx_error_from_graphql(error)))
                }

                /// Fetch a bounded forward or backward keyset connection using
                /// the entity's configured canonical order.
                pub async fn keyset_connection_page(
                    db: &::graphql_orm::db::Database<#backend_marker>,
                    filter: #where_input,
                    page: ::graphql_orm::graphql::pagination::KeysetConnectionInput,
                ) -> ::std::result::Result<::graphql_orm::graphql::pagination::Connection<Self>, ::graphql_orm::graphql::errors::OrmPublicError> {
                    ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                        .filter_with_entity_matching(&filter)
                        .fetch_authorized_keyset_connection(db, ::std::option::Option::None, ::std::option::Option::None, page).await
                        .map_err(|error| ::graphql_orm::graphql::errors::OrmPublicError::from(::graphql_orm::graphql::errors::sqlx_error_from_graphql(error)))
                }
            },
            quote! {
                impl ::graphql_orm::graphql::orm::MutationContextKeysetPage<#backend_marker> for #struct_name {
                    type Filter = #where_input;

                    fn keyset_page_in_mutation_context<'a>(
                        hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                        filter: Self::Filter,
                        page: ::graphql_orm::graphql::pagination::KeysetPageInput,
                    ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::std::result::Result<::graphql_orm::graphql::pagination::Connection<Self>, ::graphql_orm::graphql::errors::OrmPublicError>> {
                        ::std::boxed::Box::pin(async move {
                            use ::graphql_orm::graphql::orm::DatabaseFilter;
                            const ORDER: &[::graphql_orm::graphql::orm::KeysetOrderColumn] = &[
                                #(#order_tokens),*
                            ];
                            const FINGERPRINT: &str = #fingerprint;
                            let bound_fingerprint = ::graphql_orm::graphql::orm::bind_keyset_fingerprint(
                                ::std::option::Option::Some(<Self as ::graphql_orm::graphql::orm::Entity>::entity_name()),
                                FINGERPRINT,
                                &filter.keyset_filter_identity().into_iter().collect::<::std::vec::Vec<_>>(),
                            );
                            if filter.requires_in_memory_filtering(<#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT) {
                                return ::std::result::Result::Err(::graphql_orm::graphql::errors::OrmPublicError::new(
                                    ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                                ));
                            }
                            let config = hook_ctx.database().pagination_config();
                            // A transaction read is trusted Rust code: its limit is exact or refused.
                            if let ::std::option::Option::Some(requested) = page.limit {
                                config.checked_explicit_limit(requested)?;
                            }
                            let resolved = config.resolve_page(
                                ::std::option::Option::Some(&::graphql_orm::graphql::orm::PageInput { limit: page.limit, offset: ::std::option::Option::Some(0) }),
                                true,
                            );
                            let limit = resolved.limit.unwrap_or(::graphql_orm::graphql::orm::PaginationConfig::DEFAULT_LIMIT).max(0);
                            let mut query = ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new().filter(&filter);
                            let total_count = if page.include_total_count {
                                ::std::option::Option::Some(query.clone().count_in_transaction(hook_ctx).await.map_err(::graphql_orm::graphql::errors::OrmPublicError::from)?)
                            } else {
                                ::std::option::Option::None
                            };
                            if let ::std::option::Option::Some(cursor) = page.after.as_deref() {
                                let values = ::graphql_orm::graphql::pagination::decode_keyset_cursor(cursor, &bound_fingerprint, ORDER.len())?;
                                let (predicate, binds) = ::graphql_orm::graphql::orm::render_keyset_after(
                                    <#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT,
                                    ORDER,
                                    &values,
                                    query.values.len() + 1,
                                )?;
                                query = query.where_values(&predicate, binds);
                            }
                            query.order_clauses = ORDER.iter().map(|column| column.order_sql()).collect();
                            query = query.paginate(&::graphql_orm::graphql::orm::PageInput {
                                limit: ::std::option::Option::Some(limit.saturating_add(1)),
                                offset: ::std::option::Option::Some(0),
                            });
                            let mut entities = query.fetch_all_in_transaction(hook_ctx).await.map_err(::graphql_orm::graphql::errors::OrmPublicError::from)?;
                            let has_next_page = entities.len() > limit as usize;
                            if has_next_page { entities.truncate(limit as usize); }
                            let edges = entities
                                .into_iter()
                                .map(|entity| {
                                    let values = ::std::vec![#(#cursor_tokens),*];
                                    ::graphql_orm::graphql::pagination::Edge {
                                        cursor: ::graphql_orm::graphql::pagination::encode_keyset_cursor(&bound_fingerprint, values),
                                        node: entity,
                                    }
                                })
                                .collect::<::std::vec::Vec<_>>();
                            ::std::result::Result::Ok(::graphql_orm::graphql::pagination::Connection {
                                page_info: ::graphql_orm::graphql::pagination::PageInfo {
                                    has_next_page,
                                    has_previous_page: page.after.is_some(),
                                    start_cursor: edges.first().map(|edge| edge.cursor.clone()),
                                    end_cursor: edges.last().map(|edge| edge.cursor.clone()),
                                    total_count,
                                },
                                edges,
                            })
                        })
                    }
                }

                impl ::graphql_orm::graphql::orm::MutationContextKeysetConnectionPage<#backend_marker> for #struct_name {
                    type Filter = #where_input;

                    fn keyset_connection_page_in_mutation_context<'a>(
                        hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                        filter: Self::Filter,
                        page: ::graphql_orm::graphql::pagination::KeysetConnectionInput,
                    ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::std::result::Result<::graphql_orm::graphql::pagination::Connection<Self>, ::graphql_orm::graphql::errors::OrmPublicError>> {
                        ::std::boxed::Box::pin(async move {
                            use ::graphql_orm::graphql::orm::DatabaseFilter;
                            const ORDER: &[::graphql_orm::graphql::orm::KeysetOrderColumn] = &[
                                #(#order_tokens),*
                            ];
                            const FINGERPRINT: &str = #fingerprint;
                            let bound_fingerprint = ::graphql_orm::graphql::orm::bind_keyset_fingerprint(
                                ::std::option::Option::Some(<Self as ::graphql_orm::graphql::orm::Entity>::entity_name()),
                                FINGERPRINT,
                                &filter.keyset_filter_identity().into_iter().collect::<::std::vec::Vec<_>>(),
                            );
                            if filter.requires_in_memory_filtering(<#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT) {
                                return ::std::result::Result::Err(::graphql_orm::graphql::errors::OrmPublicError::new(
                                    ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                                ));
                            }
                            let requested = page.first.or(page.last);
                            let validated = page.validate(
                                ::graphql_orm::graphql::orm::PaginationConfig::DEFAULT_LIMIT,
                                i64::MAX,
                            )?;
                            let config = hook_ctx.database().pagination_config();
                            // A transaction read is trusted Rust code: its limit is exact or refused.
                            if let ::std::option::Option::Some(requested) = requested {
                                config.checked_explicit_limit(requested)?;
                            }
                            let resolved = config.resolve_page(
                                ::std::option::Option::Some(&::graphql_orm::graphql::orm::PageInput {
                                    limit: ::std::option::Option::Some(validated.limit),
                                    offset: ::std::option::Option::Some(0),
                                }),
                                true,
                            );
                            let limit = resolved
                                .limit
                                .unwrap_or(::graphql_orm::graphql::orm::PaginationConfig::DEFAULT_LIMIT)
                                .max(1);
                            let backward = validated.direction
                                == ::graphql_orm::graphql::pagination::KeysetWindowDirection::Backward;
                            let had_cursor = validated.cursor.is_some();
                            let mut query = ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new().filter(&filter);
                            let total_count = if validated.include_total_count {
                                ::std::option::Option::Some(query.clone().count_in_transaction(hook_ctx).await.map_err(::graphql_orm::graphql::errors::OrmPublicError::from)?)
                            } else {
                                ::std::option::Option::None
                            };
                            if let ::std::option::Option::Some(cursor) = validated.cursor.as_deref() {
                                let values = ::graphql_orm::graphql::pagination::decode_keyset_cursor(cursor, &bound_fingerprint, ORDER.len())?;
                                let (predicate, binds) = if backward {
                                    ::graphql_orm::graphql::orm::render_keyset_before(
                                        <#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT,
                                        ORDER,
                                        &values,
                                        query.values.len() + 1,
                                    )?
                                } else {
                                    ::graphql_orm::graphql::orm::render_keyset_after(
                                        <#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT,
                                        ORDER,
                                        &values,
                                        query.values.len() + 1,
                                    )?
                                };
                                query = query.where_values(&predicate, binds);
                            }
                            query.order_clauses = if backward {
                                ORDER.iter().map(|column| column.reversed().order_sql()).collect()
                            } else {
                                ORDER.iter().map(|column| column.order_sql()).collect()
                            };
                            query = query.paginate(&::graphql_orm::graphql::orm::PageInput {
                                limit: ::std::option::Option::Some(limit.saturating_add(1)),
                                offset: ::std::option::Option::Some(0),
                            });
                            let mut entities = query.fetch_all_in_transaction(hook_ctx).await.map_err(::graphql_orm::graphql::errors::OrmPublicError::from)?;
                            let has_more = entities.len() > limit as usize;
                            if has_more {
                                entities.truncate(limit as usize);
                            }
                            if backward {
                                entities.reverse();
                            }
                            let edges = entities
                                .into_iter()
                                .map(|entity| {
                                    let values = ::std::vec![#(#cursor_tokens),*];
                                    ::graphql_orm::graphql::pagination::Edge {
                                        cursor: ::graphql_orm::graphql::pagination::encode_keyset_cursor(&bound_fingerprint, values),
                                        node: entity,
                                    }
                                })
                                .collect::<::std::vec::Vec<_>>();
                            ::std::result::Result::Ok(::graphql_orm::graphql::pagination::Connection {
                                page_info: ::graphql_orm::graphql::pagination::PageInfo {
                                    has_next_page: if backward { had_cursor } else { has_more },
                                    has_previous_page: if backward { has_more } else { had_cursor },
                                    start_cursor: edges.first().map(|edge| edge.cursor.clone()),
                                    end_cursor: edges.last().map(|edge| edge.cursor.clone()),
                                    total_count,
                                },
                                edges,
                            })
                        })
                    }
                }
            },
            definitions,
        )
    } else {
        (quote! {}, quote! {}, quote! {})
    };
    let (keyset_repository_methods, keyset_trait_impl, authorized_keyset_definitions) =
        keyset_generated;
    let runtime_keyset_argument = if runtime_keyset_available {
        quote! {
            #[__gom_catalogue_type(Option<Vec<#keyset_order_input>>)]
            #[graphql(name = #order_by_arg_name, desc = #order_by_arg_description)] order_by: ::std::option::Option<::std::vec::Vec<#keyset_order_input>>,
        }
    } else {
        quote! {}
    };
    let runtime_keyset_prepare = if runtime_keyset_available {
        quote! {
            let order = order_by.map(#keyset_order_input::__gom_columns).transpose()?;
        }
    } else {
        quote! {}
    };
    let runtime_keyset_fetch = if runtime_keyset_available {
        quote! {
            .fetch_authorized_keyset_ordered(db, ::std::option::Option::Some(ctx), auth, page, order).await?
        }
    } else {
        quote! {
            .fetch_authorized_keyset(db, ::std::option::Option::Some(ctx), auth, page).await?
        }
    };
    let keyset_graphql_method = if keyset_parts.is_some() {
        quote! {
            #[doc = #keyset_operation_description]
            #[graphql(#keyset_query_skip name = #keyset_query_name, #keyset_list_requires_scopes #authenticated_directive)]
            async fn keyset(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[__gom_catalogue_type(Option<#where_input>)]
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: ::std::option::Option<#where_input>,
                #runtime_keyset_argument
                #[graphql(name = #page_arg_name, desc = #page_arg_description)] page: ::graphql_orm::graphql::pagination::KeysetPageInput,
            ) -> ::graphql_orm::async_graphql::Result<#connection_type> {
                ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #keyset_list_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                )?;
                let db = ::graphql_orm::db::request_database::<#backend_marker>(ctx)?;
                let auth = ctx.data_opt::<::graphql_orm::graphql::orm::DbAuthContext>();
                #filter_read_authorization
                let filter = where_input.unwrap_or_default();
                #runtime_keyset_prepare
                let connection = ::graphql_orm::graphql::orm::EntityQuery::<#struct_name, #backend_marker>::new()
                    .filter_with_entity_matching(&filter)
                    #runtime_keyset_fetch;
                ::std::result::Result::Ok(#connection_type::from_generic(connection))
            }

            #[doc = #scan_operation_description]
            #[graphql(#scan_query_skip name = #scan_query_name, #keyset_list_requires_scopes #authenticated_directive)]
            async fn authorized_scan(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[__gom_catalogue_type(Option<#where_input>)]
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: ::std::option::Option<#where_input>,
                #[graphql(name = #page_arg_name, desc = #page_arg_description)] page: ::graphql_orm::graphql::orm::AuthorizedScanInput,
            ) -> ::graphql_orm::async_graphql::Result<#scan_connection_type> {
                ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #keyset_list_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx, ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                )?;
                let db = ::graphql_orm::db::request_database::<#backend_marker>(ctx)?;
                let auth = ctx.data_opt::<::graphql_orm::graphql::orm::DbAuthContext>();
                #filter_read_authorization
                let result = ::graphql_orm::graphql::orm::EntityQuery::<#struct_name, #backend_marker>::new()
                    .filter_with_entity_matching(&where_input.unwrap_or_default())
                    .fetch_authorized_scan(db, ::std::option::Option::Some(ctx), auth, page).await?;
                ::std::result::Result::Ok(#scan_connection_type { nodes: result.nodes, page_info: result.page_info })
            }
        }
    } else {
        quote! {}
    };

    Ok(KeysetEmission {
        repository_methods: keyset_repository_methods,
        transaction_trait: keyset_trait_impl,
        definitions: authorized_keyset_definitions,
        resolvers: keyset_graphql_method,
    })
}
