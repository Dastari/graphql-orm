use super::*;
use crate::backend::backend_pool_type_tokens;
use crate::entity::{
    graphql_field_name, is_bool_type, is_byte_vec_type, is_option_type, is_uuid_type, is_vec_type,
    maybe_wrap_write_transform, parse_entity_metadata, parse_field_metadata,
};

pub(crate) fn generate_graphql_operations(
    input: &DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;
    let struct_name_str = struct_name.to_string();
    let pool_type = backend_pool_type_tokens();

    let data = match &input.data {
        Data::Struct(data) => data,
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "GraphQLOperations can only be derived for structs",
            ));
        }
    };

    let fields = match &data.fields {
        Fields::Named(fields) => &fields.named,
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "GraphQLOperations requires named fields",
            ));
        }
    };

    let entity_meta = parse_entity_metadata(&input.attrs)?;
    let rename_all_rule = entity_meta.serde_rename_all.as_deref();
    let table_name = entity_meta.table_name.as_deref().unwrap_or("unknown");
    let plural_name = entity_meta
        .plural_name
        .clone()
        .unwrap_or_else(|| format!("{}s", struct_name));

    // Generate optional post-mutation hook code if a hook path is configured.
    //
    // Expected signature:
    // `async fn(&::graphql_orm::async_graphql::Context<'_>, &#pool_type, &#Entity, ChangeAction)
    //      -> ::graphql_orm::async_graphql::Result<()>`
    let notify_handler_path = if let Some(ref notify_handler) = entity_meta.notify_handler {
        Some(syn::parse_str::<syn::Path>(notify_handler).map_err(|_| {
            syn::Error::new(
                struct_name.span(),
                "graphql_entity notify/notify_with must be a valid Rust path string",
            )
        })?)
    } else {
        None
    };
    let notify_on_created = if let Some(ref notify_handler) = notify_handler_path {
        quote! {
            #notify_handler(ctx, pool, &entity, ::graphql_orm::graphql::orm::ChangeAction::Created).await?;
        }
    } else {
        quote! {}
    };
    let notify_on_updated = if let Some(ref notify_handler) = notify_handler_path {
        quote! {
            #notify_handler(ctx, pool, &entity, ::graphql_orm::graphql::orm::ChangeAction::Updated).await?;
        }
    } else {
        quote! {}
    };
    let notify_on_deleted = if let Some(ref notify_handler) = notify_handler_path {
        quote! {
            #notify_handler(ctx, pool, &entity, ::graphql_orm::graphql::orm::ChangeAction::Deleted).await?;
        }
    } else {
        quote! {}
    };
    let entity_name_lit = struct_name_str.clone();

    // Find primary key field
    let mut pk_field_name: Option<syn::Ident> = None;
    let mut pk_type_ty: Option<syn::Type> = None;
    for field in fields {
        let meta = parse_field_metadata(field)?;
        if meta.is_primary_key {
            pk_field_name = Some(field.ident.clone().unwrap());
            pk_type_ty = Some(field.ty.clone());
            break;
        }
    }
    let pk_field = pk_field_name
        .clone()
        .unwrap_or_else(|| syn::Ident::new("id", struct_name.span()));
    let pk_type_ty: syn::Type = pk_type_ty.unwrap_or_else(|| syn::parse_quote!(String));
    let pk_type = quote! { #pk_type_ty };
    let auto_generated_pk = pk_field == syn::Ident::new("id", pk_field.span());
    let pk_is_uuid = is_uuid_type(&pk_type_ty);
    let pk_bind_value = if pk_is_uuid {
        quote! { ::graphql_orm::graphql::orm::SqlValue::Uuid(id) }
    } else {
        quote! { ::graphql_orm::graphql::orm::SqlValue::String(id.to_string()) }
    };
    let pk_bind_value_ref = if pk_is_uuid {
        quote! { ::graphql_orm::graphql::orm::SqlValue::Uuid(*id) }
    } else {
        quote! { ::graphql_orm::graphql::orm::SqlValue::String(id.to_string()) }
    };
    let created_pk_value = if pk_is_uuid {
        quote! { ::graphql_orm::graphql::orm::SqlValue::Uuid(created_pk) }
    } else {
        quote! { ::graphql_orm::graphql::orm::SqlValue::String(created_pk.clone()) }
    };
    let created_pk_id_string = quote! { created_pk.to_string() };

    // Generate type names
    let queries_struct = syn::Ident::new(&format!("{}Queries", struct_name), struct_name.span());
    let mutations_struct =
        syn::Ident::new(&format!("{}Mutations", struct_name), struct_name.span());
    let subscriptions_struct =
        syn::Ident::new(&format!("{}Subscriptions", struct_name), struct_name.span());
    let where_input = syn::Ident::new(&format!("{}WhereInput", struct_name), struct_name.span());
    let order_by_input =
        syn::Ident::new(&format!("{}OrderByInput", struct_name), struct_name.span());
    let create_input = syn::Ident::new(&format!("Create{}Input", struct_name), struct_name.span());
    let update_input = syn::Ident::new(&format!("Update{}Input", struct_name), struct_name.span());
    let result_type = syn::Ident::new(&format!("{}Result", struct_name), struct_name.span());
    let changed_event =
        syn::Ident::new(&format!("{}ChangedEvent", struct_name), struct_name.span());

    // GraphQL operation names (PascalCase)
    let list_query_name = &plural_name;
    let single_query_name = &struct_name_str;
    let create_mutation_name = format!("Create{}", struct_name);
    let update_mutation_name = format!("Update{}", struct_name);
    let update_many_mutation_name = format!("Update{}", plural_name);
    let delete_mutation_name = format!("Delete{}", struct_name);
    let delete_many_mutation_name = format!("Delete{}", plural_name);
    let subscription_name = format!("{}Changed", struct_name);
    let update_many_result_type =
        syn::Ident::new(&format!("Update{}Result", plural_name), struct_name.span());
    let update_many_result_type_str = format!("Update{}Result", plural_name);
    let delete_many_result_type =
        syn::Ident::new(&format!("Delete{}Result", plural_name), struct_name.span());
    let delete_many_result_type_str = format!("Delete{}Result", plural_name);

    // Generate input fields (excluding primary key for create, all optional for update)
    let mut create_input_fields: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_input_fields: Vec<proc_macro2::TokenStream> = Vec::new();

    // For SQL generation
    let mut insert_columns: Vec<String> = Vec::new();
    let mut insert_binds: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_field_checks: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut create_policy_checks: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_policy_checks: Vec<proc_macro2::TokenStream> = Vec::new();

    // Track string-filterable fields for search_similar
    let mut string_filterable_fields: Vec<(syn::Ident, bool)> = Vec::new(); // (field_name, is_option)

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;
        let meta = parse_field_metadata(field)?;

        // Skip relations and computed fields
        if meta.is_relation || meta.skip_db {
            continue;
        }

        let rust_name = field_name.to_string();
        let graphql_name = graphql_field_name(&meta, &rust_name, rename_all_rule);
        let db_col = meta.db_column.clone().unwrap_or_else(|| rust_name.clone());

        // Track string-filterable fields for fuzzy search
        if meta.filter && meta.filterable.as_deref() == Some("string") {
            string_filterable_fields.push((field_name.clone(), is_option_type(field_type)));
        }

        // Skip auto-generated primary key, timestamps, and skip_input fields (e.g. password_hash)
        // But #[input_only] overrides skip_input (allows write-only fields like encrypted credentials)
        let is_timestamp = rust_name == "created_at" || rust_name == "updated_at";
        let include_in_create = (!meta.is_primary_key || !auto_generated_pk)
            && !is_timestamp
            && meta.write
            && (!meta.skip_input || meta.input_only);
        if include_in_create {
            let create_policy_check = if let Some(policy_key) = &meta.write_policy {
                quote! {
                    db.ensure_writable_field(
                        ctx,
                        #entity_name_lit,
                        #graphql_name,
                        Some(#policy_key),
                        None,
                        Some(&input.#field_name as &(dyn ::std::any::Any + Send + Sync)),
                    ).await?;
                }
            } else {
                quote! {}
            };
            create_policy_checks.push(create_policy_check);
            // For create: use the field type directly (required fields stay required)
            create_input_fields.push(quote! {
                #[graphql(name = #graphql_name)]
                pub #field_name: #field_type,
            });

            // Track columns for INSERT
            insert_columns.push(db_col.clone());

            // Generate bind value push based on field type
            // We push to bind_values vector to avoid lifetime issues with ::graphql_orm::sqlx::query
            if meta.is_boolean_field || is_bool_type(field_type) {
                if is_option_type(field_type) {
                    insert_binds.push(quote! {
                        match input.#field_name {
                            Some(b) => bind_values.push(#struct_name::__gom_bool_sql_value(b)),
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                        }
                    });
                } else {
                    insert_binds.push(quote! {
                        bind_values.push(#struct_name::__gom_bool_sql_value(input.#field_name));
                    });
                }
            } else if meta.is_json_field
                || (is_vec_type(field_type) && !is_byte_vec_type(field_type))
            {
                insert_binds.push(quote! {
                    bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(
                        serde_json::to_string(&input.#field_name).unwrap_or_else(|_| "[]".to_string())
                    ));
                });
            } else if is_uuid_type(field_type) {
                if is_option_type(field_type) {
                    insert_binds.push(quote! {
                        match input.#field_name {
                            Some(v) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(v)),
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                        }
                    });
                } else {
                    insert_binds.push(quote! {
                        bind_values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(input.#field_name));
                    });
                }
            } else if is_option_type(field_type) {
                let value_expr =
                    maybe_wrap_write_transform(quote! { v.to_string() }, &meta.transform_write);
                if meta.transform_write.is_some() {
                    // Async transform requires .await — use a block
                    insert_binds.push(quote! {
                        match &input.#field_name {
                            Some(v) => {
                                let __transformed = #value_expr;
                                bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(__transformed));
                            }
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                        }
                    });
                } else {
                    insert_binds.push(quote! {
                        match &input.#field_name {
                            Some(v) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr)),
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                        }
                    });
                }
            } else {
                let value_expr = maybe_wrap_write_transform(
                    quote! { input.#field_name.to_string() },
                    &meta.transform_write,
                );
                if meta.transform_write.is_some() {
                    insert_binds.push(quote! {
                        {
                            let __transformed = #value_expr;
                            bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(__transformed));
                        }
                    });
                } else {
                    insert_binds.push(quote! {
                        bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr));
                    });
                }
            }
        }

        // For update: wrap in Option to make all fields optional (skip PK, timestamps, skip_input)
        // But #[input_only] overrides skip_input (allows write-only fields like encrypted credentials)
        let is_timestamp = rust_name == "created_at" || rust_name == "updated_at";
        if !meta.is_primary_key
            && !is_timestamp
            && meta.write
            && (!meta.skip_input || meta.input_only)
        {
            // All update fields are wrapped in Option (even if already optional)
            // This allows distinguishing between "not provided" and "set to null"
            let update_type = quote! { Option<#field_type> };

            update_input_fields.push(quote! {
                #[graphql(name = #graphql_name)]
                pub #field_name: #update_type,
            });

            // Generate update field check
            let is_already_optional = is_option_type(field_type);
            let update_policy_check = if let Some(policy_key) = &meta.write_policy {
                quote! {
                    if let Some(ref val) = input.#field_name {
                        db.ensure_writable_field(
                            ctx,
                            #entity_name_lit,
                            #graphql_name,
                            Some(#policy_key),
                            current_entity
                                .as_ref()
                                .map(|entity| entity as &(dyn ::std::any::Any + Send + Sync)),
                            Some(val as &(dyn ::std::any::Any + Send + Sync)),
                        ).await?;
                    }
                }
            } else {
                quote! {}
            };
            update_policy_checks.push(update_policy_check);

            if meta.is_boolean_field || is_bool_type(field_type) {
                if is_already_optional {
                    // Option<Option<bool>> case
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            match val {
                                Some(b) => values.push(#struct_name::__gom_bool_sql_value(*b)),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                            }
                        }
                    });
                } else {
                    // Option<bool> case
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            values.push(#struct_name::__gom_bool_sql_value(*val));
                        }
                    });
                }
            } else if meta.is_json_field
                || (is_vec_type(field_type) && !is_byte_vec_type(field_type))
            {
                update_field_checks.push(quote! {
                    if let Some(ref val) = input.#field_name {
                        changed_fields.push(#db_col);
                        set_clauses.push(format!("{} = ?", #db_col));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(
                            serde_json::to_string(val).unwrap_or_else(|_| "[]".to_string())
                        ));
                    }
                });
            } else if is_uuid_type(field_type) {
                if is_already_optional {
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            match val {
                                Some(v) => values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(*v)),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                            }
                        }
                    });
                } else {
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(*val));
                        }
                    });
                }
            } else if is_already_optional {
                // Field type is already Option<T>, update type is Option<Option<T>>
                let value_expr =
                    maybe_wrap_write_transform(quote! { v.to_string() }, &meta.transform_write);
                if meta.transform_write.is_some() {
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            match val {
                                Some(v) => {
                                    let __transformed = #value_expr;
                                    values.push(::graphql_orm::graphql::orm::SqlValue::String(__transformed));
                                }
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                            }
                        }
                    });
                } else {
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            match val {
                                Some(v) => values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr)),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                            }
                        }
                    });
                }
            } else {
                // Field type is T, update type is Option<T>
                let value_expr =
                    maybe_wrap_write_transform(quote! { val.to_string() }, &meta.transform_write);
                if meta.transform_write.is_some() {
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            let __transformed = #value_expr;
                            values.push(::graphql_orm::graphql::orm::SqlValue::String(__transformed));
                        }
                    });
                } else {
                    update_field_checks.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr));
                        }
                    });
                }
            }
        }
    }

    let has_updated_at_column = fields.iter().any(|f| {
        parse_field_metadata(f)
            .ok()
            .filter(|m| !m.is_relation && !m.skip_db)
            .and_then(|m| {
                f.ident
                    .as_ref()
                    .map(|ident| m.db_column.unwrap_or_else(|| ident.to_string()))
            })
            .is_some_and(|col| col == "updated_at")
    });

    // Build INSERT SQL template
    let insert_placeholders: Vec<&str> = insert_columns.iter().map(|_| "?").collect();
    let insert_sql = if auto_generated_pk {
        let mut columns = vec!["id".to_string()];
        columns.extend(insert_columns.iter().cloned());
        let mut placeholders = vec!["?".to_string()];
        placeholders.extend(insert_placeholders.iter().map(|value| value.to_string()));
        format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table_name,
            columns.join(", "),
            placeholders.join(", ")
        )
    } else {
        format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table_name,
            insert_columns.join(", "),
            insert_placeholders.join(", ")
        )
    };
    let create_mutation_fields = if auto_generated_pk {
        let mut fields = vec!["id".to_string()];
        fields.extend(insert_columns.iter().cloned());
        fields
    } else {
        insert_columns.clone()
    };
    let create_mutation_field_literals: Vec<syn::LitStr> = create_mutation_fields
        .iter()
        .map(|field| syn::LitStr::new(field, struct_name.span()))
        .collect();
    let created_pk_init = if auto_generated_pk {
        if pk_is_uuid {
            quote! {
                let created_pk = ::graphql_orm::uuid::Uuid::new_v4();
            }
        } else {
            quote! {
                let created_pk = ::graphql_orm::uuid::Uuid::new_v4().to_string();
            }
        }
    } else {
        quote! {
            let created_pk = input.#pk_field.clone();
        }
    };
    let prepend_pk_bind = if auto_generated_pk {
        quote! {
            bind_values.push(#created_pk_value);
        }
    } else {
        quote! {}
    };

    // Column list for SQL (unused now but kept for reference)
    let column_names: Vec<String> = fields
        .iter()
        .filter_map(|f| {
            let meta = parse_field_metadata(f).ok()?;
            if meta.is_relation || meta.skip_db {
                return None;
            }
            let name = f.ident.as_ref()?.to_string();
            Some(meta.db_column.unwrap_or(name))
        })
        .collect();
    let _columns_str = column_names.join(", ");

    // Generate additional type names
    let edge_type = syn::Ident::new(&format!("{}Edge", struct_name), struct_name.span());
    let connection_type =
        syn::Ident::new(&format!("{}Connection", struct_name), struct_name.span());
    let edge_type_str = format!("{}Edge", struct_name);
    let connection_type_str = format!("{}Connection", struct_name);
    let create_input_str = format!("Create{}Input", struct_name);
    let update_input_str = format!("Update{}Input", struct_name);
    let result_type_str = format!("{}Result", struct_name);
    let changed_event_str = format!("{}ChangedEvent", struct_name);
    let has_relations = fields
        .iter()
        .filter_map(|f| parse_field_metadata(f).ok())
        .any(|m| m.is_relation);
    let relation_preload_list = if has_relations {
        quote! {
            let selection = ctx.field().selection_set().collect::<Vec<_>>();
            if !generic_conn.edges.is_empty() {
                let cursors = generic_conn
                    .edges
                    .iter()
                    .map(|edge| edge.cursor.clone())
                    .collect::<Vec<_>>();
                let mut entities = std::mem::take(&mut generic_conn.edges)
                    .into_iter()
                    .map(|edge| edge.node)
                    .collect::<Vec<_>>();
                <#struct_name as ::graphql_orm::graphql::orm::RelationLoader>::bulk_load_relations(
                    &mut entities,
                    pool,
                    &selection,
                )
                .await
                .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;
                generic_conn.edges = cursors
                    .into_iter()
                    .zip(entities.into_iter())
                    .map(|(cursor, node)| ::graphql_orm::graphql::pagination::Edge { cursor, node })
                    .collect();
            }
        }
    } else {
        quote! {}
    };
    let relation_preload_single = if has_relations {
        quote! {
            if let Some(entity) = entity.as_mut() {
                let selection = ctx.field().selection_set().collect::<Vec<_>>();
                <#struct_name as ::graphql_orm::graphql::orm::RelationLoader>::load_relations(
                    entity,
                    pool,
                    &selection,
                )
                .await
                .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;
            }
        }
    } else {
        quote! {}
    };

    // Generate match arms for searchable fields (used in search_similar)
    let searchable_field_arms: Vec<proc_macro2::TokenStream> = string_filterable_fields
        .iter()
        .map(|(field_name, is_option)| {
            let field_str = field_name.to_string();
            if *is_option {
                quote! {
                    #field_str => entity.#field_name.as_deref(),
                }
            } else {
                quote! {
                    #field_str => Some(entity.#field_name.as_str()),
                }
            }
        })
        .collect();

    let searchable_field_match = if searchable_field_arms.is_empty() {
        quote! { None }
    } else {
        quote! {
            match field {
                #(#searchable_field_arms)*
                _ => None,
            }
        }
    };

    Ok(quote! {
        // ============================================================================
        // Connection/Edge Types (for pagination)
        // ============================================================================

        /// Edge containing a node and cursor
        #[derive(::graphql_orm::async_graphql::SimpleObject, Debug, Clone)]
        #[graphql(name = #edge_type_str)]
        pub struct #edge_type {
            /// The item at the end of the edge
            #[graphql(name = "Node")]
            pub node: #struct_name,
            /// A cursor for pagination
            #[graphql(name = "Cursor")]
            pub cursor: String,
        }

        /// Connection containing edges and page info
        #[derive(::graphql_orm::async_graphql::SimpleObject, Debug, Clone)]
        #[graphql(name = #connection_type_str)]
        pub struct #connection_type {
            /// The edges in this connection
            #[graphql(name = "Edges")]
            pub edges: Vec<#edge_type>,
            /// Pagination information
            #[graphql(name = "PageInfo")]
            pub page_info: ::graphql_orm::graphql::pagination::PageInfo,
        }

        impl #connection_type {
            /// Create from a generic Connection
            pub fn from_generic(conn: ::graphql_orm::graphql::pagination::Connection<#struct_name>) -> Self {
                Self {
                    edges: conn.edges.into_iter().map(|e| #edge_type {
                        node: e.node,
                        cursor: e.cursor,
                    }).collect(),
                    page_info: conn.page_info,
                }
            }

            /// Create an empty connection
            pub fn empty() -> Self {
                Self {
                    edges: Vec::new(),
                    page_info: ::graphql_orm::graphql::pagination::PageInfo::default(),
                }
            }
        }

        // ============================================================================
        // Create/Update Input Types
        // ============================================================================

        /// Input for creating a new #struct_name
        #[derive(::graphql_orm::async_graphql::InputObject, Clone, Debug)]
        #[graphql(name = #create_input_str)]
        pub struct #create_input {
            #(#create_input_fields)*
        }

        /// Input for updating an existing #struct_name
        #[derive(::graphql_orm::async_graphql::InputObject, Clone, Debug, Default)]
        #[graphql(name = #update_input_str)]
        pub struct #update_input {
            #(#update_input_fields)*
        }

        /// Result type for #struct_name mutations
        #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
        #[graphql(name = #result_type_str)]
        pub struct #result_type {
            #[graphql(name = "Success")]
            pub success: bool,
            #[graphql(name = "Error")]
            pub error: Option<String>,
            #[graphql(name = #struct_name_str)]
            pub entity: Option<#struct_name>,
        }

        impl #result_type {
            /// Create a successful result with the entity
            pub fn ok(entity: #struct_name) -> Self {
                Self { success: true, error: None, entity: Some(entity) }
            }
            /// Create an error result
            pub fn err(msg: impl Into<String>) -> Self {
                Self { success: false, error: Some(msg.into()), entity: None }
            }
        }

        /// Event for #struct_name changes (subscriptions)
        #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject, serde::Serialize, serde::Deserialize)]
        #[graphql(name = #changed_event_str)]
        pub struct #changed_event {
            #[graphql(name = "Action")]
            pub action: ::graphql_orm::graphql::orm::ChangeAction,
            #[graphql(name = "Id")]
            pub id: #pk_type,
            #[graphql(name = #struct_name_str)]
            pub entity: Option<#struct_name>,
        }

        /// Result of bulk delete by Where filter
        #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
        #[graphql(name = #delete_many_result_type_str)]
        pub struct #delete_many_result_type {
            pub success: bool,
            pub error: Option<String>,
            #[graphql(name = "DeletedCount")]
            pub deleted_count: i64,
        }

        impl #delete_many_result_type {
            pub fn ok(deleted_count: i64) -> Self {
                Self { success: true, error: None, deleted_count }
            }
            pub fn err(msg: impl Into<String>) -> Self {
                Self { success: false, error: Some(msg.into()), deleted_count: 0 }
            }
        }

        /// Result of bulk update by Where filter
        #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
        #[graphql(name = #update_many_result_type_str)]
        pub struct #update_many_result_type {
            pub success: bool,
            pub error: Option<String>,
            pub affected_count: i64,
        }

        impl #update_many_result_type {
            pub fn ok(affected_count: i64) -> Self {
                Self { success: true, error: None, affected_count }
            }
            pub fn err(msg: impl Into<String>) -> Self {
                Self { success: false, error: Some(msg.into()), affected_count: 0 }
            }
        }

        // ============================================================================
        // Query Struct
        // ============================================================================

        /// Generated queries for #struct_name
        #[derive(Default)]
        pub struct #queries_struct;

        #[::graphql_orm::async_graphql::Object]
        impl #queries_struct {
            /// Get a list of #plural_name with optional filtering, sorting, and pagination
            #[graphql(name = #list_query_name)]
            async fn list(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Where")] where_input: Option<#where_input>,
                #[graphql(name = "OrderBy")] order_by: Option<Vec<#order_by_input>>,
                #[graphql(name = "Page")] page: Option<::graphql_orm::graphql::orm::PageInput>,
            ) -> ::graphql_orm::async_graphql::Result<#connection_type> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, DatabaseOrderBy, EntityQuery, FromSqlRow};
                use ::graphql_orm::graphql::auth::AuthExt;

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();

                let mut query = EntityQuery::<#struct_name>::new();

                if let Some(ref filter) = where_input {
                    query = query.filter(filter);
                }

                if let Some(ref orders) = order_by {
                    for order in orders {
                        query = query.order_by(order);
                    }
                }

                if query.order_clauses.is_empty() {
                    query = query.default_order();
                }

                if let Some(ref p) = page {
                    query = query.paginate(p);
                }

                let mut generic_conn = query.fetch_connection(pool).await
                    .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                #relation_preload_list

                Ok(#connection_type::from_generic(generic_conn))
            }

            /// Get a single #struct_name_str by ID
            #[graphql(name = #single_query_name)]
            async fn get_by_id(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Id")] id: #pk_type,
            ) -> ::graphql_orm::async_graphql::Result<Option<#struct_name>> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};
                use ::graphql_orm::graphql::auth::AuthExt;

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();

                let pk_col = #struct_name::PRIMARY_KEY;
                let mut entity = EntityQuery::<#struct_name>::new()
                    .where_clause(
                        &format!("{} = {}", pk_col, #struct_name::__gom_placeholder(1)),
                        #pk_bind_value
                    )
                    .fetch_one(pool)
                    .await
                    .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                #relation_preload_single

                Ok(entity)
            }
        }

        // ============================================================================
        // Mutation Struct
        // ============================================================================

        /// Generated mutations for #struct_name
        #[derive(Default)]
        pub struct #mutations_struct;

        #[::graphql_orm::async_graphql::Object]
        impl #mutations_struct {
            /// Create a new #struct_name_str
            #[graphql(name = #create_mutation_name)]
            async fn create(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Input")] input: #create_input,
            ) -> ::graphql_orm::async_graphql::Result<#result_type> {
                use ::graphql_orm::graphql::auth::AuthExt;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();

                #created_pk_init
                let sql = #struct_name::__gom_rebind_sql(#insert_sql, 1);

                // Collect all values first
                let mut bind_values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                #(#create_policy_checks)*
                #prepend_pk_bind
                #(#insert_binds)*
                let mutation_fields = vec![#(#create_mutation_field_literals),*];
                let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&mutation_fields, &bind_values);
                db.run_mutation_hook(
                    ctx,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        id: #created_pk_id_string,
                        changes: mutation_changes.clone(),
                    },
                ).await?;

                // Execute using our helper that handles lifetimes properly
                let result = ::graphql_orm::graphql::orm::execute_with_binds(&sql, &bind_values, pool).await;

                match result {
                    Ok(_) => {
                        // Fetch the created entity
                        let entity = EntityQuery::<#struct_name>::new()
                            .where_clause(
                                &format!("{} = {}", #struct_name::PRIMARY_KEY, #struct_name::__gom_placeholder(1)),
                                #created_pk_value
                            )
                            .fetch_one(pool)
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?
                            .ok_or_else(|| ::graphql_orm::async_graphql::Error::new("Entity not found after creation"))?;

                        // Broadcast entity change event if subscription channel is configured
                        if let Ok(tx) = ctx.data::<::graphql_orm::tokio::sync::broadcast::Sender<#changed_event>>() {
                            let _ = tx.send(#changed_event {
                                action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                                id: entity.#pk_field.clone(),
                                entity: Some(entity.clone()),
                            });
                        }

                        db.run_mutation_hook(
                            ctx,
                            &::graphql_orm::graphql::orm::MutationEvent {
                                phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                                entity_name: #entity_name_lit,
                                table_name: #table_name,
                                id: entity.#pk_field.to_string(),
                                changes: mutation_changes.clone(),
                            },
                        ).await?;

                        // Invoke optional post-mutation hook.
                        #notify_on_created

                        Ok(#result_type::ok(entity))
                    }
                    Err(e) => Ok(#result_type::err(e.to_string())),
                }
            }

            /// Update an existing #struct_name_str
            #[graphql(name = #update_mutation_name)]
            async fn update(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Id")] id: #pk_type,
                #[graphql(name = "Input")] input: #update_input,
            ) -> ::graphql_orm::async_graphql::Result<#result_type> {
                use ::graphql_orm::graphql::auth::AuthExt;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();
                let current_entity = EntityQuery::<#struct_name>::new()
                    .where_clause(
                        &format!("{} = {}", #struct_name::PRIMARY_KEY, #struct_name::__gom_placeholder(1)),
                        #pk_bind_value
                    )
                    .fetch_one(pool)
                    .await
                    .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                if current_entity.is_none() {
                    return Ok(#result_type::err("Entity not found"));
                }

                // Build dynamic UPDATE SQL based on provided fields
                let mut set_clauses: Vec<String> = Vec::new();
                let mut changed_fields: Vec<&str> = Vec::new();
                let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();

                #(#update_policy_checks)*
                #(#update_field_checks)*

                // Update timestamp column when this entity defines one
                if #has_updated_at_column {
                    set_clauses.push(format!("updated_at = {}", #struct_name::__gom_current_epoch_expr()));
                }

                if set_clauses.is_empty() {
                    return Ok(#result_type::err("No fields to update"));
                }
                let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                db.run_mutation_hook(
                    ctx,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        id: id.to_string(),
                        changes: mutation_changes.clone(),
                    },
                ).await?;

                let sql = #struct_name::__gom_rebind_sql(&format!(
                    "UPDATE {} SET {} WHERE {} = ?",
                    #table_name,
                    set_clauses.join(", "),
                    #struct_name::PRIMARY_KEY
                ), 1);

                // Add the ID to the values for the WHERE clause
                values.push(#pk_bind_value);

                let result = ::graphql_orm::graphql::orm::execute_with_binds(&sql, &values, pool).await;

                match result {
                    Ok(r) if r.rows_affected() > 0 => {
                        // Fetch the updated entity
                        let entity = EntityQuery::<#struct_name>::new()
                            .where_clause(
                                &format!("{} = {}", #struct_name::PRIMARY_KEY, #struct_name::__gom_placeholder(1)),
                                #pk_bind_value
                            )
                            .fetch_one(pool)
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                        match entity {
                            Some(entity) => {
                                // Broadcast entity change event if subscription channel is configured
                                if let Ok(tx) = ctx.data::<::graphql_orm::tokio::sync::broadcast::Sender<#changed_event>>() {
                                    let _ = tx.send(#changed_event {
                                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                                        id: entity.#pk_field.clone(),
                                        entity: Some(entity.clone()),
                                    });
                                }

                                db.run_mutation_hook(
                                    ctx,
                                    &::graphql_orm::graphql::orm::MutationEvent {
                                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                                        entity_name: #entity_name_lit,
                                        table_name: #table_name,
                                        id: entity.#pk_field.to_string(),
                                        changes: mutation_changes.clone(),
                                    },
                                ).await?;

                                // Invoke optional post-mutation hook.
                                #notify_on_updated

                                Ok(#result_type::ok(entity))
                            },
                            None => Ok(#result_type::err("Entity not found after update")),
                        }
                    }
                    Ok(_) => Ok(#result_type::err("Entity not found")),
                    Err(e) => Ok(#result_type::err(e.to_string())),
                }
            }

            /// Delete a #struct_name_str
            #[graphql(name = #delete_mutation_name)]
            async fn delete(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Id")] id: #pk_type,
            ) -> ::graphql_orm::async_graphql::Result<#result_type> {
                use ::graphql_orm::graphql::auth::AuthExt;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, SqlValue};

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();

                // Fetch entity before deletion for notification purposes
                let entity = EntityQuery::<#struct_name>::new()
                    .where_clause(
                        &format!("{} = {}", #struct_name::PRIMARY_KEY, #struct_name::__gom_placeholder(1)),
                        #pk_bind_value
                    )
                    .fetch_one(pool)
                    .await
                    .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                if entity.is_none() {
                    return Ok(#result_type::err("Entity not found"));
                }
                let entity = entity.unwrap();
                db.run_mutation_hook(
                    ctx,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        id: entity.#pk_field.to_string(),
                        changes: Vec::new(),
                    },
                ).await?;

                let sql = #struct_name::__gom_rebind_sql(
                    &format!("DELETE FROM {} WHERE {} = ?", #table_name, #struct_name::PRIMARY_KEY),
                    1
                );
                let values = vec![#pk_bind_value];
                let result = ::graphql_orm::graphql::orm::execute_with_binds(&sql, &values, pool).await;

                match result {
                    Ok(r) if r.rows_affected() > 0 => {
                        // Broadcast entity change event if subscription channel is configured
                        if let Ok(tx) = ctx.data::<::graphql_orm::tokio::sync::broadcast::Sender<#changed_event>>() {
                            let _ = tx.send(#changed_event {
                                action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                                id: entity.#pk_field.clone(),
                                entity: Some(entity.clone()),
                            });
                        }

                        db.run_mutation_hook(
                            ctx,
                            &::graphql_orm::graphql::orm::MutationEvent {
                                phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                                entity_name: #entity_name_lit,
                                table_name: #table_name,
                                id: entity.#pk_field.to_string(),
                                changes: Vec::new(),
                            },
                        ).await?;

                        // Invoke optional post-mutation hook.
                        #notify_on_deleted

                        Ok(#result_type {
                            success: true,
                            error: None,
                            entity: None
                        })
                    },
                    Ok(_) => Ok(#result_type::err("Entity not found")),
                    Err(e) => Ok(#result_type::err(e.to_string())),
                }
            }

            /// Update multiple #plural_name matching the given Where filter
            #[graphql(name = #update_many_mutation_name)]
            async fn update_many(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Where")] where_input: Option<#where_input>,
                #[graphql(name = "Input")] input: #update_input,
            ) -> ::graphql_orm::async_graphql::Result<#update_many_result_type> {
                use ::graphql_orm::graphql::auth::AuthExt;
                use ::graphql_orm::graphql::orm::{DatabaseFilter, EntityQuery};

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();

                let filter = match where_input {
                    Some(ref f) if !f.is_empty() => f,
                    _ => return Ok(#update_many_result_type::err("Where filter is required for bulk update and must not be empty")),
                };

                // Build dynamic UPDATE SQL based on provided fields
                let mut set_clauses: Vec<String> = Vec::new();
                let mut changed_fields: Vec<&str> = Vec::new();
                let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();

                #(#update_field_checks)*

                // Update timestamp column when this entity defines one
                if #has_updated_at_column {
                    set_clauses.push(format!("updated_at = {}", #struct_name::__gom_current_epoch_expr()));
                }

                if set_clauses.is_empty() {
                    return Ok(#update_many_result_type::err("No fields to update"));
                }

                // Reuse EntityQuery WHERE SQL construction and bind values.
                let query = EntityQuery::<#struct_name>::new().filter(filter);
                let (delete_sql, filter_values) = query.build_delete_sql();
                let where_clause = match delete_sql.split_once(" WHERE ") {
                    Some((_, clause)) => #struct_name::__gom_rebind_sql(clause, values.len() + 1),
                    None => return Ok(#update_many_result_type::err("Where filter produced empty SQL")),
                };

                let sql = format!(
                    "UPDATE {} SET {} WHERE {}",
                    #table_name,
                    set_clauses.join(", "),
                    where_clause
                );

                values.extend(filter_values);
                let result = ::graphql_orm::graphql::orm::execute_with_binds(&sql, &values, pool).await;

                match result {
                    Ok(r) => Ok(#update_many_result_type::ok(r.rows_affected() as i64)),
                    Err(e) => Ok(#update_many_result_type::err(e.to_string())),
                }
            }

            /// Delete multiple #plural_name matching the given Where filter
            #[graphql(name = #delete_many_mutation_name)]
            async fn delete_many(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Where")] where_input: Option<#where_input>,
            ) -> ::graphql_orm::async_graphql::Result<#delete_many_result_type> {
                use ::graphql_orm::graphql::auth::AuthExt;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, EntityQuery, FromSqlRow};

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();

                let filter = match where_input {
                    Some(ref f) if !f.is_empty() => f,
                    _ => return Ok(#delete_many_result_type::err("Where filter is required for bulk delete and must not be empty")),
                };

                let mut query = EntityQuery::<#struct_name>::new().filter(filter);
                let (sql, values) = query.build_delete_sql();
                let sql = #struct_name::__gom_rebind_sql(&sql, 1);

                let result = ::graphql_orm::graphql::orm::execute_with_binds(&sql, &values, pool).await;

                match result {
                    Ok(r) => Ok(#delete_many_result_type::ok(r.rows_affected() as i64)),
                    Err(e) => Ok(#delete_many_result_type::err(e.to_string())),
                }
            }
        }

        // ============================================================================
        // Subscription Struct
        // ============================================================================

        /// Generated subscriptions for #struct_name
        #[derive(Default)]
        pub struct #subscriptions_struct;

        #[::graphql_orm::async_graphql::Subscription]
        impl #subscriptions_struct {
            /// Subscribe to #struct_name_str changes
            #[graphql(name = #subscription_name)]
            async fn on_changed(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "Filter")] _filter: Option<::graphql_orm::graphql::orm::SubscriptionFilterInput>,
            ) -> ::graphql_orm::async_graphql::Result<impl ::graphql_orm::futures::Stream<Item = #changed_event>> {
                use ::graphql_orm::futures::stream::{self, StreamExt};
                use ::graphql_orm::graphql::auth::AuthExt;

                let _user = ctx.auth_user()?;

                // Try to get the broadcast channel for this entity type
                // If not available, return an empty stream (subscription not enabled)
                let maybe_events = ctx.data_opt::<::graphql_orm::tokio::sync::broadcast::Sender<#changed_event>>();

                Ok(match maybe_events {
                    None => {
                        // Return empty stream if no broadcast channel is configured
                        stream::empty().left_stream()
                    }
                    Some(events) => {
                        let rx = events.subscribe();

                        use ::graphql_orm::tokio_stream::wrappers::BroadcastStream;

                        BroadcastStream::new(rx)
                            .filter_map(move |result| async move {
                                match result {
                                    Ok(event) => Some(event),
                                    Err(_) => None,
                                }
                            })
                            .right_stream()
                    }
                })
            }
        }

        // ============================================================================
        // Repository Trait Implementation
        // ============================================================================

        /// Repository implementation for #struct_name
        ///
        /// Provides static async methods for common database operations.
        impl #struct_name {
            /// Insert a new entity record using the generated create input.
            pub async fn insert(
                pool: &#pool_type,
                input: #create_input,
            ) -> Result<Self, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                #created_pk_init
                let sql = Self::__gom_rebind_sql(#insert_sql, 1);

                let mut bind_values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                #prepend_pk_bind
                #(#insert_binds)*

                ::graphql_orm::graphql::orm::execute_with_binds(&sql, &bind_values, pool).await?;

                EntityQuery::<Self>::new()
                    .where_clause(
                        &format!("{} = {}", <Self as DatabaseEntity>::PRIMARY_KEY, Self::__gom_placeholder(1)),
                        #created_pk_value,
                    )
                    .fetch_one(pool)
                    .await?
                    .ok_or(::graphql_orm::sqlx::Error::RowNotFound)
            }

            /// Find all entities matching the given filter
            pub fn query<'a>(pool: &'a #pool_type) -> ::graphql_orm::graphql::orm::FindQuery<'a, Self, #where_input, #order_by_input> {
                ::graphql_orm::graphql::orm::FindQuery::new(pool)
            }

            /// Find entity by ID
            pub async fn get(pool: &#pool_type, id: &#pk_type_ty) -> Result<Option<Self>, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                EntityQuery::<Self>::new()
                    .where_clause(
                        &format!("{} = {}", <Self as DatabaseEntity>::PRIMARY_KEY, Self::__gom_placeholder(1)),
                        #pk_bind_value_ref
                    )
                    .fetch_one(pool)
                    .await
            }

            /// Count entities matching the given filter
            pub fn count_query<'a>(pool: &'a #pool_type) -> ::graphql_orm::graphql::orm::CountQuery<'a, #where_input> {
                use ::graphql_orm::graphql::orm::DatabaseEntity;
                ::graphql_orm::graphql::orm::CountQuery::new(pool, <Self as DatabaseEntity>::TABLE_NAME)
            }

            /// Search entities with fuzzy/similar text matching
            ///
            /// # Arguments
            /// * `pool` - Database connection pool
            /// * `field` - Name of the field to search (snake_case)
            /// * `query` - The search query text
            /// * `threshold` - Minimum similarity score (0.0-1.0, recommended: 0.5-0.7)
            /// * `filter` - Optional additional filter to apply
            /// * `limit` - Maximum number of results to return
            ///
            /// # Returns
            /// Vector of (entity, score) tuples, sorted by score descending
            ///
            /// # Example
            /// ```rust,ignore
            /// let matches = Entity::search_similar(
            ///     &pool,
            ///     "name",
            ///     "example",
            ///     0.6,
            ///     Some(EntityWhereInput {
            ///         name: Some(StringFilter::contains("ex")),
            ///         ..Default::default()
            ///     }),
            ///     Some(25),
            /// ).await?;
            ///
            /// for (entity, score) in matches {
            ///     println!("{}: {:.2}", entity.name, score);
            /// }
            /// ```
            pub async fn search_similar(
                pool: &#pool_type,
                field: &str,
                query: &str,
                threshold: f64,
                filter: Option<#where_input>,
                limit: Option<i64>,
            ) -> Result<Vec<(Self, f64)>, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::graphql::orm::FuzzyMatcher;

                // Fetch candidates (optionally filtered)
                let mut q = Self::query(pool);
                if let Some(f) = filter {
                    q = q.filter(f);
                }
                // Fetch more than limit to account for fuzzy filtering
                if let Some(l) = limit {
                    q = q.limit(l * 5);
                }
                let candidates = q.fetch_all().await?;

                // Score with fuzzy matcher
                let matcher = FuzzyMatcher::new(query).with_threshold(threshold);
                let mut results = matcher.filter_and_score(candidates, |entity| {
                    Self::get_searchable_field(entity, field)
                });

                // Apply limit
                if let Some(l) = limit {
                    results.truncate(l as usize);
                }

                Ok(results.into_iter().map(|m| (m.entity, m.score)).collect())
            }

            /// Get a searchable field value by name (for fuzzy matching)
            #[doc(hidden)]
            fn get_searchable_field<'a>(entity: &'a Self, field: &str) -> Option<&'a str> {
                #searchable_field_match
            }
        }
    })
}
