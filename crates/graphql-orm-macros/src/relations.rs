use super::*;
use crate::backend::{
    backend_marker_tokens, backend_pool_type_tokens, backend_quote_identifier_path, resolve_backend,
};
use crate::entity::{
    collect_parsed_fields, has_graphql_complex, parse_entity_metadata,
    relation_change_propagation_tokens, relation_delete_policy_tokens,
};
use crate::naming::{apply_graphql_case, graphql_field_name, selected_argument_case};
use syn::spanned::Spanned;

struct RelationDef {
    field_name: syn::Ident,
    graphql_name: String,
    target_type_str: String,
    source_columns: Vec<String>,
    fk_columns: Vec<String>,
    is_multiple: bool,
    source_field_idents: Vec<syn::Ident>,
    source_kinds: Vec<RelationValueKind>,
    source_optional: Vec<bool>,
    source_supports_dataloader: bool,
    emit_foreign_key: bool,
    on_delete: proc_macro2::TokenStream,
    propagate_change: proc_macro2::TokenStream,
    storage_kind: RelationStorageKind,
}

#[derive(Copy, Clone)]
enum RelationValueKind {
    String,
    Uuid,
    Int,
    Float,
    Bool,
}

#[derive(Copy, Clone)]
enum RelationStorageKind {
    Direct,
    BoxedSingle,
    BoxedMany,
}

fn relation_storage_kind(ty: &syn::Type, is_multiple: bool) -> RelationStorageKind {
    match ty {
        syn::Type::Path(type_path) => {
            let Some(segment) = type_path.path.segments.last() else {
                return RelationStorageKind::Direct;
            };
            match segment.ident.to_string().as_str() {
                "Option" if !is_multiple => {
                    if let syn::PathArguments::AngleBracketed(args) = &segment.arguments
                        && let Some(syn::GenericArgument::Type(syn::Type::Path(inner_path))) =
                            args.args.first()
                        && let Some(inner_segment) = inner_path.path.segments.last()
                        && inner_segment.ident == "Box"
                    {
                        return RelationStorageKind::BoxedSingle;
                    }
                    RelationStorageKind::Direct
                }
                "Vec" if is_multiple => {
                    if let syn::PathArguments::AngleBracketed(args) = &segment.arguments
                        && let Some(syn::GenericArgument::Type(syn::Type::Path(inner_path))) =
                            args.args.first()
                        && let Some(inner_segment) = inner_path.path.segments.last()
                        && inner_segment.ident == "Box"
                    {
                        return RelationStorageKind::BoxedMany;
                    }
                    RelationStorageKind::Direct
                }
                _ => RelationStorageKind::Direct,
            }
        }
        _ => RelationStorageKind::Direct,
    }
}

fn classify_relation_value_type(ty: &syn::Type) -> Option<(RelationValueKind, bool)> {
    let mut current = ty;
    let mut is_option = false;

    loop {
        match current {
            syn::Type::Path(type_path) => {
                let segment = type_path.path.segments.last()?;
                let name = segment.ident.to_string();
                if name == "Option" {
                    if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                        if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                            current = inner;
                            is_option = true;
                            continue;
                        }
                    }
                    return None;
                }

                let kind = match name.as_str() {
                    "String" => RelationValueKind::String,
                    "Uuid" => RelationValueKind::Uuid,
                    "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64"
                    | "usize" => RelationValueKind::Int,
                    "f32" | "f64" => RelationValueKind::Float,
                    "bool" => RelationValueKind::Bool,
                    _ => return None,
                };
                return Some((kind, is_option));
            }
            _ => return None,
        }
    }
}

fn relation_key_part_kind_tokens(kind: RelationValueKind) -> proc_macro2::TokenStream {
    match kind {
        RelationValueKind::String => {
            quote! { ::graphql_orm::graphql::loaders::RelationKeyPartKind::String }
        }
        RelationValueKind::Uuid => {
            quote! { ::graphql_orm::graphql::loaders::RelationKeyPartKind::Uuid }
        }
        RelationValueKind::Int => {
            quote! { ::graphql_orm::graphql::loaders::RelationKeyPartKind::Int }
        }
        RelationValueKind::Float => {
            quote! { ::graphql_orm::graphql::loaders::RelationKeyPartKind::Float }
        }
        RelationValueKind::Bool => {
            quote! { ::graphql_orm::graphql::loaders::RelationKeyPartKind::Bool }
        }
    }
}

pub(crate) fn generate_graphql_relations(
    input: &DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;
    let entity_meta = parse_entity_metadata(&input.attrs)?;
    let backend = resolve_backend(
        entity_meta.backend.as_deref(),
        struct_name.span(),
        "graphql_entity",
    )?;
    let backend_marker = backend_marker_tokens(backend);
    let pool_type = backend_pool_type_tokens(backend);

    let data = match &input.data {
        Data::Struct(data) => data,
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "GraphQLRelations can only be derived for structs",
            ));
        }
    };

    let fields = match &data.fields {
        Fields::Named(fields) => &fields.named,
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "GraphQLRelations requires named fields",
            ));
        }
    };

    let graphql_rename_fields = entity_meta.graphql_rename_fields.as_deref();
    let serde_rename_all = entity_meta.serde_rename_all.as_deref();
    let argument_case = selected_argument_case();
    let where_arg_name = apply_graphql_case("where", argument_case);
    let order_by_arg_name = apply_graphql_case("orderBy", argument_case);
    let page_arg_name = apply_graphql_case("page", argument_case);
    let legacy_graphql_complex = has_graphql_complex(&input.attrs);
    let parsed_fields = collect_parsed_fields(fields.iter())?;

    // Find primary key field
    let mut pk_field_name: Option<syn::Ident> = None;
    for parsed_field in &parsed_fields {
        if parsed_field.meta.is_primary_key {
            pk_field_name = Some(parsed_field.field.ident.clone().unwrap());
            break;
        }
    }
    let pk_field = pk_field_name.unwrap_or_else(|| syn::Ident::new("id", struct_name.span()));

    // Collect relations
    let mut relations: Vec<RelationDef> = Vec::new();

    for parsed_field in &parsed_fields {
        let field = &parsed_field.field;
        let meta = &parsed_field.meta;
        if !meta.is_relation || !meta.read {
            continue;
        }

        let field_name = field.ident.clone().unwrap();
        let rust_name = field_name.to_string();
        let graphql_name =
            graphql_field_name(meta, &rust_name, graphql_rename_fields, serde_rename_all);

        let target_type = meta
            .relation_target
            .clone()
            .unwrap_or_else(|| "Unknown".to_string());
        let to_cols = meta.relation_to_fields.clone().unwrap_or_else(|| {
            vec![
                meta.relation_to
                    .clone()
                    .unwrap_or_else(|| "unknown_id".to_string()),
            ]
        });
        let from_cols = meta.relation_from_fields.clone().unwrap_or_else(|| {
            vec![
                meta.relation_from
                    .clone()
                    .unwrap_or_else(|| pk_field.to_string()),
            ]
        });
        if from_cols.len() != to_cols.len() {
            return Err(syn::Error::new_spanned(
                field,
                format!(
                    "Relation '{}' has {} source key part(s) but {} target key part(s)",
                    rust_name,
                    from_cols.len(),
                    to_cols.len()
                ),
            ));
        }
        if from_cols.is_empty() {
            return Err(syn::Error::new_spanned(
                field,
                format!("Relation '{}' must define at least one key part", rust_name),
            ));
        }

        let mut source_field_idents = Vec::with_capacity(from_cols.len());
        let mut source_kinds = Vec::with_capacity(from_cols.len());
        let mut source_optional = Vec::with_capacity(from_cols.len());
        for from_col in &from_cols {
            let source_field = parsed_fields
                .iter()
                .find(|parsed| {
                    parsed
                        .field
                        .ident
                        .as_ref()
                        .map(|ident| ident == &syn::Ident::new(from_col, ident.span()))
                        .unwrap_or(false)
                })
                .ok_or_else(|| {
                    syn::Error::new_spanned(
                        field,
                        format!(
                            "Relation '{}' references unknown source field '{}' on '{}'",
                            rust_name, from_col, struct_name
                        ),
                    )
                })?;
            let source_field_ty = source_field.field.ty.clone();
            let (source_kind, source_is_option) =
                classify_relation_value_type(&source_field_ty).ok_or_else(|| {
                    syn::Error::new_spanned(
                        &source_field.field.ty,
                        format!(
                            "Unsupported relation source field type for '{}.{}': expected String/uuid/int/float/bool (optionals allowed)",
                            struct_name, from_col
                        ),
                    )
                })?;
            source_field_idents.push(source_field.field.ident.clone().unwrap());
            source_kinds.push(source_kind);
            source_optional.push(source_is_option);
        }
        let source_supports_dataloader = true;
        let is_multiple = meta.relation_multiple;
        let on_delete =
            relation_delete_policy_tokens(meta.relation_on_delete.as_deref(), field.span())?;
        let propagate_change = relation_change_propagation_tokens(
            meta.relation_propagate_change.as_deref(),
            field.span(),
        )?;
        let storage_kind = relation_storage_kind(&field.ty, is_multiple);

        relations.push(RelationDef {
            field_name,
            graphql_name,
            target_type_str: target_type,
            source_columns: from_cols,
            fk_columns: to_cols,
            is_multiple,
            source_field_idents,
            source_kinds,
            source_optional,
            source_supports_dataloader,
            emit_foreign_key: meta.relation_emit_foreign_key.unwrap_or(!is_multiple),
            on_delete,
            propagate_change,
            storage_kind,
        });
    }

    // Generate relation metadata
    let relation_metadata: Vec<_> = relations
        .iter()
        .map(|r| {
            let graphql_name = &r.graphql_name;
            let target_type = &r.target_type_str;
            let source_column = r
                .source_columns
                .first()
                .expect("relation has source column");
            let target_column = r.fk_columns.first().expect("relation has target column");
            let source_columns = r.source_columns.iter().collect::<Vec<_>>();
            let target_columns = r.fk_columns.iter().collect::<Vec<_>>();
            let is_multiple = r.is_multiple;
            let emit_foreign_key = r.emit_foreign_key;
            let on_delete = &r.on_delete;
            let propagate_change = &r.propagate_change;
            quote! {
                ::graphql_orm::graphql::orm::RelationMetadata {
                    field_name: #graphql_name,
                    target_type: #target_type,
                    source_column: #source_column,
                    target_column: #target_column,
                    source_columns: &[#(#source_columns),*],
                    target_columns: &[#(#target_columns),*],
                    is_multiple: #is_multiple,
                    emit_foreign_key: #emit_foreign_key,
                    on_delete: #on_delete,
                    propagate_change: #propagate_change,
                }
            }
        })
        .collect();

    // Generate ComplexObject resolver methods for relations with filtering/sorting/pagination
    //
    // Strategy:
    // - When NO filter/sort/pagination args provided: Use DataLoader for batching (N+1 free)
    // - When args ARE provided: Use direct database query (supports full SQL filtering)
    //
    // This gives optimal performance for simple relation traversal while keeping
    // full filter/sort/pagination support for complex queries.
    let relation_resolvers: Vec<_> = relations
        .iter()
        .map(|r| -> syn::Result<proc_macro2::TokenStream> {
        let field_name = &r.field_name;
        let graphql_name = &r.graphql_name;
        let fk_columns_sql = r
            .fk_columns
            .iter()
            .map(|column| backend_quote_identifier_path(backend, column))
            .collect::<Vec<_>>();
        let source_supports_dataloader = r.source_supports_dataloader;
        let storage_kind = r.storage_kind;
        let source_fields = &r.source_field_idents;
        let source_kinds = &r.source_kinds;
        let source_optional = &r.source_optional;
        let key_part_kind_tokens = source_kinds
            .iter()
            .copied()
            .map(relation_key_part_kind_tokens)
            .collect::<Vec<_>>();

        let source_value_bindings = source_fields
            .iter()
            .zip(source_kinds.iter())
            .zip(source_optional.iter())
            .map(|((source_field, source_kind), source_is_option)| {
                let sql_value_expr = match source_kind {
                    RelationValueKind::String => quote! { ::graphql_orm::graphql::orm::SqlValue::String(value.clone()) },
                    RelationValueKind::Uuid => quote! { ::graphql_orm::graphql::orm::SqlValue::Uuid(*value) },
                    RelationValueKind::Int => quote! { ::graphql_orm::graphql::orm::SqlValue::Int(*value as i64) },
                    RelationValueKind::Float => quote! { ::graphql_orm::graphql::orm::SqlValue::Float((*value).into()) },
                    RelationValueKind::Bool => quote! { ::graphql_orm::graphql::orm::SqlValue::Bool(*value) },
                };
                let key_part_expr = match source_kind {
                    RelationValueKind::String => quote! { value.clone() },
                    RelationValueKind::Uuid => quote! { value.to_string() },
                    _ => quote! { value.to_string() },
                };
                if *source_is_option {
                    quote! {
                        let Some(value) = self.#source_field.as_ref() else {
                            return None;
                        };
                        relation_sql_values.push(#sql_value_expr);
                        relation_key_parts.push(#key_part_expr);
                    }
                } else {
                    quote! {
                        let value = &self.#source_field;
                        relation_sql_values.push(#sql_value_expr);
                        relation_key_parts.push(#key_part_expr);
                    }
                }
            })
            .collect::<Vec<_>>();

        // Generate type name strings for use in fully-qualified paths
        let target_type_str = &r.target_type_str;
        let where_input_str = format!("{}WhereInput", r.target_type_str);
        let order_by_input_str = format!("{}OrderByInput", r.target_type_str);
        let connection_type_str = format!("{}Connection", r.target_type_str);
        let edge_type_str = format!("{}Edge", r.target_type_str);

        // Create idents for local use in the macro
        let target_type = syn::Ident::new(target_type_str, struct_name.span());
        let where_input = syn::Ident::new(&where_input_str, struct_name.span());
        let order_by_input = syn::Ident::new(&order_by_input_str, struct_name.span());
        let connection_type = syn::Ident::new(&connection_type_str, struct_name.span());
        let edge_type = syn::Ident::new(&edge_type_str, struct_name.span());

        let source_binding_multiple = if source_optional.iter().any(|is_option| *is_option) {
            quote! {
                let Some((relation_loader_key, relation_sql_values)) = (|| {
                    let mut relation_sql_values = Vec::new();
                    let mut relation_key_parts = Vec::new();
                    #(#source_value_bindings)*
                    Some((
                        ::graphql_orm::graphql::loaders::RelationKey::new(relation_key_parts),
                        relation_sql_values,
                    ))
                })() else {
                    let page_info = ::graphql_orm::graphql::pagination::PageInfo {
                        has_next_page: false,
                        has_previous_page: false,
                        start_cursor: None,
                        end_cursor: None,
                        total_count: Some(0),
                    };
                    return Ok(#connection_type { edges: Vec::new(), page_info });
                };
            }
        } else {
            quote! {
                let (relation_loader_key, relation_sql_values) = {
                    let mut relation_sql_values = Vec::new();
                    let mut relation_key_parts = Vec::new();
                    #(#source_value_bindings)*
                    (
                        ::graphql_orm::graphql::loaders::RelationKey::new(relation_key_parts),
                        relation_sql_values,
                    )
                };
            }
        };

        let source_binding_single = if source_optional.iter().any(|is_option| *is_option) {
            quote! {
                let Some((relation_loader_key, relation_sql_values)) = (|| {
                    let mut relation_sql_values = Vec::new();
                    let mut relation_key_parts = Vec::new();
                    #(#source_value_bindings)*
                    Some((
                        ::graphql_orm::graphql::loaders::RelationKey::new(relation_key_parts),
                        relation_sql_values,
                    ))
                })() else {
                    return Ok(None);
                };
            }
        } else {
            quote! {
                let (relation_loader_key, relation_sql_values) = {
                    let mut relation_sql_values = Vec::new();
                    let mut relation_key_parts = Vec::new();
                    #(#source_value_bindings)*
                    (
                        ::graphql_orm::graphql::loaders::RelationKey::new(relation_key_parts),
                        relation_sql_values,
                    )
                };
            }
        };

        let relation_query_key = quote! {
            ::graphql_orm::graphql::loaders::CompositeRelationQueryKey {
                relation: #graphql_name,
                parent_key: relation_loader_key.clone(),
                parent_values: relation_sql_values.clone(),
                fk_columns: vec![#(#fk_columns_sql),*],
                key_part_kinds: vec![#(#key_part_kind_tokens),*],
                where_signature: where_input
                    .as_ref()
                    .and_then(|filter| filter.to_filter_expression().map(|expr| format!("{expr:?}"))),
                order_signature: order_by
                    .as_ref()
                    .and_then(|order| order.to_sort_expression().map(|expr| expr.clause)),
                page_signature: page
                    .as_ref()
                    .map(|page| format!("limit={:?};offset={}", page.limit(), page.offset())),
                filter: where_input.as_ref().and_then(|filter| filter.to_filter_expression()),
                sorts: order_by
                    .as_ref()
                    .and_then(|order| order.to_sort_expression())
                    .into_iter()
                    .collect(),
                pagination: page.as_ref().map(::graphql_orm::graphql::orm::PaginationRequest::from),
                auth_context: auth_context.clone(),
            }
        };

        let single_relation_query_key = quote! {
            ::graphql_orm::graphql::loaders::CompositeRelationQueryKey {
                relation: #graphql_name,
                parent_key: relation_loader_key.clone(),
                parent_values: relation_sql_values.clone(),
                fk_columns: vec![#(#fk_columns_sql),*],
                key_part_kinds: vec![#(#key_part_kind_tokens),*],
                where_signature: None,
                order_signature: None,
                page_signature: None,
                filter: None,
                sorts: Vec::new(),
                pagination: None,
                auth_context: auth_context.clone(),
            }
        };
        let fallback_predicate_parts = fk_columns_sql
            .iter()
            .enumerate()
            .map(|(index, column)| {
                let placeholder_index = index + 1;
                quote! {
                    format!("{} = {}", #column, #target_type::__gom_placeholder(#placeholder_index))
                }
            })
            .collect::<Vec<_>>();
        let fallback_relation_clause = quote! {
            vec![#(#fallback_predicate_parts),*].join(" AND ")
        };

        if r.is_multiple {
            let preloaded_entities = match storage_kind {
                RelationStorageKind::BoxedMany => {
                    quote! {
                        let entities: Vec<#target_type> = self.#field_name
                            .iter()
                            .cloned()
                            .map(|entity| *entity)
                            .collect();
                    }
                }
                _ => {
                    quote! {
                        let entities = self.#field_name.clone();
                    }
                }
            };
            // One-to-many relation with smart batching
            Ok(quote! {
                /// Get related #graphql_name with optional filtering, sorting, and pagination.
                ///
                /// When no arguments are provided, uses DataLoader to batch queries and
                /// avoid N+1 when loading relations for multiple parent entities.
                /// When filter/sort/pagination arguments are provided, uses direct
                /// database query for full SQL support.
                #[graphql(name = #graphql_name)]
                async fn #field_name(
                    &self,
                    ctx: &::graphql_orm::async_graphql::Context<'_>,
                    #[graphql(name = #where_arg_name)] where_input: Option<#where_input>,
                    #[graphql(name = #order_by_arg_name)] order_by: Option<#order_by_input>,
                    #[graphql(name = #page_arg_name)] page: Option<::graphql_orm::graphql::orm::PageInput>,
                ) -> ::graphql_orm::async_graphql::Result<#connection_type> {
                    use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, DatabaseOrderBy, EntityQuery, SqlValue};

                    let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                    let auth_context = ctx
                        .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                        .cloned();
                    db.ensure_entity_access(
                        Some(ctx),
                        <#target_type as ::graphql_orm::graphql::orm::Entity>::entity_name(),
                        <#target_type as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                        ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlRelation,
                    ).await?;

                    if where_input.is_none() && order_by.is_none() && page.is_none() && !self.#field_name.is_empty() {
                        #preloaded_entities
                        let edges: Vec<#edge_type> = entities
                            .into_iter()
                            .enumerate()
                            .map(|(i, entity)| #edge_type {
                                cursor: ::graphql_orm::graphql::pagination::encode_cursor(i as i64),
                                node: entity,
                            })
                            .collect();
                        let page_info = ::graphql_orm::graphql::pagination::PageInfo {
                            has_next_page: false,
                            has_previous_page: false,
                            start_cursor: edges.first().map(|e| e.cursor.clone()),
                            end_cursor: edges.last().map(|e| e.cursor.clone()),
                            total_count: Some(edges.len() as i64),
                        };
                        return Ok(#connection_type { edges, page_info });
                    }

                    // Use DataLoader whenever the relation key can be batched.
                    #source_binding_multiple

                    let use_dataloader = #source_supports_dataloader;

                    let loaded = if use_dataloader {
                        use ::graphql_orm::graphql::loaders::RelationLoader;
                        use ::graphql_orm::async_graphql::dataloader::DataLoader;

                        let loader = ctx.data_unchecked::<DataLoader<RelationLoader<#target_type, #backend_marker>>>();
                        loader
                            .load_one(#relation_query_key)
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?
                            .unwrap_or(::graphql_orm::graphql::loaders::RelationLoadResult {
                                entities: Vec::new(),
                                total_count: 0,
                                has_next_page: false,
                                has_previous_page: false,
                                offset: 0,
                            })
                    } else {
                        // Slow path: Use direct query with full SQL support
                        let mut query = EntityQuery::<#target_type, #backend_marker>::new()
                            .where_values(&#fallback_relation_clause, relation_sql_values.clone());

                        if let Some(ref filter) = where_input {
                            query = query.filter(filter);
                        }

                        if let Some(ref order) = order_by {
                            query = query.order_by(order);
                        }

                        if query.order_clauses.is_empty() {
                            query = query.default_order();
                        }

                        if let Some(ref p) = page {
                            query = query.paginate(p);
                        }

                        // Count must be computed before/independent of pagination window.
                        // EntityQuery::count ignores limit/offset and uses only WHERE clauses.
                        let total = query
                            .count_with_auth(db, auth_context.as_ref())
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                        let offset = page.as_ref().map(|p| p.offset()).unwrap_or(0) as usize;

                        let entities = query
                            .fetch_all_with_auth(db, auth_context.as_ref())
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                        ::graphql_orm::graphql::loaders::RelationLoadResult {
                            has_next_page: (offset as i64 + entities.len() as i64) < total,
                            has_previous_page: offset > 0,
                            entities,
                            total_count: total,
                            offset: offset as i64,
                        }
                    };

                    let edges: Vec<#edge_type> = loaded.entities
                        .into_iter()
                        .enumerate()
                        .map(|(i, entity)| #edge_type {
                            cursor: ::graphql_orm::graphql::pagination::encode_cursor(loaded.offset + i as i64),
                                node: entity,
                            })
                            .collect();

                    let page_info = ::graphql_orm::graphql::pagination::PageInfo {
                        has_next_page: loaded.has_next_page,
                        has_previous_page: loaded.has_previous_page,
                        start_cursor: edges.first().map(|e| e.cursor.clone()),
                        end_cursor: edges.last().map(|e| e.cursor.clone()),
                        total_count: Some(loaded.total_count),
                    };

                    Ok(#connection_type { edges, page_info })
                }
            })
        } else {
            let preloaded_single = match storage_kind {
                RelationStorageKind::BoxedSingle => {
                    quote! {
                        return Ok(self.#field_name.clone().map(|entity| *entity));
                    }
                }
                _ => {
                    quote! {
                        return Ok(self.#field_name.clone());
                    }
                }
            };
            // Single relation (many-to-one) - uses DataLoader when the source key supports it
            Ok(quote! {
                /// Get related #graphql_name
                #[graphql(name = #graphql_name)]
                async fn #field_name(
                    &self,
                    ctx: &::graphql_orm::async_graphql::Context<'_>,
                ) -> ::graphql_orm::async_graphql::Result<Option<#target_type>> {
                    use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, SqlValue};

                    if self.#field_name.is_some() {
                        #preloaded_single
                    }

                    let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                    let auth_context = ctx
                        .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                        .cloned();
                    db.ensure_entity_access(
                        Some(ctx),
                        <#target_type as ::graphql_orm::graphql::orm::Entity>::entity_name(),
                        <#target_type as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                        ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlRelation,
                    ).await?;
                    #source_binding_single

                    let result = if #source_supports_dataloader {
                        use ::graphql_orm::graphql::loaders::RelationLoader;
                        use ::graphql_orm::async_graphql::dataloader::DataLoader;

                        let loader = ctx.data_unchecked::<DataLoader<RelationLoader<#target_type, #backend_marker>>>();
                        loader
                            .load_one(#single_relation_query_key)
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?
                            .and_then(|mut result| result.entities.drain(..).next())
                    } else {
                        EntityQuery::<#target_type, #backend_marker>::new()
                            .where_values(&#fallback_relation_clause, relation_sql_values.clone())
                            .fetch_one_with_auth(db, auth_context.as_ref())
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?
                    };

                    Ok(result)
                }
            })
        }
    })
    .collect::<syn::Result<Vec<_>>>()?;

    let bulk_load_blocks: Vec<proc_macro2::TokenStream> = relations
        .iter()
        .map(|r| -> syn::Result<proc_macro2::TokenStream> {
            let field_name = &r.field_name;
            let graphql_name = &r.graphql_name;
            let target_type = syn::Ident::new(&r.target_type_str, struct_name.span());
            let storage_kind = r.storage_kind;
            let fk_columns_sql = r
                .fk_columns
                .iter()
                .map(|column| backend_quote_identifier_path(backend, column))
                .collect::<Vec<_>>();
            let first_fk_column_sql = fk_columns_sql
                .first()
                .expect("relation has target column")
                .clone();
            let key_part_kind_tokens = r
                .source_kinds
                .iter()
                .copied()
                .map(relation_key_part_kind_tokens)
                .collect::<Vec<_>>();
            let relation_key_aliases = (0..r.fk_columns.len())
                .map(|index| format!("__gom_relation_key_{index}"))
                .collect::<Vec<_>>();
            let relation_key_arity = r.fk_columns.len();

            let source_value_bindings = r
                .source_field_idents
                .iter()
                .zip(r.source_kinds.iter())
                .zip(r.source_optional.iter())
                .map(|((source_field, source_kind), source_is_option)| {
                    let sql_value_expr = match source_kind {
                        RelationValueKind::String => quote! { ::graphql_orm::graphql::orm::SqlValue::String(value.clone()) },
                        RelationValueKind::Uuid => quote! { ::graphql_orm::graphql::orm::SqlValue::Uuid(*value) },
                        RelationValueKind::Int => quote! { ::graphql_orm::graphql::orm::SqlValue::Int(*value as i64) },
                        RelationValueKind::Float => quote! { ::graphql_orm::graphql::orm::SqlValue::Float((*value).into()) },
                        RelationValueKind::Bool => quote! { ::graphql_orm::graphql::orm::SqlValue::Bool(*value) },
                    };
                    let key_part_expr = match source_kind {
                        RelationValueKind::String => quote! { value.clone() },
                        RelationValueKind::Uuid => quote! { value.to_string() },
                        _ => quote! { value.to_string() },
                    };
                    if *source_is_option {
                        quote! {
                            let Some(value) = entity.#source_field.as_ref() else {
                                return None;
                            };
                            relation_sql_values.push(#sql_value_expr);
                            relation_key_parts.push(#key_part_expr);
                        }
                    } else {
                        quote! {
                            let value = &entity.#source_field;
                            relation_sql_values.push(#sql_value_expr);
                            relation_key_parts.push(#key_part_expr);
                        }
                    }
                })
                .collect::<Vec<_>>();

            let source_key_bindings = r
                .source_field_idents
                .iter()
                .zip(r.source_kinds.iter())
                .zip(r.source_optional.iter())
                .map(|((source_field, source_kind), source_is_option)| {
                    let key_part_expr = match source_kind {
                        RelationValueKind::String => quote! { value.clone() },
                        RelationValueKind::Uuid => quote! { value.to_string() },
                        _ => quote! { value.to_string() },
                    };
                    if *source_is_option {
                        quote! {
                            let Some(value) = entity.#source_field.as_ref() else {
                                return None;
                            };
                            relation_key_parts.push(#key_part_expr);
                        }
                    } else {
                        quote! {
                            let value = &entity.#source_field;
                            relation_key_parts.push(#key_part_expr);
                        }
                    }
                })
                .collect::<Vec<_>>();

            let entity_key_pair_expr = quote! {
                (|| {
                    let mut relation_sql_values = Vec::new();
                    let mut relation_key_parts = Vec::new();
                    #(#source_value_bindings)*
                    Some((
                        ::graphql_orm::graphql::loaders::RelationKey::new(relation_key_parts),
                        relation_sql_values,
                    ))
                })()
            };
            let entity_key_expr = quote! {
                (|| {
                    let mut relation_key_parts = Vec::new();
                    #(#source_key_bindings)*
                    Some(::graphql_orm::graphql::loaders::RelationKey::new(relation_key_parts))
                })()
            };

            let assign_expr = if r.source_optional.iter().any(|is_option| *is_option) {
                if r.is_multiple {
                    let assign_value = match storage_kind {
                        RelationStorageKind::BoxedMany => {
                            quote! { grouped.remove(&relation_key).unwrap_or_default().into_iter().map(Box::new).collect() }
                        }
                        _ => quote! { grouped.remove(&relation_key).unwrap_or_default() },
                    };
                    quote! {
                        if let Some(relation_key) = #entity_key_expr {
                            entity.#field_name = #assign_value;
                        } else {
                            entity.#field_name = Vec::new();
                        }
                    }
                } else {
                    let assign_value = match storage_kind {
                        RelationStorageKind::BoxedSingle => {
                            quote! { grouped.remove(&relation_key).map(Box::new) }
                        }
                        _ => quote! { grouped.remove(&relation_key) },
                    };
                    quote! {
                        if let Some(relation_key) = #entity_key_expr {
                            entity.#field_name = #assign_value;
                        } else {
                            entity.#field_name = None;
                        }
                    }
                }
            } else if r.is_multiple {
                let assign_value = match storage_kind {
                    RelationStorageKind::BoxedMany => {
                        quote! { grouped.remove(&relation_key).unwrap_or_default().into_iter().map(Box::new).collect() }
                    }
                    _ => quote! { grouped.remove(&relation_key).unwrap_or_default() },
                };
                quote! {
                    if let Some(relation_key) = #entity_key_expr {
                        entity.#field_name = #assign_value;
                    } else {
                        entity.#field_name = Vec::new();
                    }
                }
            } else {
                let assign_value = match storage_kind {
                    RelationStorageKind::BoxedSingle => {
                        quote! { grouped.remove(&relation_key).map(Box::new) }
                    }
                    _ => quote! { grouped.remove(&relation_key) },
                };
                quote! {
                    if let Some(relation_key) = #entity_key_expr {
                        entity.#field_name = #assign_value;
                    } else {
                        entity.#field_name = None;
                    }
                }
            };

            let grouped_type = if r.is_multiple {
                quote! { std::collections::HashMap<::graphql_orm::graphql::loaders::RelationKey, Vec<#target_type>> }
            } else {
                quote! { std::collections::HashMap<::graphql_orm::graphql::loaders::RelationKey, #target_type> }
            };

            let insert_grouped = if r.is_multiple {
                quote! {
                    grouped.entry(relation_key).or_default().push(related);
                }
            } else {
                quote! {
                    grouped.entry(relation_key).or_insert(related);
                }
            };

            Ok(quote! {
                if Self::__gom_selection_contains(selection, #graphql_name)
                    && !Self::__gom_selected_field_has_arguments(selection, #graphql_name)
                {
                    let mut unique_relation_keys: Vec<(
                        ::graphql_orm::graphql::loaders::RelationKey,
                        Vec<::graphql_orm::graphql::orm::SqlValue>,
                    )> = Vec::new();
                    let mut seen_relation_keys = std::collections::HashSet::new();

                    for entity in entities.iter() {
                        if let Some((relation_key, relation_value)) = #entity_key_pair_expr {
                            if seen_relation_keys.insert(relation_key.clone()) {
                                unique_relation_keys.push((relation_key, relation_value));
                            }
                        }
                    }

                    let mut grouped: #grouped_type = std::collections::HashMap::new();

                    if !unique_relation_keys.is_empty() {
                        let bind_values = unique_relation_keys
                            .iter()
                            .flat_map(|(_, values)| values.iter().cloned())
                            .collect::<Vec<_>>();
                        let relation_predicate = if #relation_key_arity == 1 {
                            let placeholders = (0..unique_relation_keys.len())
                                .map(|index| <#target_type>::__gom_placeholder(index + 1))
                                .collect::<Vec<_>>();
                            format!("{} IN ({})", #first_fk_column_sql, placeholders.join(", "))
                        } else {
                            let mut next_placeholder = 1usize;
                            unique_relation_keys
                                .iter()
                                .map(|_| {
                                    let predicates = vec![
                                        #({
                                            let placeholder = <#target_type>::__gom_placeholder(next_placeholder);
                                            next_placeholder += 1;
                                            format!("{} = {}", #fk_columns_sql, placeholder)
                                        }),*
                                    ];
                                    format!("({})", predicates.join(" AND "))
                                })
                                .collect::<Vec<_>>()
                                .join(" OR ")
                        };
                        let relation_key_projections = vec![
                            #(format!(
                                "{} AS {}",
                                ::graphql_orm::graphql::loaders::relation_key_projection(
                                    <#backend_marker as ::graphql_orm::OrmBackend>::DIALECT,
                                    #fk_columns_sql,
                                    #key_part_kind_tokens,
                                ),
                                #relation_key_aliases,
                            )),*
                        ].join(", ");
                        let sql = format!(
                            "SELECT {}, {} FROM {} WHERE {}",
                            <#target_type as ::graphql_orm::graphql::orm::DatabaseEntity>::column_names().join(", "),
                            relation_key_projections,
                            <#target_type as ::graphql_orm::graphql::orm::DatabaseEntity>::TABLE_NAME,
                            relation_predicate,
                        );

                        let rows = <#backend_marker as ::graphql_orm::OrmBackend>::fetch_rows_with_auth(
                            pool,
                            &sql,
                            &bind_values,
                            auth_context,
                        )
                        .await?;
                        for row in rows {
                            let relation_key = ::graphql_orm::graphql::loaders::RelationKey::new(vec![
                                #(<#backend_marker as ::graphql_orm::OrmBackend>::try_get_string(&row, #relation_key_aliases)?),*
                            ]);
                            let related = <#target_type as ::graphql_orm::graphql::orm::FromSqlRow<#backend_marker>>::from_row(&row)?;
                            #insert_grouped
                        }
                    }

                    for entity in entities.iter_mut() {
                        #assign_expr
                    }
                }
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;

    let struct_name_str = struct_name.to_string();
    let has_relations = !relations.is_empty();

    let complex_object_impl = if has_relations {
        if legacy_graphql_complex {
            quote! {
                #[::graphql_orm::async_graphql::ComplexObject]
                impl #struct_name {
                    #(#relation_resolvers)*
                }
            }
        } else {
            quote! {
                #[::graphql_orm::async_graphql::Object(extends)]
                impl #struct_name {
                    #(#relation_resolvers)*
                }
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        impl ::graphql_orm::graphql::orm::RelationLoader<#backend_marker> for #struct_name {
            async fn load_relations(
                &mut self,
                pool: &#pool_type,
                selection: &[::graphql_orm::async_graphql::context::SelectionField<'_>],
            ) -> Result<(), ::graphql_orm::sqlx::Error> {
                Self::bulk_load_relations_with_auth(std::slice::from_mut(self), pool, selection, None).await
            }

            async fn bulk_load_relations(
                entities: &mut [Self],
                pool: &#pool_type,
                selection: &[::graphql_orm::async_graphql::context::SelectionField<'_>],
            ) -> Result<(), ::graphql_orm::sqlx::Error> {
                Self::bulk_load_relations_with_auth(entities, pool, selection, None).await
            }

            async fn load_relations_with_auth(
                &mut self,
                pool: &#pool_type,
                selection: &[::graphql_orm::async_graphql::context::SelectionField<'_>],
                auth_context: Option<&::graphql_orm::graphql::orm::DbAuthContext>,
            ) -> Result<(), ::graphql_orm::sqlx::Error> {
                Self::bulk_load_relations_with_auth(std::slice::from_mut(self), pool, selection, auth_context).await
            }

            async fn bulk_load_relations_with_auth(
                entities: &mut [Self],
                pool: &#pool_type,
                selection: &[::graphql_orm::async_graphql::context::SelectionField<'_>],
                auth_context: Option<&::graphql_orm::graphql::orm::DbAuthContext>,
            ) -> Result<(), ::graphql_orm::sqlx::Error> {
                #(#bulk_load_blocks)*
                Ok(())
            }
        }

        impl #struct_name {
            fn __gom_selection_contains(
                selection: &[::graphql_orm::async_graphql::context::SelectionField<'_>],
                target: &str,
            ) -> bool {
                selection.iter().any(|field| {
                    field.name() == target
                        || {
                            let children = field.selection_set().collect::<Vec<_>>();
                            Self::__gom_selection_contains(&children, target)
                        }
                })
            }

            fn __gom_selected_field_has_arguments(
                selection: &[::graphql_orm::async_graphql::context::SelectionField<'_>],
                target: &str,
            ) -> bool {
                selection.iter().any(|field| {
                    if field.name() == target {
                        field.arguments().map(|arguments| !arguments.is_empty()).unwrap_or(false)
                    } else {
                        let children = field.selection_set().collect::<Vec<_>>();
                        if children.is_empty() {
                            false
                        } else {
                            Self::__gom_selected_field_has_arguments(&children, target)
                        }
                    }
                })
            }

            /// Get relation metadata for look_ahead traversal
            pub fn relation_metadata() -> &'static [::graphql_orm::graphql::orm::RelationMetadata] {
                static RELATIONS: &[::graphql_orm::graphql::orm::RelationMetadata] = &[
                    #(#relation_metadata),*
                ];
                RELATIONS
            }

            /// Get entity name for relation registry
            pub fn entity_name() -> &'static str {
                #struct_name_str
            }
        }

        #complex_object_impl
    })
}
