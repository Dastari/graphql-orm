use proc_macro2::TokenStream;
use quote::quote;

use super::{PrimaryKeyField, keyset_order_fingerprint, runtime_keyset_value};
use crate::backend::{BackendKind, backend_render_identifier_path};
use crate::entity::{
    EntityMetadata, is_bool_type, is_byte_vec_type, is_string_type, is_uuid_type,
    option_inner_type, parse_field_metadata,
};
use crate::naming::graphql_field_name;

pub(super) type KeysetPart = (syn::Ident, syn::Type, String, bool, bool);

pub(super) struct QueryContext<'a> {
    pub(super) struct_name: &'a syn::Ident,
    pub(super) backend_marker: &'a TokenStream,
    pub(super) resolver_auth_mode: &'a TokenStream,
    pub(super) authenticated_directive: &'a TokenStream,
    pub(super) rls_request_guard: &'a TokenStream,
    pub(super) entity_name_lit: &'a str,
    pub(super) where_input: &'a syn::Ident,
    pub(super) where_arg_name: &'a str,
    pub(super) where_arg_description: &'a str,
    pub(super) page_arg_name: &'a str,
    pub(super) page_arg_description: &'a str,
    pub(super) order_by_arg_name: &'a str,
    pub(super) order_by_arg_description: &'a str,
    pub(super) field_case_rule: &'a str,
    pub(super) page_info_field_name: &'a str,
    pub(super) connection_type: &'a syn::Ident,
    pub(super) filter_read_authorization: TokenStream,
}

pub(super) fn filter_read_authorization(
    struct_name: &syn::Ident,
    backend_marker: &TokenStream,
) -> TokenStream {
    // Filtering reveals field information even when the response selects only
    // unrelated fields. Authorize original recursive predicate intent before
    // any list/count/scan SQL, using the same public identities as field reads.
    quote! {
        let mut where_input = where_input;
        // One budget per statement: where-input nodes, and the relation
        // policy SQL that relation filters and count orderings add.
        let mut __gom_statement_budget = ::graphql_orm::graphql::orm::RelationFilterBudget::new();
        if let ::std::option::Option::Some(filter) = where_input.as_mut() {
            filter.__gom_prepare_read(ctx, 0, &mut __gom_statement_budget).await?;
        }

        if let ::std::option::Option::Some(filter) = where_input.as_ref() {
            ::graphql_orm::graphql::orm::authorize_read_fields::<#struct_name, #backend_marker>(
                db, ctx, ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter),
            ).await?;
        }
    }
}

pub(super) struct KeysetInput<'a> {
    pub(super) scan_connection_type: &'a syn::Ident,
    pub(super) keyset_operation_description: &'a str,
    pub(super) keyset_query_skip: &'a TokenStream,
    pub(super) keyset_query_name: &'a str,
    pub(super) keyset_list_requires_scopes: &'a TokenStream,
    pub(super) keyset_list_scope_enforcement: &'a TokenStream,
    pub(super) scan_operation_description: &'a str,
    pub(super) scan_query_skip: &'a TokenStream,
    pub(super) scan_query_name: &'a str,
    pub(super) primary_key_fields: &'a [PrimaryKeyField],
    pub(super) fields: &'a syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
    pub(super) graphql_rename_fields: Option<&'a str>,
    pub(super) serde_rename_all: Option<&'a str>,
    pub(super) backend: BackendKind,
    pub(super) keyset_parts: Option<&'a [KeysetPart]>,
}

pub(super) struct KeysetEmission {
    pub(super) repository_methods: TokenStream,
    pub(super) transaction_trait: TokenStream,
    pub(super) definitions: TokenStream,
    pub(super) resolvers: TokenStream,
}

pub(super) fn keyset(
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
                let db = ctx.data::<::graphql_orm::db::Database<#backend_marker>>()?;
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
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: ::std::option::Option<#where_input>,
                #[graphql(name = #page_arg_name, desc = #page_arg_description)] page: ::graphql_orm::graphql::orm::AuthorizedScanInput,
            ) -> ::graphql_orm::async_graphql::Result<#scan_connection_type> {
                ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #keyset_list_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx, ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                )?;
                let db = ctx.data::<::graphql_orm::db::Database<#backend_marker>>()?;
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

pub(super) struct SearchInput<'a> {
    pub(super) search_edge_type_str: &'a str,
    pub(super) search_edge_type: &'a syn::Ident,
    pub(super) search_connection_type_str: &'a str,
    pub(super) search_connection_type: &'a syn::Ident,
    pub(super) search_operation_description: &'a str,
    pub(super) search_query_skip: &'a TokenStream,
    pub(super) search_query_name: &'a str,
    pub(super) search_requires_scopes: &'a TokenStream,
    pub(super) search_arg_name: &'a str,
    pub(super) search_arg_description: &'a str,
    pub(super) search_scope_enforcement: &'a TokenStream,
    pub(super) pool_type: &'a TokenStream,
    pub(super) has_search: bool,
}

pub(super) fn search(
    context: &QueryContext<'_>,
    input: SearchInput<'_>,
) -> (TokenStream, TokenStream, TokenStream) {
    let SearchInput {
        search_edge_type_str,
        search_edge_type,
        search_connection_type_str,
        search_connection_type,
        search_operation_description,
        search_query_skip,
        search_query_name,
        search_requires_scopes,
        search_arg_name,
        search_arg_description,
        search_scope_enforcement,
        pool_type,
        has_search,
    } = input;
    let QueryContext {
        struct_name,
        backend_marker,
        resolver_auth_mode,
        authenticated_directive,
        rls_request_guard,
        entity_name_lit,
        where_input,
        where_arg_name,
        where_arg_description,
        page_arg_name,
        page_arg_description,
        field_case_rule,
        page_info_field_name,
        ref filter_read_authorization,
        ..
    } = *context;
    let search_connection_definitions = if has_search {
        quote! {
            #[derive(::graphql_orm::async_graphql::SimpleObject, Debug, Clone)]
            #[graphql(name = #search_edge_type_str, rename_fields = #field_case_rule)]
            pub struct #search_edge_type {
                pub node: #struct_name,
                pub cursor: ::std::string::String,
                pub score: f64,
            }

            #[derive(::graphql_orm::async_graphql::SimpleObject, Debug, Clone)]
            #[graphql(name = #search_connection_type_str, rename_fields = #field_case_rule)]
            pub struct #search_connection_type {
                pub edges: ::std::vec::Vec<#search_edge_type>,
                #[graphql(name = #page_info_field_name)]
                pub page_info: ::graphql_orm::graphql::pagination::PageInfo,
            }

            impl #search_connection_type {
                pub fn from_generic(conn: ::graphql_orm::graphql::orm::SearchConnection<#struct_name>) -> Self {
                    Self {
                        edges: conn.edges.into_iter().map(|edge| #search_edge_type {
                            node: edge.node,
                            cursor: edge.cursor,
                            score: edge.score,
                        }).collect(),
                        page_info: conn.page_info,
                    }
                }
            }
        }
    } else {
        quote! {}
    };

    let search_query_method = if has_search {
        quote! {
            #[doc = #search_operation_description]
            #[graphql(#search_query_skip name = #search_query_name, #search_requires_scopes #authenticated_directive)]
            async fn search(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #search_arg_name, desc = #search_arg_description)] search: ::graphql_orm::graphql::filters::SearchInput,
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: ::std::option::Option<#where_input>,
                #[graphql(name = #page_arg_name, desc = #page_arg_description)] page: ::std::option::Option<::graphql_orm::graphql::orm::PageInput>,
            ) -> ::graphql_orm::async_graphql::Result<#search_connection_type> {
                ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #search_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                )?;
                let db = ctx.data::<::graphql_orm::db::Database<#backend_marker>>()?;
                let pool = db.pool();
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                db.ensure_entity_access(
                    ::std::option::Option::Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                #filter_read_authorization

                let mut query = #struct_name::search(pool, search)
                    .client_pagination_config(db.client_pagination_config());
                if let ::std::option::Option::Some(ref filter) = where_input {
                    query = query.filter(filter.clone());
                }
                query = query.default_order();

                let generic_conn = query
                    .fetch_authorized_connection(db, ctx, auth_context.as_ref(), page)
                    .await?;

                ::std::result::Result::Ok(#search_connection_type::from_generic(generic_conn))
            }
        }
    } else {
        quote! {}
    };

    let search_repository_method = if has_search {
        quote! {
            /// Build a trusted, unchecked search over a raw pool.
            ///
            /// This builder has no request and no policy provider: it applies
            /// no entity, row, field or search-source authorization. Use
            /// `search_db` for policy-aware searches.
            pub fn search<'a>(
                pool: &'a #pool_type,
                search: ::graphql_orm::graphql::filters::SearchInput,
            ) -> ::graphql_orm::graphql::orm::EntitySearchQuery<'a, Self, #where_input, #backend_marker> {
                ::graphql_orm::graphql::orm::EntitySearchQuery::new(pool, search)
            }

            /// Build a policy-aware search from a `Database` handle.
            ///
            /// The search authorizes entity access and every search source
            /// before any SQL, and row and field policies on every hit, like
            /// the other repository reads. `search(pool, …)` is the trusted,
            /// unchecked builder.
            pub fn search_db<'a>(
                db: &'a ::graphql_orm::db::Database<#backend_marker>,
                search: ::graphql_orm::graphql::filters::SearchInput,
            ) -> ::graphql_orm::graphql::orm::RepositorySearchQuery<'a, Self, #where_input, #backend_marker> {
                ::graphql_orm::graphql::orm::RepositorySearchQuery::new(db, search)
            }
        }
    } else {
        quote! {}
    };
    (
        search_connection_definitions,
        search_query_method,
        search_repository_method,
    )
}

pub(super) struct AggregateInput<'a> {
    pub(super) source_entity_field_name: &'a str,
    pub(super) aggregate_metric_input: &'a syn::Ident,
    pub(super) aggregate_field: &'a syn::Ident,
    pub(super) aggregate_operation_description: &'a str,
    pub(super) aggregate_query_skip: &'a TokenStream,
    pub(super) aggregate_query_name: &'a str,
    pub(super) aggregate_requires_scopes: &'a TokenStream,
    pub(super) group_by_arg_name: &'a str,
    pub(super) group_by_arg_description: &'a str,
    pub(super) metrics_arg_name: &'a str,
    pub(super) metrics_arg_description: &'a str,
    pub(super) group_limit_arg_name: &'a str,
    pub(super) group_limit_arg_description: &'a str,
    pub(super) aggregate_scope_enforcement: &'a TokenStream,
    pub(super) entity_meta: &'a EntityMetadata,
}

pub(super) fn aggregate(
    context: &QueryContext<'_>,
    input: AggregateInput<'_>,
) -> (TokenStream, TokenStream, TokenStream) {
    let AggregateInput {
        source_entity_field_name,
        aggregate_metric_input,
        aggregate_field,
        aggregate_operation_description,
        aggregate_query_skip,
        aggregate_query_name,
        aggregate_requires_scopes,
        group_by_arg_name,
        group_by_arg_description,
        metrics_arg_name,
        metrics_arg_description,
        group_limit_arg_name,
        group_limit_arg_description,
        aggregate_scope_enforcement,
        entity_meta,
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
        field_case_rule,
        ref filter_read_authorization,
        ..
    } = *context;
    // The optional source field retains the entity in a pulled schema.
    let aggregate_row = syn::Ident::new(
        &format!("{}AggregateResultRow", struct_name),
        struct_name.span(),
    );
    let aggregate_row_str = aggregate_row.to_string();
    let aggregate_row_type = if entity_meta.aggregate_source {
        quote! { #aggregate_row }
    } else {
        quote! { ::graphql_orm::graphql::orm::GraphqlAggregateResultRow }
    };
    let aggregate_rows_value = if entity_meta.aggregate_source {
        quote! {
            rows.into_iter()
                .map(|row| #aggregate_row::from(::graphql_orm::graphql::orm::GraphqlAggregateResultRow::from(row)))
                .collect()
        }
    } else {
        quote! { rows.into_iter().map(::std::convert::Into::into).collect() }
    };
    let aggregate_row_object = if entity_meta.aggregate_source {
        let object = crate::wrapper_object::wrapper_object(quote! {
            /// One generated aggregate result group of this entity.
            #[derive(Clone, Debug)]
            #[graphql(name = #aggregate_row_str, rename_fields = #field_case_rule)]
            pub struct #aggregate_row {
                /// Group keys in requested order.
                pub groups: ::std::vec::Vec<::graphql_orm::graphql::orm::GraphqlAggregateResultValue>,
                /// Metrics in requested order.
                pub metrics: ::std::vec::Vec<::graphql_orm::graphql::orm::GraphqlAggregateResultValue>,
                /// Always null. The field's type is the entity this aggregate reads.
                #[graphql(name = #source_entity_field_name)]
                pub source_entity: ::std::option::Option<#struct_name>,
            }
        });
        quote! {
            #object

            impl ::std::convert::From<::graphql_orm::graphql::orm::GraphqlAggregateResultRow> for #aggregate_row {
                fn from(row: ::graphql_orm::graphql::orm::GraphqlAggregateResultRow) -> Self {
                    Self { groups: row.groups, metrics: row.metrics, source_entity: ::std::option::Option::None }
                }
            }
        }
    } else {
        quote! {}
    };
    let aggregate_definitions = if entity_meta.aggregate {
        quote! {
            #aggregate_row_object

            /// One reviewed aggregate expression for this entity.
            #[derive(Clone, Debug, ::graphql_orm::async_graphql::InputObject)]
            #[graphql(rename_fields = #field_case_rule)]
            pub struct #aggregate_metric_input {
                /// Canonical aggregate operator.
                pub operator: ::graphql_orm::graphql::orm::GraphqlAggregateInputOperator,
                /// Selected public field. Omit only for row count.
                pub field: ::std::option::Option<#aggregate_field>,
            }
        }
    } else {
        quote! {}
    };
    let aggregate_query_method = if entity_meta.aggregate {
        quote! {
            #[doc = #aggregate_operation_description]
            #[graphql(#aggregate_query_skip name = #aggregate_query_name, #aggregate_requires_scopes #authenticated_directive)]
            async fn aggregate(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: ::std::option::Option<#where_input>,
                #[graphql(name = #group_by_arg_name, desc = #group_by_arg_description)] group_by: ::std::vec::Vec<#aggregate_field>,
                #[graphql(name = #metrics_arg_name, desc = #metrics_arg_description)] metrics: ::std::vec::Vec<#aggregate_metric_input>,
                #[graphql(name = #group_limit_arg_name, desc = #group_limit_arg_description)] group_limit: i32,
            ) -> ::graphql_orm::async_graphql::Result<::std::vec::Vec<#aggregate_row_type>> {
                ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #aggregate_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                )?;
                let db = ctx.data::<::graphql_orm::db::Database<#backend_marker>>()?;
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                #filter_read_authorization
                let mut query = #struct_name::aggregate(db).with_auth(auth_context.as_ref());
                if let ::std::option::Option::Some(filter) = where_input {
                    query = query.filter(filter);
                }
                for field in group_by {
                    query = query.group_by(field).map_err(
                        ::graphql_orm::graphql::errors::graphql_error_from_sqlx
                    )?;
                }
                for metric in metrics {
                    use ::graphql_orm::graphql::orm::GraphqlAggregateInputOperator as Operator;
                    query = match (metric.operator, metric.field) {
                        (Operator::Count, ::std::option::Option::Some(field)) => query.count(field),
                        (Operator::Count, ::std::option::Option::None) => query.count_rows(),
                        (Operator::Min, ::std::option::Option::Some(field)) => query.min(field),
                        (Operator::Max, ::std::option::Option::Some(field)) => query.max(field),
                        (Operator::Sum, ::std::option::Option::Some(field)) => query.sum(field),
                        (_, ::std::option::Option::None) => ::std::result::Result::Err(::graphql_orm::sqlx::Error::Protocol(
                            "aggregate metric field is required for this operator".to_owned()
                        )),
                    }.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                }
                query = query.group_limit(i64::from(group_limit)).map_err(
                    ::graphql_orm::graphql::errors::graphql_error_from_sqlx
                )?;
                let rows = query.fetch_with_graphql_context(ctx).await.map_err(
                    ::graphql_orm::graphql::errors::graphql_error_from_sqlx
                )?;
                ::std::result::Result::Ok(#aggregate_rows_value)
            }
        }
    } else {
        quote! {}
    };
    (
        aggregate_definitions,
        aggregate_query_method,
        aggregate_row_type,
    )
}

pub(super) struct RepositoryKeyMethodsInput<'a> {
    pub(super) pk_type_ty: &'a syn::Type,
    pub(super) pool_type: &'a TokenStream,
    pub(super) key_type: &'a syn::Ident,
    pub(super) has_composite_primary_key: bool,
}

pub(super) fn repository_key_methods(
    context: &QueryContext<'_>,
    input: RepositoryKeyMethodsInput<'_>,
) -> TokenStream {
    let RepositoryKeyMethodsInput {
        pk_type_ty,
        pool_type,
        key_type,
        has_composite_primary_key,
    } = input;
    let QueryContext {
        backend_marker,
        entity_name_lit,
        ..
    } = *context;
    let find_by_id_method = if has_composite_primary_key {
        quote! {}
    } else {
        quote! {
            #[allow(clippy::ptr_arg)]
            pub async fn find_by_id(
                db: &::graphql_orm::db::Database<#backend_marker>,
                id: &#pk_type_ty,
            ) -> ::graphql_orm::Result<::std::option::Option<Self>> {
                Self::find_by_key(db, id).await
            }
        }
    };
    let get_by_id_method = if has_composite_primary_key {
        quote! {}
    } else {
        quote! {
            pub async fn get(pool: &#pool_type, id: &#pk_type_ty) -> ::graphql_orm::Result<::std::option::Option<Self>> {
                Self::get_by_key(pool, id).await
            }
        }
    };
    quote! {
        #find_by_id_method

        pub async fn find_by_key(
            db: &::graphql_orm::db::Database<#backend_marker>,
            key: &#key_type,
        ) -> ::graphql_orm::Result<::std::option::Option<Self>> {
            db.ensure_entity_access(
                ::std::option::Option::None,
                #entity_name_lit,
                <Self as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
            ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

            // The Repository-surface predicate narrows the SQL, as for
            // repository list reads; the row still passes `can_read_row`.
            let entity = ::graphql_orm::graphql::orm::with_repository_read_visibility(
                db,
                ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                    .where_values(&Self::__gom_key_where_clause(), Self::__gom_key_values(key)),
            )
            .await?
            .fetch_one(db)
            .await?;

            if let ::std::option::Option::Some(ref loaded) = entity {
                if !db.can_read_row(
                    ::std::option::Option::None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                    loaded as &(dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync),
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)? {
                    return ::std::result::Result::Ok(::std::option::Option::None);
                }
                for field in <Self as ::graphql_orm::graphql::orm::Entity>::repository_field_policies() {
                    db.ensure_repository_readable_field(
                        ::std::option::Option::None,
                        #entity_name_lit,
                        field.api_name,
                        field.read_policy,
                        ::std::option::Option::Some(loaded as &(dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync)),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                }
            }

            ::std::result::Result::Ok(entity)
        }

        #get_by_id_method

        pub async fn get_by_key(pool: &#pool_type, key: &#key_type) -> ::graphql_orm::Result<::std::option::Option<Self>> {
            ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                .where_values(&Self::__gom_key_where_clause(), Self::__gom_key_values(key))
                .fetch_one(pool)
                .await
        }
    }
}

pub(super) struct RootInput<'a> {
    pub(super) queries_struct: &'a syn::Ident,
    pub(super) aggregate_query_method: &'a TokenStream,
    pub(super) list_operation_description: &'a str,
    pub(super) list_query_skip: &'a TokenStream,
    pub(super) list_query_name: &'a str,
    pub(super) list_requires_scopes: &'a TokenStream,
    pub(super) order_by_input: &'a syn::Ident,
    pub(super) list_scope_enforcement: &'a TokenStream,
    pub(super) search_query_method: &'a TokenStream,
    pub(super) keyset_graphql_method: &'a TokenStream,
    pub(super) single_operation_description: &'a str,
    pub(super) single_query_skip: &'a TokenStream,
    pub(super) single_query_name: &'a str,
    pub(super) single_read_requires_scopes: &'a TokenStream,
    pub(super) single_query_args: &'a [TokenStream],
    pub(super) single_read_scope_enforcement: &'a TokenStream,
    pub(super) single_query_key_init: &'a TokenStream,
    pub(super) federation_entity_resolvers: &'a TokenStream,
    pub(super) node_id_entity_resolver: &'a TokenStream,
    pub(super) full_write_surface: bool,
}

pub(super) fn root(context: &QueryContext<'_>, input: RootInput<'_>) -> TokenStream {
    let RootInput {
        queries_struct,
        aggregate_query_method,
        list_operation_description,
        list_query_skip,
        list_query_name,
        list_requires_scopes,
        order_by_input,
        list_scope_enforcement,
        search_query_method,
        keyset_graphql_method,
        single_operation_description,
        single_query_skip,
        single_query_name,
        single_read_requires_scopes,
        single_query_args,
        single_read_scope_enforcement,
        single_query_key_init,
        federation_entity_resolvers,
        node_id_entity_resolver,
        full_write_surface,
    } = input;
    let QueryContext {
        struct_name,
        backend_marker,
        resolver_auth_mode,
        authenticated_directive,
        rls_request_guard,
        entity_name_lit,
        where_input,
        where_arg_name,
        where_arg_description,
        page_arg_name,
        page_arg_description,
        order_by_arg_name,
        order_by_arg_description,
        connection_type,
        ref filter_read_authorization,
        ..
    } = *context;
    let query_docs = if full_write_surface {
        quote! { /// Generated queries for this entity
        }
    } else {
        quote! {}
    };

    // Ordering reveals field values, and relation count ordering reveals
    // related-row counts. Authorize each scalar term with the field's read
    // policy, and each count against the target's access, operation scopes
    // and SQL visibility, before any SQL.
    let order_read_authorization = quote! {
        let mut order_by = order_by;
        if let ::std::option::Option::Some(orders) = order_by.as_mut() {
            if orders.len() > ::graphql_orm::graphql::orm::MAX_ORDER_BY_TERMS {
                return ::std::result::Result::Err(::graphql_orm::graphql::errors::OrmPublicError::new(
                    ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput).into_graphql_error());
            }
            for order in orders.iter_mut() {
                order.__gom_prepare_read(ctx, &mut __gom_statement_budget).await?;
            }
            ::graphql_orm::graphql::orm::authorize_read_fields::<#struct_name, #backend_marker>(
                db,
                ctx,
                orders
                    .iter()
                    .flat_map(::graphql_orm::graphql::orm::DatabaseOrderBy::referenced_fields),
            ).await?;
        }
    };
    quote! {
        #query_docs
        #[derive(Default)]
        pub struct #queries_struct;

        #[::graphql_orm::async_graphql::Object]
        impl #queries_struct {
            #aggregate_query_method

            #[doc = #list_operation_description]
            #[graphql(#list_query_skip name = #list_query_name, #list_requires_scopes #authenticated_directive)]
            async fn list(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: ::std::option::Option<#where_input>,
                #[graphql(name = #order_by_arg_name, desc = #order_by_arg_description)] order_by: ::std::option::Option<::std::vec::Vec<#order_by_input>>,
                #[graphql(name = #page_arg_name, desc = #page_arg_description)] page: ::std::option::Option<::graphql_orm::graphql::orm::PageInput>,
            ) -> ::graphql_orm::async_graphql::Result<#connection_type> {
                ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
                use ::graphql_orm::graphql::orm::{DatabaseOrderBy, EntityQuery};
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #list_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                )?;
                let db = ctx.data::<::graphql_orm::db::Database<#backend_marker>>()?;
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                db.ensure_entity_access(
                    ::std::option::Option::Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                #filter_read_authorization
                #order_read_authorization
                let mut query = EntityQuery::<#struct_name, #backend_marker>::new();

                if let ::std::option::Option::Some(ref filter) = where_input {
                    query = query.filter_with_entity_matching(filter);
                }

                if let ::std::option::Option::Some(ref orders) = order_by {
                    for order in orders {
                        query = query.order_by_with_context(order, ctx)?;
                    }
                }

                if query.order_clauses.is_empty() {
                    query = query.default_order();
                }

                let visibility = db.read_visibility::<#struct_name>(
                    ::std::option::Option::Some(ctx), ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                if visibility.requires_residual_checks() {
                    let resolved_page = db.client_pagination_config().resolve_client_page(page.as_ref(), true).map_err(|error| error.into_graphql_error())?;
                    let authorized_page = ::graphql_orm::graphql::loaders::scan_authorized_query(
                        db, ctx, query, auth_context.as_ref(), &visibility, resolved_page,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                    ).await?;
                    return ::std::result::Result::Ok(#connection_type::from_generic(
                        authorized_page.into_offset_connection(),
                    ));
                }

                query = query.with_read_visibility(&visibility)?;
                // Client page semantics: reduce, or refuse when the host opted in,
                // before the exact host connection read.
                if page.is_some() {
                    let resolved_page = db.client_pagination_config().resolve_client_page(page.as_ref(), true).map_err(|error| error.into_graphql_error())?;
                    query = query.paginate(&::graphql_orm::graphql::orm::PageInput {
                        limit: resolved_page.limit,
                        offset: ::std::option::Option::Some(resolved_page.offset),
                    });
                }

                let generic_conn = query
                    .fetch_connection_selected_with_auth(db, auth_context.as_ref(), ::graphql_orm::graphql::pagination::ConnectionSelection::from_context(ctx))
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                ::std::result::Result::Ok(#connection_type::from_generic(generic_conn))
            }

            #search_query_method
            #keyset_graphql_method

            #[doc = #single_operation_description]
            #[graphql(#single_query_skip name = #single_query_name, #single_read_requires_scopes #authenticated_directive)]
            async fn get_by_id(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #(#single_query_args)*
            ) -> ::graphql_orm::async_graphql::Result<::std::option::Option<#struct_name>> {
                use ::graphql_orm::graphql::orm::EntityQuery;
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #single_read_scope_enforcement
                #single_query_key_init
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                )?;
                let db = ctx.data::<::graphql_orm::db::Database<#backend_marker>>()?;
                db.ensure_entity_access(
                    ::std::option::Option::Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                // The same row authority as generated list reads.
                ::graphql_orm::graphql::orm::fetch_authorized_point_row(
                    db,
                    ctx,
                    EntityQuery::<#struct_name, #backend_marker>::new()
                        .where_values(&#struct_name::__gom_key_where_clause(), #struct_name::__gom_key_values(&key)),
                ).await
            }

            #federation_entity_resolvers
                #node_id_entity_resolver
        }
    }
}

pub(super) struct ConnectionInput<'a> {
    pub(super) edge_type_str: &'a str,
    pub(super) edge_type: &'a syn::Ident,
    pub(super) connection_type_str: &'a str,
    pub(super) full_write_surface: bool,
}

pub(super) fn connection(context: &QueryContext<'_>, input: ConnectionInput<'_>) -> TokenStream {
    let ConnectionInput {
        edge_type_str,
        edge_type,
        connection_type_str,
        full_write_surface,
    } = input;
    let QueryContext {
        struct_name,
        field_case_rule,
        page_info_field_name,
        connection_type,
        ..
    } = *context;
    let (
        edge_docs,
        node_docs,
        cursor_docs,
        connection_docs,
        edges_docs,
        page_info_docs,
        conversion_docs,
        empty_docs,
    ) = if full_write_surface {
        (
            quote! { /// Edge containing a node and cursor
            },
            quote! { /// The item at the end of the edge
            },
            quote! { /// A cursor for pagination
            },
            quote! { /// Connection containing edges and page info
            },
            quote! { /// The edges in this connection
            },
            quote! { /// Pagination information
            },
            quote! { /// Create from a generic Connection
            },
            quote! { /// Create an empty connection
            },
        )
    } else {
        Default::default()
    };

    let edge_object = crate::wrapper_object::wrapper_object(quote! {
        #edge_docs
        #[derive(Debug, Clone)]
        #[graphql(name = #edge_type_str, rename_fields = #field_case_rule)]
        pub struct #edge_type {
            #node_docs
            pub node: #struct_name,
            #cursor_docs
            pub cursor: ::std::string::String,
        }
    });
    let connection_object = crate::wrapper_object::wrapper_object(quote! {
        #connection_docs
        #[derive(Debug, Clone)]
        #[graphql(name = #connection_type_str, rename_fields = #field_case_rule)]
        pub struct #connection_type {
            #edges_docs
            pub edges: ::std::vec::Vec<#edge_type>,
            #page_info_docs
            #[graphql(name = #page_info_field_name)]
            pub page_info: ::graphql_orm::graphql::pagination::PageInfo,
        }
    });
    let connection_impl = quote! {
        impl #connection_type {
            #conversion_docs
            pub fn from_generic(conn: ::graphql_orm::graphql::pagination::Connection<#struct_name>) -> Self {
                Self {
                    edges: conn.edges.into_iter().map(|e| #edge_type {
                        node: e.node,
                        cursor: e.cursor,
                    }).collect(),
                    page_info: conn.page_info,
                }
            }

            #empty_docs
            pub fn empty() -> Self {
                Self {
                    edges: ::std::vec::Vec::new(),
                    page_info: ::graphql_orm::graphql::pagination::PageInfo::default(),
                }
            }
        }

    };
    quote! { #edge_object #connection_object #connection_impl }
}

#[cfg(test)]
mod tests;
