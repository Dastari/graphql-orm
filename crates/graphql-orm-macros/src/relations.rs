use super::*;
use crate::backend::backend_pool_type_tokens;
use crate::entity::{
    collect_parsed_fields, graphql_field_name, has_graphql_complex, parse_entity_metadata,
};

struct RelationDef {
    field_name: syn::Ident,
    graphql_name: String,
    target_type_str: String,
    source_column: String,
    fk_column: String,
    is_multiple: bool,
    source_field_ty: syn::Type,
    source_supports_dataloader: bool,
}

#[derive(Copy, Clone)]
enum RelationValueKind {
    String,
    Uuid,
    Int,
    Float,
    Bool,
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

pub(crate) fn generate_graphql_relations(
    input: &DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;
    let pool_type = backend_pool_type_tokens();

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

    let entity_meta = parse_entity_metadata(&input.attrs)?;
    let rename_all_rule = entity_meta.serde_rename_all.as_deref();
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
        let graphql_name = graphql_field_name(meta, &rust_name, rename_all_rule);

        let target_type = meta
            .relation_target
            .clone()
            .unwrap_or_else(|| "Unknown".to_string());
        let to_col = meta
            .relation_to
            .clone()
            .unwrap_or_else(|| "unknown_id".to_string());
        let from_col = meta
            .relation_from
            .clone()
            .unwrap_or_else(|| pk_field.to_string());
        let source_field = parsed_fields
            .iter()
            .find(|parsed| {
                parsed
                    .field
                    .ident
                    .as_ref()
                    .map(|ident| ident == &syn::Ident::new(&from_col, ident.span()))
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
        let source_supports_dataloader = matches!(
            classify_relation_value_type(&source_field_ty),
            Some((RelationValueKind::String, _))
        );
        let is_multiple = meta.relation_multiple;

        relations.push(RelationDef {
            field_name,
            graphql_name,
            target_type_str: target_type,
            source_column: from_col,
            fk_column: to_col,
            is_multiple,
            source_field_ty,
            source_supports_dataloader,
        });
    }

    // Generate relation metadata
    let relation_metadata: Vec<_> = relations
        .iter()
        .map(|r| {
            let graphql_name = &r.graphql_name;
            let target_type = &r.target_type_str;
            let source_column = &r.source_column;
            let target_column = &r.fk_column;
            let is_multiple = r.is_multiple;
            quote! {
                ::graphql_orm::graphql::orm::RelationMetadata {
                    field_name: #graphql_name,
                    target_type: #target_type,
                    source_column: #source_column,
                    target_column: #target_column,
                    is_multiple: #is_multiple,
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
        let fk_column = &r.fk_column;
        let source_column = &r.source_column;
        let source_field = syn::Ident::new(source_column, struct_name.span());
        let source_supports_dataloader = r.source_supports_dataloader;
        let source_ty = &r.source_field_ty;

        let (source_kind, source_is_option) =
            classify_relation_value_type(source_ty).ok_or_else(|| {
                syn::Error::new_spanned(
                    source_ty,
                    format!(
                        "Unsupported relation source field type for '{}.{}': expected String/uuid/int/float/bool (optionals allowed)",
                        struct_name, field_name
                    ),
                )
            })?;

        let sql_value_expr = match source_kind {
            RelationValueKind::String => quote! { ::graphql_orm::graphql::orm::SqlValue::String(value.clone()) },
            RelationValueKind::Uuid => quote! { ::graphql_orm::graphql::orm::SqlValue::Uuid(*value) },
            RelationValueKind::Int => quote! { ::graphql_orm::graphql::orm::SqlValue::Int(*value as i64) },
            RelationValueKind::Float => quote! { ::graphql_orm::graphql::orm::SqlValue::Float((*value).into()) },
            RelationValueKind::Bool => quote! { ::graphql_orm::graphql::orm::SqlValue::Bool(*value) },
        };

        let loader_key_expr = match source_kind {
            RelationValueKind::String => quote! { value.clone() },
            RelationValueKind::Uuid => quote! { value.to_string() },
            _ => quote! { value.to_string() },
        };

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

        let source_binding_multiple = if source_is_option {
            quote! {
                let Some(value) = self.#source_field.as_ref() else {
                    let page_info = ::graphql_orm::graphql::pagination::PageInfo {
                        has_next_page: false,
                        has_previous_page: false,
                        start_cursor: None,
                        end_cursor: None,
                        total_count: Some(0),
                    };
                    return Ok(#connection_type { edges: Vec::new(), page_info });
                };
                let relation_sql_value = #sql_value_expr;
                let relation_loader_key = #loader_key_expr;
            }
        } else {
            quote! {
                let value = &self.#source_field;
                let relation_sql_value = #sql_value_expr;
                let relation_loader_key = #loader_key_expr;
            }
        };

        let source_binding_single = if source_is_option {
            quote! {
                let Some(value) = self.#source_field.as_ref() else {
                    return Ok(None);
                };
                let relation_sql_value = #sql_value_expr;
                let relation_loader_key = #loader_key_expr;
            }
        } else {
            quote! {
                let value = &self.#source_field;
                let relation_sql_value = #sql_value_expr;
                let relation_loader_key = #loader_key_expr;
            }
        };

        let relation_query_key = quote! {
            ::graphql_orm::graphql::loaders::RelationQueryKey {
                relation: #graphql_name,
                parent_key: relation_loader_key.clone(),
                parent_value: relation_sql_value.clone(),
                fk_column: #fk_column,
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
            }
        };

        let single_relation_query_key = quote! {
            ::graphql_orm::graphql::loaders::RelationQueryKey {
                relation: #graphql_name,
                parent_key: relation_loader_key.clone(),
                parent_value: relation_sql_value.clone(),
                fk_column: #fk_column,
                where_signature: None,
                order_signature: None,
                page_signature: None,
                filter: None,
                sorts: Vec::new(),
                pagination: None,
            }
        };

        if r.is_multiple {
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
                    #[graphql(name = "Where")] where_input: Option<#where_input>,
                    #[graphql(name = "OrderBy")] order_by: Option<#order_by_input>,
                    #[graphql(name = "Page")] page: Option<::graphql_orm::graphql::orm::PageInput>,
                ) -> ::graphql_orm::async_graphql::Result<#connection_type> {
                    use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, DatabaseOrderBy, EntityQuery, SqlValue};

                    let db = ctx.data_unchecked::<::graphql_orm::db::Database>();

                    if where_input.is_none() && order_by.is_none() && page.is_none() && !self.#field_name.is_empty() {
                        let entities = self.#field_name.clone();
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

                        let loader = ctx.data_unchecked::<DataLoader<RelationLoader<#target_type>>>();
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
                        let mut query = EntityQuery::<#target_type>::new()
                            .where_clause(
                                &format!("{} = {}", #fk_column, #target_type::__gom_placeholder(1)),
                                relation_sql_value
                            );

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
                            .count(db)
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                        let offset = page.as_ref().map(|p| p.offset()).unwrap_or(0) as usize;

                        let entities = query.fetch_all(db)
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
                        return Ok(self.#field_name.clone());
                    }

                    let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                    #source_binding_single

                    let result = if #source_supports_dataloader {
                        use ::graphql_orm::graphql::loaders::RelationLoader;
                        use ::graphql_orm::async_graphql::dataloader::DataLoader;

                        let loader = ctx.data_unchecked::<DataLoader<RelationLoader<#target_type>>>();
                        loader
                            .load_one(#single_relation_query_key)
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?
                            .and_then(|mut result| result.entities.drain(..).next())
                    } else {
                        EntityQuery::<#target_type>::new()
                            .where_clause(
                                &format!("{} = {}", #fk_column, #target_type::__gom_placeholder(1)),
                                relation_sql_value
                            )
                            .fetch_one(db)
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
            let fk_column = &r.fk_column;
            let source_column = &r.source_column;
            let source_field = syn::Ident::new(source_column, struct_name.span());
            let target_type = syn::Ident::new(&r.target_type_str, struct_name.span());
            let source_ty = &r.source_field_ty;

            let (source_kind, source_is_option) =
                classify_relation_value_type(source_ty).ok_or_else(|| {
                    syn::Error::new_spanned(
                        source_ty,
                        format!(
                        "Unsupported relation source field type for '{}.{}': expected String/uuid/int/float/bool (optionals allowed)",
                            struct_name, field_name
                        ),
                    )
                })?;

            let key_string_expr = match source_kind {
                RelationValueKind::String => quote! { value.clone() },
                RelationValueKind::Uuid => quote! { value.to_string() },
                _ => quote! { value.to_string() },
            };
            let sql_value_expr = match source_kind {
                RelationValueKind::String => {
                    quote! { ::graphql_orm::graphql::orm::SqlValue::String(value.clone()) }
                }
                RelationValueKind::Uuid => {
                    quote! { ::graphql_orm::graphql::orm::SqlValue::Uuid(*value) }
                }
                RelationValueKind::Int => {
                    quote! { ::graphql_orm::graphql::orm::SqlValue::Int((*value) as i64) }
                }
                RelationValueKind::Float => {
                    quote! { ::graphql_orm::graphql::orm::SqlValue::Float((*value).into()) }
                }
                RelationValueKind::Bool => {
                    quote! { ::graphql_orm::graphql::orm::SqlValue::Bool(*value) }
                }
            };

            let entity_key_pair_expr = if source_is_option {
                quote! {
                    entity.#source_field.as_ref().map(|value| {
                        (#key_string_expr, #sql_value_expr)
                    })
                }
            } else {
                quote! {
                    {
                        let value = &entity.#source_field;
                        Some((#key_string_expr, #sql_value_expr))
                    }
                }
            };

            let assign_expr = if source_is_option {
                if r.is_multiple {
                    quote! {
                        if let Some(value) = entity.#source_field.as_ref() {
                            let value = value;
                            let relation_key = #key_string_expr;
                            entity.#field_name = grouped.remove(&relation_key).unwrap_or_default();
                        } else {
                            entity.#field_name = Vec::new();
                        }
                    }
                } else {
                    quote! {
                        if let Some(value) = entity.#source_field.as_ref() {
                            let value = value;
                            let relation_key = #key_string_expr;
                            entity.#field_name = grouped.remove(&relation_key);
                        } else {
                            entity.#field_name = None;
                        }
                    }
                }
            } else if r.is_multiple {
                quote! {
                    let value = &entity.#source_field;
                    let relation_key = #key_string_expr;
                    entity.#field_name = grouped.remove(&relation_key).unwrap_or_default();
                }
            } else {
                quote! {
                    let value = &entity.#source_field;
                    let relation_key = #key_string_expr;
                    entity.#field_name = grouped.remove(&relation_key);
                }
            };

            let grouped_type = if r.is_multiple {
                quote! { std::collections::HashMap<String, Vec<#target_type>> }
            } else {
                quote! { std::collections::HashMap<String, #target_type> }
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
                    let mut unique_relation_keys: Vec<(String, ::graphql_orm::graphql::orm::SqlValue)> = Vec::new();
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
                            .map(|(_, value)| value.clone())
                            .collect::<Vec<_>>();
                        let placeholders = (0..unique_relation_keys.len())
                            .map(|index| <#target_type>::__gom_placeholder(index + 1))
                            .collect::<Vec<_>>();
                        let sql = format!(
                            "SELECT {}, CAST({} AS TEXT) AS __gom_relation_key FROM {} WHERE {} IN ({})",
                            <#target_type as ::graphql_orm::graphql::orm::DatabaseEntity>::column_names().join(", "),
                            #fk_column,
                            <#target_type as ::graphql_orm::graphql::orm::DatabaseEntity>::TABLE_NAME,
                            #fk_column,
                            placeholders.join(", ")
                        );

                        let rows = ::graphql_orm::graphql::orm::fetch_rows(pool, &sql, &bind_values).await?;
                        for row in rows {
                            use ::graphql_orm::sqlx::Row;
                            let relation_key: String = row.try_get("__gom_relation_key")?;
                            let related = <#target_type as ::graphql_orm::graphql::orm::FromSqlRow>::from_row(&row)?;
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
        impl ::graphql_orm::graphql::orm::RelationLoader for #struct_name {
            async fn load_relations(
                &mut self,
                pool: &#pool_type,
                selection: &[::graphql_orm::async_graphql::context::SelectionField<'_>],
            ) -> Result<(), ::graphql_orm::sqlx::Error> {
                Self::bulk_load_relations(std::slice::from_mut(self), pool, selection).await
            }

            async fn bulk_load_relations(
                entities: &mut [Self],
                pool: &#pool_type,
                selection: &[::graphql_orm::async_graphql::context::SelectionField<'_>],
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
