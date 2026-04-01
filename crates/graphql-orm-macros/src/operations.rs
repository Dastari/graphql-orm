use super::*;
use crate::backend::backend_database_type_tokens;
use crate::backend::backend_pool_type_tokens;
use crate::entity::{
    graphql_field_name, is_bool_type, is_byte_vec_type, is_option_type, is_uuid_type, is_vec_type,
    maybe_wrap_write_transform, option_inner_type, parse_entity_metadata, parse_field_metadata,
    type_path_last_ident,
};

pub(crate) fn generate_graphql_operations(
    input: &DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;
    let struct_name_str = struct_name.to_string();
    let pool_type = backend_pool_type_tokens();
    let database_type = backend_database_type_tokens();

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
    let rename_all_rule = entity_meta
        .graphql_rename_fields
        .as_deref()
        .or(entity_meta.serde_rename_all.as_deref());
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
    let graphql_create_input = syn::Ident::new(
        &format!("GraphQLCreate{}Input", struct_name),
        struct_name.span(),
    );
    let graphql_update_input = syn::Ident::new(
        &format!("GraphQLUpdate{}Input", struct_name),
        struct_name.span(),
    );
    let result_type = syn::Ident::new(&format!("{}Result", struct_name), struct_name.span());
    let changed_event =
        syn::Ident::new(&format!("{}ChangedEvent", struct_name), struct_name.span());

    // GraphQL operation names: public fields are camelCase, type names remain PascalCase.
    let list_query_name = plural_name.to_case(Case::Camel);
    let single_query_name = struct_name_str.to_case(Case::Camel);
    let create_mutation_name = format!("create{}", struct_name).to_case(Case::Camel);
    let update_mutation_name = format!("update{}", struct_name).to_case(Case::Camel);
    let update_many_mutation_name = format!("update{}", plural_name).to_case(Case::Camel);
    let delete_mutation_name = format!("delete{}", struct_name).to_case(Case::Camel);
    let delete_many_mutation_name = format!("delete{}", plural_name).to_case(Case::Camel);
    let subscription_name = format!("{}Changed", struct_name).to_case(Case::Camel);
    let entity_result_field_name = struct_name_str.to_case(Case::Camel);
    let update_many_result_type =
        syn::Ident::new(&format!("Update{}Result", plural_name), struct_name.span());
    let update_many_result_type_str = format!("Update{}Result", plural_name);
    let delete_many_result_type =
        syn::Ident::new(&format!("Delete{}Result", plural_name), struct_name.span());
    let delete_many_result_type_str = format!("Delete{}Result", plural_name);

    // Generate input fields (excluding primary key for create, all optional for update)
    let mut create_input_fields: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut graphql_create_input_fields: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut create_input_from_graphql_fields: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_input_fields: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut graphql_update_input_fields: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_input_from_graphql_fields: Vec<proc_macro2::TokenStream> = Vec::new();

    // For SQL generation
    let mut insert_columns: Vec<String> = Vec::new();
    let mut insert_default_columns: Vec<String> = Vec::new();
    let mut insert_default_exprs: Vec<String> = Vec::new();
    let mut insert_binds_graphql: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut insert_binds_repo: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_field_checks_graphql: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_field_checks_repo: Vec<proc_macro2::TokenStream> = Vec::new();
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
        let include_in_create =
            (!meta.is_primary_key || !auto_generated_pk) && !is_timestamp && meta.write;
        let include_generated_default_in_create = (!meta.is_primary_key || !auto_generated_pk)
            && !is_timestamp
            && !meta.write
            && meta.default.is_some();
        if include_in_create {
            let graphql_include_in_create =
                (!meta.skip_input && !meta.is_json_field) || meta.input_only;
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
                pub #field_name: #field_type,
            });
            if graphql_include_in_create {
                graphql_create_input_fields.push(quote! {
                    #[graphql(name = #graphql_name)]
                    pub #field_name: #field_type,
                });
                create_input_from_graphql_fields.push(quote! {
                    #field_name: input.#field_name,
                });
            } else {
                create_input_from_graphql_fields.push(quote! {
                    #field_name: ::std::default::Default::default(),
                });
            }

            // Track columns for INSERT
            insert_columns.push(db_col.clone());

            // Generate bind value push based on field type
            // We push to bind_values vector to avoid lifetime issues with ::graphql_orm::sqlx::query
            if meta.is_boolean_field || is_bool_type(field_type) {
                if is_option_type(field_type) {
                    let bind_tokens = quote! {
                        match input.#field_name {
                            Some(b) => bind_values.push(#struct_name::__gom_bool_sql_value(b)),
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                        }
                    };
                    insert_binds_graphql.push(bind_tokens.clone());
                    insert_binds_repo.push(bind_tokens);
                } else {
                    let bind_tokens = quote! {
                        bind_values.push(#struct_name::__gom_bool_sql_value(input.#field_name));
                    };
                    insert_binds_graphql.push(bind_tokens.clone());
                    insert_binds_repo.push(bind_tokens);
                }
            } else if meta.is_json_field
                || (is_vec_type(field_type) && !is_byte_vec_type(field_type))
            {
                if is_option_type(field_type) {
                    insert_binds_graphql.push(quote! {
                        match &input.#field_name {
                            Some(value) => bind_values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::async_graphql::Error>(value)?),
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::JsonNull),
                        }
                    });
                    insert_binds_repo.push(quote! {
                        match &input.#field_name {
                            Some(value) => bind_values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::sqlx::Error>(value)?),
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::JsonNull),
                        }
                    });
                } else {
                    insert_binds_graphql.push(quote! {
                        bind_values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::async_graphql::Error>(&input.#field_name)?);
                    });
                    insert_binds_repo.push(quote! {
                        bind_values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::sqlx::Error>(&input.#field_name)?);
                    });
                }
            } else if is_uuid_type(field_type) {
                if is_option_type(field_type) {
                    let bind_tokens = quote! {
                        match input.#field_name {
                            Some(v) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(v)),
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                        }
                    };
                    insert_binds_graphql.push(bind_tokens.clone());
                    insert_binds_repo.push(bind_tokens);
                } else {
                    let bind_tokens = quote! {
                        bind_values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(input.#field_name));
                    };
                    insert_binds_graphql.push(bind_tokens.clone());
                    insert_binds_repo.push(bind_tokens);
                }
            } else if option_inner_type(field_type)
                .and_then(type_path_last_ident)
                .is_some_and(|ident| {
                    matches!(
                        ident.to_string().as_str(),
                        "i8" | "i16"
                            | "i32"
                            | "i64"
                            | "isize"
                            | "u8"
                            | "u16"
                            | "u32"
                            | "u64"
                            | "usize"
                    )
                })
            {
                let bind_tokens = quote! {
                    match input.#field_name {
                        Some(v) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Int(v as i64)),
                        None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                    }
                };
                insert_binds_graphql.push(bind_tokens.clone());
                insert_binds_repo.push(bind_tokens);
            } else if option_inner_type(field_type)
                .and_then(type_path_last_ident)
                .is_some_and(|ident| matches!(ident.to_string().as_str(), "f32" | "f64"))
            {
                let bind_tokens = quote! {
                    match input.#field_name {
                        Some(v) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Float(v.into())),
                        None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                    }
                };
                insert_binds_graphql.push(bind_tokens.clone());
                insert_binds_repo.push(bind_tokens);
            } else if type_path_last_ident(field_type).is_some_and(|ident| {
                matches!(
                    ident.to_string().as_str(),
                    "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize"
                )
            }) {
                let bind_tokens = quote! {
                    bind_values.push(::graphql_orm::graphql::orm::SqlValue::Int(input.#field_name as i64));
                };
                insert_binds_graphql.push(bind_tokens.clone());
                insert_binds_repo.push(bind_tokens);
            } else if type_path_last_ident(field_type)
                .is_some_and(|ident| matches!(ident.to_string().as_str(), "f32" | "f64"))
            {
                let bind_tokens = quote! {
                    bind_values.push(::graphql_orm::graphql::orm::SqlValue::Float(input.#field_name.into()));
                };
                insert_binds_graphql.push(bind_tokens.clone());
                insert_binds_repo.push(bind_tokens);
            } else if is_option_type(field_type) {
                let value_expr =
                    maybe_wrap_write_transform(quote! { v.to_string() }, &meta.transform_write);
                if meta.transform_write.is_some() {
                    // Async transform requires .await — use a block
                    let bind_tokens = quote! {
                        match &input.#field_name {
                            Some(v) => {
                                let __transformed = #value_expr;
                                bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(__transformed));
                            }
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                        }
                    };
                    insert_binds_graphql.push(bind_tokens.clone());
                    insert_binds_repo.push(bind_tokens);
                } else {
                    let bind_tokens = quote! {
                        match &input.#field_name {
                            Some(v) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr)),
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                        }
                    };
                    insert_binds_graphql.push(bind_tokens.clone());
                    insert_binds_repo.push(bind_tokens);
                }
            } else {
                let value_expr = maybe_wrap_write_transform(
                    quote! { input.#field_name.to_string() },
                    &meta.transform_write,
                );
                if meta.transform_write.is_some() {
                    let bind_tokens = quote! {
                        {
                            let __transformed = #value_expr;
                            bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(__transformed));
                        }
                    };
                    insert_binds_graphql.push(bind_tokens.clone());
                    insert_binds_repo.push(bind_tokens);
                } else {
                    let bind_tokens = quote! {
                        bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr));
                    };
                    insert_binds_graphql.push(bind_tokens.clone());
                    insert_binds_repo.push(bind_tokens);
                }
            }
        } else if include_generated_default_in_create {
            insert_default_columns.push(db_col.clone());
            insert_default_exprs.push(
                meta.default
                    .clone()
                    .expect("generated create default must exist"),
            );
        }

        // For update: wrap in Option to make all fields optional (skip PK, timestamps, skip_input)
        // But #[input_only] overrides skip_input (allows write-only fields like encrypted credentials)
        let is_timestamp = rust_name == "created_at" || rust_name == "updated_at";
        if !meta.is_primary_key && !is_timestamp && meta.write {
            // All update fields are wrapped in Option (even if already optional)
            // This allows distinguishing between "not provided" and "set to null"
            let is_already_optional = is_option_type(field_type);
            let update_type = quote! { Option<#field_type> };
            let graphql_update_type = if let Some(inner_type) = option_inner_type(field_type) {
                quote! { ::graphql_orm::async_graphql::MaybeUndefined<#inner_type> }
            } else {
                quote! { #update_type }
            };
            let graphql_include_in_update =
                (!meta.skip_input && !meta.is_json_field) || meta.input_only;
            update_input_fields.push(quote! {
                pub #field_name: #update_type,
            });
            if graphql_include_in_update {
                graphql_update_input_fields.push(quote! {
                    #[graphql(name = #graphql_name)]
                    pub #field_name: #graphql_update_type,
                });
                if is_already_optional {
                    update_input_from_graphql_fields.push(quote! {
                        #field_name: input.#field_name.into(),
                    });
                } else {
                    update_input_from_graphql_fields.push(quote! {
                        #field_name: input.#field_name,
                    });
                }
            } else {
                update_input_from_graphql_fields.push(quote! {
                    #field_name: ::std::default::Default::default(),
                });
            }

            // Generate update field check
            let update_policy_check = if let Some(policy_key) = &meta.write_policy {
                quote! {
                    if let Some(ref val) = input.#field_name {
                        db.ensure_writable_field(
                            ctx,
                            #entity_name_lit,
                            #graphql_name,
                            Some(#policy_key),
                            Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
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
                    let update_tokens = quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            match val {
                                Some(b) => values.push(#struct_name::__gom_bool_sql_value(*b)),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                            }
                        }
                    };
                    update_field_checks_graphql.push(update_tokens.clone());
                    update_field_checks_repo.push(update_tokens);
                } else {
                    // Option<bool> case
                    let update_tokens = quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            values.push(#struct_name::__gom_bool_sql_value(*val));
                        }
                    };
                    update_field_checks_graphql.push(update_tokens.clone());
                    update_field_checks_repo.push(update_tokens);
                }
            } else if meta.is_json_field
                || (is_vec_type(field_type) && !is_byte_vec_type(field_type))
            {
                if is_already_optional {
                    update_field_checks_graphql.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            match val {
                                Some(value) => values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::async_graphql::Error>(value)?),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::JsonNull),
                            }
                        }
                    });
                    update_field_checks_repo.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            match val {
                                Some(value) => values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::sqlx::Error>(value)?),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::JsonNull),
                            }
                        }
                    });
                } else {
                    update_field_checks_graphql.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::async_graphql::Error>(val)?);
                        }
                    });
                    update_field_checks_repo.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::sqlx::Error>(val)?);
                        }
                    });
                }
            } else if is_uuid_type(field_type) {
                if is_already_optional {
                    let update_tokens = quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            match val {
                                Some(v) => values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(*v)),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                            }
                        }
                    };
                    update_field_checks_graphql.push(update_tokens.clone());
                    update_field_checks_repo.push(update_tokens);
                } else {
                    let update_tokens = quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(*val));
                        }
                    };
                    update_field_checks_graphql.push(update_tokens.clone());
                    update_field_checks_repo.push(update_tokens);
                }
            } else if is_already_optional
                && option_inner_type(field_type)
                    .and_then(type_path_last_ident)
                    .is_some_and(|ident| {
                        matches!(
                            ident.to_string().as_str(),
                            "i8" | "i16"
                                | "i32"
                                | "i64"
                                | "isize"
                                | "u8"
                                | "u16"
                                | "u32"
                                | "u64"
                                | "usize"
                        )
                    })
            {
                let update_tokens = quote! {
                    if let Some(ref val) = input.#field_name {
                        changed_fields.push(#db_col);
                        set_clauses.push(format!("{} = ?", #db_col));
                        match val {
                            Some(v) => values.push(::graphql_orm::graphql::orm::SqlValue::Int((*v) as i64)),
                            None => values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                        }
                    }
                };
                update_field_checks_graphql.push(update_tokens.clone());
                update_field_checks_repo.push(update_tokens);
            } else if is_already_optional
                && option_inner_type(field_type)
                    .and_then(type_path_last_ident)
                    .is_some_and(|ident| matches!(ident.to_string().as_str(), "f32" | "f64"))
            {
                let update_tokens = quote! {
                    if let Some(ref val) = input.#field_name {
                        changed_fields.push(#db_col);
                        set_clauses.push(format!("{} = ?", #db_col));
                        match val {
                            Some(v) => values.push(::graphql_orm::graphql::orm::SqlValue::Float((*v).into())),
                            None => values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                        }
                    }
                };
                update_field_checks_graphql.push(update_tokens.clone());
                update_field_checks_repo.push(update_tokens);
            } else if is_already_optional {
                // Field type is already Option<T>, update type is Option<Option<T>>
                let value_expr =
                    maybe_wrap_write_transform(quote! { v.to_string() }, &meta.transform_write);
                if meta.transform_write.is_some() {
                    let update_tokens = quote! {
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
                    };
                    update_field_checks_graphql.push(update_tokens.clone());
                    update_field_checks_repo.push(update_tokens);
                } else {
                    let update_tokens = quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            match val {
                                Some(v) => values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr)),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::Null),
                            }
                        }
                    };
                    update_field_checks_graphql.push(update_tokens.clone());
                    update_field_checks_repo.push(update_tokens);
                }
            } else if type_path_last_ident(field_type).is_some_and(|ident| {
                matches!(
                    ident.to_string().as_str(),
                    "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize"
                )
            }) {
                let update_tokens = quote! {
                    if let Some(ref val) = input.#field_name {
                        changed_fields.push(#db_col);
                        set_clauses.push(format!("{} = ?", #db_col));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Int((*val) as i64));
                    }
                };
                update_field_checks_graphql.push(update_tokens.clone());
                update_field_checks_repo.push(update_tokens);
            } else if type_path_last_ident(field_type)
                .is_some_and(|ident| matches!(ident.to_string().as_str(), "f32" | "f64"))
            {
                let update_tokens = quote! {
                    if let Some(ref val) = input.#field_name {
                        changed_fields.push(#db_col);
                        set_clauses.push(format!("{} = ?", #db_col));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Float((*val).into()));
                    }
                };
                update_field_checks_graphql.push(update_tokens.clone());
                update_field_checks_repo.push(update_tokens);
            } else {
                // Field type is T, update type is Option<T>
                let value_expr =
                    maybe_wrap_write_transform(quote! { val.to_string() }, &meta.transform_write);
                if meta.transform_write.is_some() {
                    let update_tokens = quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            let __transformed = #value_expr;
                            values.push(::graphql_orm::graphql::orm::SqlValue::String(__transformed));
                        }
                    };
                    update_field_checks_graphql.push(update_tokens.clone());
                    update_field_checks_repo.push(update_tokens);
                } else {
                    let update_tokens = quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col));
                            values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr));
                        }
                    };
                    update_field_checks_graphql.push(update_tokens.clone());
                    update_field_checks_repo.push(update_tokens);
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
        columns.extend(insert_default_columns.iter().cloned());
        columns.extend(insert_columns.iter().cloned());
        let mut placeholders = vec!["?".to_string()];
        placeholders.extend(insert_default_exprs.iter().cloned());
        placeholders.extend(insert_placeholders.iter().map(|value| value.to_string()));
        format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table_name,
            columns.join(", "),
            placeholders.join(", ")
        )
    } else {
        let mut columns = insert_default_columns.clone();
        columns.extend(insert_columns.iter().cloned());
        let mut placeholders = insert_default_exprs.clone();
        placeholders.extend(insert_placeholders.iter().map(|value| value.to_string()));
        format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table_name,
            columns.join(", "),
            placeholders.join(", ")
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
            pub node: #struct_name,
            /// A cursor for pagination
            pub cursor: String,
        }

        /// Connection containing edges and page info
        #[derive(::graphql_orm::async_graphql::SimpleObject, Debug, Clone)]
        #[graphql(name = #connection_type_str)]
        pub struct #connection_type {
            /// The edges in this connection
            pub edges: Vec<#edge_type>,
            /// Pagination information
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
        #[derive(Clone, Debug)]
        pub struct #create_input {
            #(#create_input_fields)*
        }

        /// Input for updating an existing #struct_name
        #[derive(Clone, Debug, Default)]
        pub struct #update_input {
            #(#update_input_fields)*
        }

        #[derive(::graphql_orm::async_graphql::InputObject, Clone, Debug)]
        #[graphql(name = #create_input_str)]
        struct #graphql_create_input {
            #(#graphql_create_input_fields)*
        }

        impl From<#graphql_create_input> for #create_input {
            fn from(input: #graphql_create_input) -> Self {
                Self {
                    #(#create_input_from_graphql_fields)*
                }
            }
        }

        #[derive(::graphql_orm::async_graphql::InputObject, Clone, Debug, Default)]
        #[graphql(name = #update_input_str)]
        struct #graphql_update_input {
            #(#graphql_update_input_fields)*
        }

        impl From<#graphql_update_input> for #update_input {
            fn from(input: #graphql_update_input) -> Self {
                Self {
                    #(#update_input_from_graphql_fields)*
                }
            }
        }

        /// Result type for #struct_name mutations
        #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
        #[graphql(name = #result_type_str)]
        pub struct #result_type {
            pub success: bool,
            pub error: Option<String>,
            #[graphql(name = #entity_result_field_name)]
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
            pub action: ::graphql_orm::graphql::orm::ChangeAction,
            pub id: #pk_type,
            #[graphql(name = #entity_result_field_name)]
            pub entity: Option<#struct_name>,
        }

        /// Result of bulk delete by Where filter
        #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
        #[graphql(name = #delete_many_result_type_str)]
        pub struct #delete_many_result_type {
            pub success: bool,
            pub error: Option<String>,
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
                #[graphql(name = "where")] where_input: Option<#where_input>,
                #[graphql(name = "orderBy")] order_by: Option<Vec<#order_by_input>>,
                #[graphql(name = "page")] page: Option<::graphql_orm::graphql::orm::PageInput>,
            ) -> ::graphql_orm::async_graphql::Result<#connection_type> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, DatabaseOrderBy, EntityQuery, FromSqlRow};
                use ::graphql_orm::graphql::auth::AuthExt;

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                ).await?;

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

                if db.row_policy().is_some() {
                    let base_query = query.clone();
                    let requested_page = page.clone();

                    let mut all_rows = base_query.fetch_all(pool).await
                        .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                    let mut visible_rows = Vec::new();
                    for row in all_rows.drain(..) {
                        if db.can_read_row(
                            Some(ctx),
                            #entity_name_lit,
                            <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                            ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                            &row as &(dyn ::std::any::Any + Send + Sync),
                        ).await? {
                            visible_rows.push(row);
                        }
                    }

                    let total = visible_rows.len() as i64;
                    let offset = requested_page.as_ref().map(|p| p.offset()).unwrap_or(0) as usize;
                    let limit = requested_page.as_ref().and_then(|p| p.limit()).map(|limit| limit as usize);

                    let paged_rows: Vec<#struct_name> = if offset >= visible_rows.len() {
                        Vec::new()
                    } else if let Some(limit) = limit {
                        visible_rows.into_iter().skip(offset).take(limit).collect()
                    } else {
                        visible_rows.into_iter().skip(offset).collect()
                    };

                    let has_next_page = (offset as i64 + paged_rows.len() as i64) < total;
                    let has_previous_page = offset > 0;

                    let mut generic_conn = ::graphql_orm::graphql::pagination::Connection {
                        edges: paged_rows.into_iter().enumerate().map(|(index, node)| {
                            ::graphql_orm::graphql::pagination::Edge {
                                cursor: ::graphql_orm::graphql::pagination::encode_cursor((offset + index) as i64),
                                node,
                            }
                        }).collect::<Vec<_>>(),
                        page_info: ::graphql_orm::graphql::pagination::PageInfo {
                            has_next_page,
                            has_previous_page,
                            start_cursor: None,
                            end_cursor: None,
                            total_count: Some(total),
                        },
                    };

                    generic_conn.page_info.start_cursor = generic_conn.edges.first().map(|edge| edge.cursor.clone());
                    generic_conn.page_info.end_cursor = generic_conn.edges.last().map(|edge| edge.cursor.clone());

                    #relation_preload_list

                    return Ok(#connection_type::from_generic(generic_conn));
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
                #[graphql(name = "id")] id: #pk_type,
            ) -> ::graphql_orm::async_graphql::Result<Option<#struct_name>> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};
                use ::graphql_orm::graphql::auth::AuthExt;

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                ).await?;

                let pk_col = #struct_name::PRIMARY_KEY;
                let mut entity = EntityQuery::<#struct_name>::new()
                    .where_clause(
                        &format!("{} = {}", pk_col, #struct_name::__gom_placeholder(1)),
                        #pk_bind_value
                    )
                    .fetch_one(pool)
                    .await
                    .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                if let Some(ref loaded) = entity {
                    if !db.can_read_row(
                        Some(ctx),
                        #entity_name_lit,
                        <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                        loaded as &(dyn ::std::any::Any + Send + Sync),
                    ).await? {
                        return Ok(None);
                    }
                }

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
                #[graphql(name = "input")] input: #graphql_create_input,
            ) -> ::graphql_orm::async_graphql::Result<#result_type> {
                use ::graphql_orm::graphql::auth::AuthExt;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                ).await?;
                let mut input: #create_input = input.into();
                db.run_before_create(
                    Some(ctx),
                    #entity_name_lit,
                    &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                ).await?;

                #created_pk_init
                let sql = #struct_name::__gom_rebind_sql(#insert_sql, 1);

                // Collect all values first
                let mut bind_values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                #(#create_policy_checks)*
                #prepend_pk_bind
                #(#insert_binds_graphql)*
                let mutation_fields = [#(#create_mutation_field_literals),*];
                let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&mutation_fields, &bind_values);
                let tx = pool.begin().await.map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::new(db, tx);
                hook_ctx.run_mutation_hook(
                    Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #created_pk_id_string,
                        changes: mutation_changes.clone(),
                        before_state: None,
                        after_state: None,
                    },
                ).await?;

                ::graphql_orm::graphql::orm::execute_with_binds_on(hook_ctx.executor(), &sql, &bind_values)
                    .await
                    .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                let entity = #struct_name::__gom_fetch_by_id_on(hook_ctx.executor(), &created_pk)
                    .await
                    .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?
                    .ok_or_else(|| ::graphql_orm::async_graphql::Error::new("Entity not found after creation"))?;
                let after_state = Some(
                    #struct_name::__gom_capture_entity_state(&entity)
                        .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?
                );

                hook_ctx.run_mutation_hook(
                    Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: entity.#pk_field.to_string(),
                        changes: mutation_changes.clone(),
                        before_state: None,
                        after_state,
                    },
                ).await?;

                #struct_name::__gom_queue_changed_event(
                    &mut hook_ctx,
                    ::graphql_orm::graphql::orm::ChangeAction::Created,
                    Some(&entity),
                );
                hook_ctx.commit_and_emit().await.map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;
                #notify_on_created
                Ok(#result_type::ok(entity))
            }

            /// Update an existing #struct_name_str
            #[graphql(name = #update_mutation_name)]
            async fn update(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "id")] id: #pk_type,
                #[graphql(name = "input")] input: #graphql_update_input,
            ) -> ::graphql_orm::async_graphql::Result<#result_type> {
                use ::graphql_orm::graphql::auth::AuthExt;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                ).await?;
                let mut input: #update_input = input.into();
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
                let current_entity = current_entity.expect("checked above");
                db.ensure_writable_row(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                    &current_entity as &(dyn ::std::any::Any + Send + Sync),
                ).await?;
                db.run_before_update(
                    Some(ctx),
                    #entity_name_lit,
                    Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                    &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                ).await?;

                // Build dynamic UPDATE SQL based on provided fields
                let mut set_clauses: Vec<String> = Vec::new();
                let mut changed_fields: Vec<&str> = Vec::new();
                let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();

                #(#update_policy_checks)*
                #(#update_field_checks_graphql)*

                // Update timestamp column when this entity defines one
                if #has_updated_at_column {
                    set_clauses.push(format!("updated_at = {}", #struct_name::__gom_current_epoch_expr()));
                }

                if set_clauses.is_empty() {
                    return Ok(#result_type::err("No fields to update"));
                }
                let before_state = #struct_name::__gom_capture_entity_state(&current_entity)
                    .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;
                let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                let tx = pool.begin().await.map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::new(db, tx);
                hook_ctx.run_mutation_hook(
                    Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: id.to_string(),
                        changes: mutation_changes.clone(),
                        before_state: Some(before_state.clone()),
                        after_state: None,
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

                let result = ::graphql_orm::graphql::orm::execute_with_binds_on(hook_ctx.executor(), &sql, &values).await;

                match result {
                    Ok(r) if r.rows_affected() > 0 => {
                        let entity = #struct_name::__gom_fetch_by_id_on(hook_ctx.executor(), &id)
                            .await
                            .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

                        match entity {
                            Some(entity) => {
                                let after_state = Some(
                                    #struct_name::__gom_capture_entity_state(&entity)
                                        .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?
                                );

                                hook_ctx.run_mutation_hook(
                                    Some(ctx),
                                    &::graphql_orm::graphql::orm::MutationEvent {
                                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                                        entity_name: #entity_name_lit,
                                        table_name: #table_name,
                                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                                        id: entity.#pk_field.to_string(),
                                        changes: mutation_changes.clone(),
                                        before_state: Some(before_state),
                                        after_state,
                                    },
                                ).await?;
                                #struct_name::__gom_queue_changed_event(
                                    &mut hook_ctx,
                                    ::graphql_orm::graphql::orm::ChangeAction::Updated,
                                    Some(&entity),
                                );
                                hook_ctx.commit_and_emit().await.map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

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
                #[graphql(name = "id")] id: #pk_type,
            ) -> ::graphql_orm::async_graphql::Result<#result_type> {
                use ::graphql_orm::graphql::auth::AuthExt;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, SqlValue};

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                let pool = db.pool();
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                ).await?;

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
                db.ensure_writable_row(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                    &entity as &(dyn ::std::any::Any + Send + Sync),
                ).await?;
                let before_state = #struct_name::__gom_capture_entity_state(&entity)
                    .map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;
                let tx = pool.begin().await.map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::new(db, tx);
                hook_ctx.run_mutation_hook(
                    Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: entity.#pk_field.to_string(),
                        changes: Vec::new(),
                        before_state: Some(before_state.clone()),
                        after_state: None,
                    },
                ).await?;

                let sql = #struct_name::__gom_rebind_sql(
                    &format!("DELETE FROM {} WHERE {} = ?", #table_name, #struct_name::PRIMARY_KEY),
                    1
                );
                let values = [#pk_bind_value];
                let result = ::graphql_orm::graphql::orm::execute_with_binds_on(hook_ctx.executor(), &sql, &values).await;

                match result {
                    Ok(r) if r.rows_affected() > 0 => {
                        hook_ctx.run_mutation_hook(
                            Some(ctx),
                            &::graphql_orm::graphql::orm::MutationEvent {
                                phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                                entity_name: #entity_name_lit,
                                table_name: #table_name,
                                metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                                id: entity.#pk_field.to_string(),
                                changes: Vec::new(),
                                before_state: Some(before_state),
                                after_state: None,
                            },
                        ).await?;
                        #struct_name::__gom_queue_changed_event(
                            &mut hook_ctx,
                            ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                            Some(&entity),
                        );
                        hook_ctx.commit_and_emit().await.map_err(|e| ::graphql_orm::async_graphql::Error::new(e.to_string()))?;

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
                #[graphql(name = "where")] where_input: Option<#where_input>,
                #[graphql(name = "input")] input: #graphql_update_input,
            ) -> ::graphql_orm::async_graphql::Result<#update_many_result_type> {
                use ::graphql_orm::graphql::auth::AuthExt;
                use ::graphql_orm::graphql::orm::{DatabaseFilter, EntityQuery};

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                ).await?;
                let input: #update_input = input.into();

                let filter = match where_input {
                    Some(ref f) if !f.is_empty() => f,
                    _ => return Ok(#update_many_result_type::err("Where filter is required for bulk update and must not be empty")),
                };

                match #struct_name::update_where(db, filter.clone(), input).await {
                    Ok(affected_count) => Ok(#update_many_result_type::ok(affected_count)),
                    Err(e) => Ok(#update_many_result_type::err(e.to_string())),
                }
            }

            /// Delete multiple #plural_name matching the given Where filter
            #[graphql(name = #delete_many_mutation_name)]
            async fn delete_many(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = "where")] where_input: Option<#where_input>,
            ) -> ::graphql_orm::async_graphql::Result<#delete_many_result_type> {
                use ::graphql_orm::graphql::auth::AuthExt;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, EntityQuery, FromSqlRow};

                let _user = ctx.auth_user()?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database>();
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                ).await?;

                let filter = match where_input {
                    Some(ref f) if !f.is_empty() => f,
                    _ => return Ok(#delete_many_result_type::err("Where filter is required for bulk delete and must not be empty")),
                };

                match #struct_name::delete_where(db, filter.clone()).await {
                    Ok(deleted_count) => Ok(#delete_many_result_type::ok(deleted_count)),
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
                #[graphql(name = "filter")] _filter: Option<::graphql_orm::graphql::orm::SubscriptionFilterInput>,
            ) -> ::graphql_orm::async_graphql::Result<impl ::graphql_orm::futures::Stream<Item = #changed_event>> {
                use ::graphql_orm::futures::StreamExt;
                use ::graphql_orm::graphql::auth::AuthExt;

                let _user = ctx.auth_user()?;
                let db = ctx.data::<::graphql_orm::db::Database>().map_err(|_| {
                    ::graphql_orm::async_graphql::Error::new(
                        "graphql-orm Database runtime not registered; build the schema with schema_builder(database) or add Database to schema data",
                    )
                })?;
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlSubscription,
                ).await?;

                let rx = db.ensure_event_sender::<#changed_event>().subscribe();

                use ::graphql_orm::tokio_stream::wrappers::BroadcastStream;

                Ok(BroadcastStream::new(rx).filter_map(move |result| async move {
                    match result {
                        Ok(event) => Some(event),
                        Err(_) => None,
                    }
                }))
            }
        }

        // ============================================================================
        // Repository Trait Implementation
        // ============================================================================

        /// Repository implementation for #struct_name
        ///
        /// Provides static async methods for common database operations.
        impl #struct_name {
            #[doc(hidden)]
            fn __gom_runtime_error(message: impl Into<String>) -> ::graphql_orm::sqlx::Error {
                ::graphql_orm::sqlx::Error::Protocol(message.into())
            }

            #[doc(hidden)]
            fn __gom_emit_changed_event(
                db: &::graphql_orm::db::Database,
                action: ::graphql_orm::graphql::orm::ChangeAction,
                entity: Option<&Self>,
            ) {
                if let Some(entity) = entity {
                    db.emit_event(#changed_event {
                        action,
                        id: entity.#pk_field.clone(),
                        entity: Some(entity.clone()),
                    });
                }
            }

            #[doc(hidden)]
            fn __gom_queue_changed_event(
                hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_>,
                action: ::graphql_orm::graphql::orm::ChangeAction,
                entity: Option<&Self>,
            ) {
                if let Some(entity) = entity {
                    hook_ctx.queue_event(#changed_event {
                        action,
                        id: entity.#pk_field.clone(),
                        entity: Some(entity.clone()),
                    });
                }
            }

            #[doc(hidden)]
            fn __gom_capture_entity_state(
                entity: &Self,
            ) -> Result<::graphql_orm::graphql::orm::EntityState, ::graphql_orm::sqlx::Error> {
                ::graphql_orm::graphql::orm::entity_state(entity)
            }

            #[doc(hidden)]
            async fn __gom_fetch_by_id_on<'e, E>(
                executor: E,
                id: &#pk_type_ty,
            ) -> Result<Option<Self>, ::graphql_orm::sqlx::Error>
            where
                E: ::graphql_orm::sqlx::Executor<'e, Database = #database_type>,
            {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, FromSqlRow, SqlValue};

                let sql = Self::__gom_rebind_sql(
                    &format!("SELECT * FROM {} WHERE {} = ?", #table_name, Self::PRIMARY_KEY),
                    1,
                );
                let values = [#pk_bind_value_ref];
                let rows = ::graphql_orm::graphql::orm::fetch_rows_on(executor, &sql, &values).await?;
                rows.first()
                    .map(<Self as ::graphql_orm::graphql::orm::FromSqlRow>::from_row)
                    .transpose()
            }

            #[doc(hidden)]
            async fn __gom_insert_with_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_>,
                mut input: #create_input,
            ) -> Result<Self, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, SqlValue};

                hook_ctx.database().run_before_create(
                    None,
                    #entity_name_lit,
                    &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                ).await.map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                #created_pk_init
                let sql = Self::__gom_rebind_sql(#insert_sql, 1);
                let mut bind_values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                #prepend_pk_bind
                #(#insert_binds_repo)*

                let before_state = None;
                hook_ctx
                    .run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: #created_pk_id_string,
                            changes: ::graphql_orm::graphql::orm::mutation_changes(&[#(#create_mutation_field_literals),*], &bind_values),
                            before_state,
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;

                ::graphql_orm::graphql::orm::execute_with_binds_on(hook_ctx.executor(), &sql, &bind_values).await?;
                let entity = Self::__gom_fetch_by_id_on(hook_ctx.executor(), &created_pk).await?
                    .ok_or(::graphql_orm::sqlx::Error::RowNotFound)?;
                let after_state = Some(Self::__gom_capture_entity_state(&entity)?);

                hook_ctx
                    .run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: entity.#pk_field.to_string(),
                            changes: ::graphql_orm::graphql::orm::mutation_changes(&[#(#create_mutation_field_literals),*], &bind_values),
                            before_state: None,
                            after_state,
                        },
                    )
                    .await
                    .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;

                Self::__gom_queue_changed_event(
                    hook_ctx,
                    ::graphql_orm::graphql::orm::ChangeAction::Created,
                    Some(&entity),
                );

                Ok(entity)
            }

            #[doc(hidden)]
            async fn __gom_update_by_id_with_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_>,
                id: &#pk_type_ty,
                mut input: #update_input,
            ) -> Result<Option<Self>, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                let current_entity = Self::__gom_fetch_by_id_on(hook_ctx.executor(), id).await?;
                let Some(current_entity) = current_entity else {
                    return Ok(None);
                };
                hook_ctx.database().run_before_update(
                    None,
                    #entity_name_lit,
                    Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                    &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                ).await.map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;

                let mut set_clauses: Vec<String> = Vec::new();
                let mut changed_fields: Vec<&str> = Vec::new();
                let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();

                #(#update_field_checks_repo)*

                if #has_updated_at_column {
                    set_clauses.push(format!("updated_at = {}", Self::__gom_current_epoch_expr()));
                }

                if set_clauses.is_empty() {
                    return Err(Self::__gom_runtime_error("No fields to update"));
                }

                let before_state = Self::__gom_capture_entity_state(&current_entity)?;
                let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: current_entity.#pk_field.to_string(),
                        changes: mutation_changes.clone(),
                        before_state: Some(before_state.clone()),
                        after_state: None,
                    },
                )
                .await
                .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;

                let sql = Self::__gom_rebind_sql(
                    &format!("UPDATE {} SET {} WHERE {} = ?", #table_name, set_clauses.join(", "), Self::PRIMARY_KEY),
                    1,
                );
                values.push(#pk_bind_value_ref);

                let result = ::graphql_orm::graphql::orm::execute_with_binds_on(hook_ctx.executor(), &sql, &values).await?;
                if result.rows_affected() == 0 {
                    return Ok(None);
                }

                let entity = Self::__gom_fetch_by_id_on(hook_ctx.executor(), id).await?;
                let Some(entity) = entity else {
                    return Ok(None);
                };
                let after_state = Some(Self::__gom_capture_entity_state(&entity)?);

                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: entity.#pk_field.to_string(),
                        changes: mutation_changes,
                        before_state: Some(before_state),
                        after_state,
                    },
                )
                .await
                .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;

                Self::__gom_queue_changed_event(
                    hook_ctx,
                    ::graphql_orm::graphql::orm::ChangeAction::Updated,
                    Some(&entity),
                );

                Ok(Some(entity))
            }

            #[doc(hidden)]
            async fn __gom_update_where_with_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_>,
                where_input: #where_input,
                mut input: #update_input,
            ) -> Result<i64, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, EntityQuery, FromSqlRow, SqlValue};

                if where_input.is_empty() {
                    return Err(Self::__gom_runtime_error("Where filter is required for bulk update and must not be empty"));
                }

                let matched_entities = EntityQuery::<Self>::new()
                    .filter(&where_input)
                    .fetch_all_on(hook_ctx.executor())
                    .await?;

                if matched_entities.is_empty() {
                    return Ok(0);
                }
                if let Some(first_entity) = matched_entities.first() {
                    hook_ctx.database().run_before_update(
                        None,
                        #entity_name_lit,
                        Some(first_entity as &(dyn ::std::any::Any + Send + Sync)),
                        &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                }

                let mut set_clauses: Vec<String> = Vec::new();
                let mut changed_fields: Vec<&str> = Vec::new();
                let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();

                #(#update_field_checks_repo)*

                if #has_updated_at_column {
                    set_clauses.push(format!("updated_at = {}", Self::__gom_current_epoch_expr()));
                }

                if set_clauses.is_empty() {
                    return Err(Self::__gom_runtime_error("No fields to update"));
                }

                let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                for entity in &matched_entities {
                    hook_ctx.run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: entity.#pk_field.to_string(),
                            changes: mutation_changes.clone(),
                            before_state: Some(Self::__gom_capture_entity_state(entity)?),
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                }

                let query = EntityQuery::<Self>::new().filter(&where_input);
                let (delete_sql, filter_values) = query.build_delete_sql();
                let where_clause = match delete_sql.split_once(" WHERE ") {
                    Some((_, clause)) => Self::__gom_rebind_sql(clause, values.len() + 1),
                    None => return Err(Self::__gom_runtime_error("Where filter produced empty SQL")),
                };

                let sql = format!(
                    "UPDATE {} SET {} WHERE {}",
                    #table_name,
                    set_clauses.join(", "),
                    where_clause
                );

                values.extend(filter_values);
                let result = ::graphql_orm::graphql::orm::execute_with_binds_on(hook_ctx.executor(), &sql, &values).await?;
                let affected = result.rows_affected() as i64;

                for previous in matched_entities {
                    if let Some(entity) = Self::__gom_fetch_by_id_on(hook_ctx.executor(), &previous.#pk_field).await? {
                        hook_ctx.run_mutation_hook(
                            None,
                            &::graphql_orm::graphql::orm::MutationEvent {
                                phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                                entity_name: #entity_name_lit,
                                table_name: #table_name,
                                metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                                id: entity.#pk_field.to_string(),
                                changes: mutation_changes.clone(),
                                before_state: Some(Self::__gom_capture_entity_state(&previous)?),
                                after_state: Some(Self::__gom_capture_entity_state(&entity)?),
                            },
                        )
                        .await
                        .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                        Self::__gom_queue_changed_event(
                            hook_ctx,
                            ::graphql_orm::graphql::orm::ChangeAction::Updated,
                            Some(&entity),
                        );
                    }
                }

                Ok(affected)
            }

            #[doc(hidden)]
            async fn __gom_delete_by_id_with_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_>,
                id: &#pk_type_ty,
            ) -> Result<bool, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, SqlValue};

                let entity = Self::__gom_fetch_by_id_on(hook_ctx.executor(), id).await?;
                let Some(entity) = entity else {
                    return Ok(false);
                };
                let before_state = Self::__gom_capture_entity_state(&entity)?;

                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: entity.#pk_field.to_string(),
                        changes: Vec::new(),
                        before_state: Some(before_state.clone()),
                        after_state: None,
                    },
                )
                .await
                .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;

                let sql = Self::__gom_rebind_sql(
                    &format!("DELETE FROM {} WHERE {} = ?", #table_name, Self::PRIMARY_KEY),
                    1,
                );
                let values = [#pk_bind_value_ref];
                let result = ::graphql_orm::graphql::orm::execute_with_binds_on(hook_ctx.executor(), &sql, &values).await?;
                if result.rows_affected() == 0 {
                    return Ok(false);
                }

                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: entity.#pk_field.to_string(),
                        changes: Vec::new(),
                        before_state: Some(before_state),
                        after_state: None,
                    },
                )
                .await
                .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;

                Self::__gom_queue_changed_event(
                    hook_ctx,
                    ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                    Some(&entity),
                );

                Ok(true)
            }

            #[doc(hidden)]
            async fn __gom_delete_where_with_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_>,
                where_input: #where_input,
            ) -> Result<i64, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, EntityQuery, FromSqlRow};

                if where_input.is_empty() {
                    return Err(Self::__gom_runtime_error("Where filter is required for bulk delete and must not be empty"));
                }

                let matched_entities = EntityQuery::<Self>::new()
                    .filter(&where_input)
                    .fetch_all_on(hook_ctx.executor())
                    .await?;

                if matched_entities.is_empty() {
                    return Ok(0);
                }

                for entity in &matched_entities {
                    hook_ctx.run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: entity.#pk_field.to_string(),
                            changes: Vec::new(),
                            before_state: Some(Self::__gom_capture_entity_state(entity)?),
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                }

                let mut query = EntityQuery::<Self>::new().filter(&where_input);
                let (sql, values) = query.build_delete_sql();
                let sql = Self::__gom_rebind_sql(&sql, 1);
                let result = ::graphql_orm::graphql::orm::execute_with_binds_on(hook_ctx.executor(), &sql, &values).await?;
                let deleted = result.rows_affected() as i64;

                for entity in matched_entities {
                    hook_ctx.run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: entity.#pk_field.to_string(),
                            changes: Vec::new(),
                            before_state: Some(Self::__gom_capture_entity_state(&entity)?),
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                    Self::__gom_queue_changed_event(
                        hook_ctx,
                        ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        Some(&entity),
                    );
                }

                Ok(deleted)
            }

            /// Insert a new entity record using the generated create input.
            pub async fn insert<P>(
                provider: &P,
                mut input: #create_input,
            ) -> Result<Self, ::graphql_orm::sqlx::Error>
            where
                P: ::graphql_orm::graphql::orm::PoolProvider + ::std::any::Any,
            {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                if let Some(db) = (provider as &dyn ::std::any::Any).downcast_ref::<::graphql_orm::db::Database>() {
                    db.ensure_entity_access(
                        None,
                        #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                    ).await.map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                    db.run_before_create(
                        None,
                        #entity_name_lit,
                        &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                    let tx = db.pool().begin().await?;
                    let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::new(db, tx);
                    let entity = Self::__gom_insert_with_mutation_context(&mut hook_ctx, input).await?;
                    hook_ctx.commit_and_emit().await?;
                    Ok(entity)
                } else {
                    let pool = provider.pool();
                    #created_pk_init
                    let sql = Self::__gom_rebind_sql(#insert_sql, 1);
                    let mut bind_values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                    #prepend_pk_bind
                    #(#insert_binds_repo)*
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

            /// Update a single entity by primary key using the generated update input.
            pub async fn update_by_id(
                db: &::graphql_orm::db::Database,
                id: &#pk_type_ty,
                mut input: #update_input,
            ) -> Result<Option<Self>, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                let pool = db.pool();
                db.ensure_entity_access(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                ).await.map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                let current_entity = EntityQuery::<Self>::new()
                    .where_clause(
                        &format!("{} = {}", Self::PRIMARY_KEY, Self::__gom_placeholder(1)),
                        #pk_bind_value_ref
                    )
                    .fetch_one(pool)
                    .await?;

                let Some(current_entity) = current_entity else {
                    return Ok(None);
                };
                db.ensure_writable_row(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                    &current_entity as &(dyn ::std::any::Any + Send + Sync),
                ).await.map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                db.run_before_update(
                    None,
                    #entity_name_lit,
                    Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                    &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                ).await.map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;

                let mut set_clauses: Vec<String> = Vec::new();
                let mut changed_fields: Vec<&str> = Vec::new();
                let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();

                #(#update_field_checks_repo)*

                if #has_updated_at_column {
                    set_clauses.push(format!("updated_at = {}", Self::__gom_current_epoch_expr()));
                }

                if set_clauses.is_empty() {
                    return Err(Self::__gom_runtime_error("No fields to update"));
                }

                let before_state = Self::__gom_capture_entity_state(&current_entity)?;
                let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                let tx = pool.begin().await?;
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::new(db, tx);
                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: current_entity.#pk_field.to_string(),
                        changes: mutation_changes.clone(),
                        before_state: Some(before_state.clone()),
                        after_state: None,
                    },
                )
                .await
                .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;

                let sql = Self::__gom_rebind_sql(
                    &format!("UPDATE {} SET {} WHERE {} = ?", #table_name, set_clauses.join(", "), Self::PRIMARY_KEY),
                    1,
                );
                values.push(#pk_bind_value_ref);

                let result = ::graphql_orm::graphql::orm::execute_with_binds_on(hook_ctx.executor(), &sql, &values).await?;
                if result.rows_affected() == 0 {
                    return Ok(None);
                }

                let entity = Self::__gom_fetch_by_id_on(hook_ctx.executor(), id).await?;

                let Some(entity) = entity else {
                    return Ok(None);
                };
                let after_state = Some(Self::__gom_capture_entity_state(&entity)?);

                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: entity.#pk_field.to_string(),
                        changes: mutation_changes,
                        before_state: Some(before_state),
                        after_state,
                    },
                )
                .await
                .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                Self::__gom_queue_changed_event(
                    &mut hook_ctx,
                    ::graphql_orm::graphql::orm::ChangeAction::Updated,
                    Some(&entity),
                );
                hook_ctx.commit_and_emit().await?;

                Ok(Some(entity))
            }

            /// Update multiple entities matching a typed where filter.
            pub async fn update_where(
                db: &::graphql_orm::db::Database,
                where_input: #where_input,
                mut input: #update_input,
            ) -> Result<i64, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, EntityQuery, FromSqlRow, SqlValue};

                if where_input.is_empty() {
                    return Err(Self::__gom_runtime_error("Where filter is required for bulk update and must not be empty"));
                }

                let pool = db.pool();
                db.ensure_entity_access(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                ).await.map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                let matched_entities = EntityQuery::<Self>::new()
                    .filter(&where_input)
                    .fetch_all(pool)
                    .await?;

                if matched_entities.is_empty() {
                    return Ok(0);
                }
                for entity in &matched_entities {
                    db.ensure_writable_row(
                        None,
                        #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                        entity as &(dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                }
                if let Some(first_entity) = matched_entities.first() {
                    db.run_before_update(
                        None,
                        #entity_name_lit,
                        Some(first_entity as &(dyn ::std::any::Any + Send + Sync)),
                        &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                }

                let mut set_clauses: Vec<String> = Vec::new();
                let mut changed_fields: Vec<&str> = Vec::new();
                let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();

                #(#update_field_checks_repo)*

                if #has_updated_at_column {
                    set_clauses.push(format!("updated_at = {}", Self::__gom_current_epoch_expr()));
                }

                if set_clauses.is_empty() {
                    return Err(Self::__gom_runtime_error("No fields to update"));
                }

                let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                let tx = pool.begin().await?;
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::new(db, tx);
                for entity in &matched_entities {
                    hook_ctx.run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: entity.#pk_field.to_string(),
                            changes: mutation_changes.clone(),
                            before_state: Some(Self::__gom_capture_entity_state(entity)?),
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                }

                let query = EntityQuery::<Self>::new().filter(&where_input);
                let (delete_sql, filter_values) = query.build_delete_sql();
                let where_clause = match delete_sql.split_once(" WHERE ") {
                    Some((_, clause)) => Self::__gom_rebind_sql(clause, values.len() + 1),
                    None => return Err(Self::__gom_runtime_error("Where filter produced empty SQL")),
                };

                let sql = format!(
                    "UPDATE {} SET {} WHERE {}",
                    #table_name,
                    set_clauses.join(", "),
                    where_clause
                );

                values.extend(filter_values);
                let result = ::graphql_orm::graphql::orm::execute_with_binds_on(hook_ctx.executor(), &sql, &values).await?;
                let affected = result.rows_affected() as i64;

                for previous in matched_entities {
                    if let Some(entity) = Self::__gom_fetch_by_id_on(hook_ctx.executor(), &previous.#pk_field).await? {
                        hook_ctx.run_mutation_hook(
                            None,
                            &::graphql_orm::graphql::orm::MutationEvent {
                                phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                                entity_name: #entity_name_lit,
                                table_name: #table_name,
                                metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                                id: entity.#pk_field.to_string(),
                                changes: mutation_changes.clone(),
                                before_state: Some(Self::__gom_capture_entity_state(&previous)?),
                                after_state: Some(Self::__gom_capture_entity_state(&entity)?),
                            },
                        )
                        .await
                        .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                        Self::__gom_queue_changed_event(
                            &mut hook_ctx,
                            ::graphql_orm::graphql::orm::ChangeAction::Updated,
                            Some(&entity),
                        );
                    }
                }
                hook_ctx.commit_and_emit().await?;

                Ok(affected)
            }

            /// Delete a single entity by primary key.
            pub async fn delete_by_id(
                db: &::graphql_orm::db::Database,
                id: &#pk_type_ty,
            ) -> Result<bool, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, SqlValue};

                let pool = db.pool();
                db.ensure_entity_access(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                ).await.map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                let entity = EntityQuery::<Self>::new()
                    .where_clause(
                        &format!("{} = {}", Self::PRIMARY_KEY, Self::__gom_placeholder(1)),
                        #pk_bind_value_ref
                    )
                    .fetch_one(pool)
                    .await?;

                let Some(entity) = entity else {
                    return Ok(false);
                };
                db.ensure_writable_row(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                    &entity as &(dyn ::std::any::Any + Send + Sync),
                ).await.map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                let before_state = Self::__gom_capture_entity_state(&entity)?;
                let tx = pool.begin().await?;
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::new(db, tx);

                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: entity.#pk_field.to_string(),
                        changes: Vec::new(),
                        before_state: Some(before_state.clone()),
                        after_state: None,
                    },
                )
                .await
                .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;

                let sql = Self::__gom_rebind_sql(
                    &format!("DELETE FROM {} WHERE {} = ?", #table_name, Self::PRIMARY_KEY),
                    1,
                );
                let values = [#pk_bind_value_ref];
                let result = ::graphql_orm::graphql::orm::execute_with_binds_on(hook_ctx.executor(), &sql, &values).await?;
                if result.rows_affected() == 0 {
                    return Ok(false);
                }

                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: entity.#pk_field.to_string(),
                        changes: Vec::new(),
                        before_state: Some(before_state),
                        after_state: None,
                    },
                )
                .await
                .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                Self::__gom_queue_changed_event(
                    &mut hook_ctx,
                    ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                    Some(&entity),
                );
                hook_ctx.commit_and_emit().await?;

                Ok(true)
            }

            /// Delete multiple entities matching a typed where filter.
            pub async fn delete_where(
                db: &::graphql_orm::db::Database,
                where_input: #where_input,
            ) -> Result<i64, ::graphql_orm::sqlx::Error> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, EntityQuery, FromSqlRow};

                if where_input.is_empty() {
                    return Err(Self::__gom_runtime_error("Where filter is required for bulk delete and must not be empty"));
                }

                let pool = db.pool();
                db.ensure_entity_access(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                ).await.map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                let matched_entities = EntityQuery::<Self>::new()
                    .filter(&where_input)
                    .fetch_all(pool)
                    .await?;

                if matched_entities.is_empty() {
                    return Ok(0);
                }
                for entity in &matched_entities {
                    db.ensure_writable_row(
                        None,
                        #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                        entity as &(dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                }

                let tx = pool.begin().await?;
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::new(db, tx);
                for entity in &matched_entities {
                    hook_ctx.run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: entity.#pk_field.to_string(),
                            changes: Vec::new(),
                            before_state: Some(Self::__gom_capture_entity_state(entity)?),
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                }

                let mut query = EntityQuery::<Self>::new().filter(&where_input);
                let (sql, values) = query.build_delete_sql();
                let sql = Self::__gom_rebind_sql(&sql, 1);
                let result = ::graphql_orm::graphql::orm::execute_with_binds_on(hook_ctx.executor(), &sql, &values).await?;
                let deleted = result.rows_affected() as i64;

                for entity in &matched_entities {
                    hook_ctx.run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: entity.#pk_field.to_string(),
                            changes: Vec::new(),
                            before_state: Some(Self::__gom_capture_entity_state(entity)?),
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(|e| Self::__gom_runtime_error(format!("{e:?}")))?;
                    Self::__gom_queue_changed_event(
                        &mut hook_ctx,
                        ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        Some(entity),
                    );
                }
                hook_ctx.commit_and_emit().await?;

                Ok(deleted)
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

        impl ::graphql_orm::graphql::orm::MutationContextInsert for #struct_name {
            type CreateInput = #create_input;

            fn insert_in_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_>,
                input: Self::CreateInput,
            ) -> ::graphql_orm::futures::future::BoxFuture<'a, Result<Self, ::graphql_orm::sqlx::Error>> {
                Box::pin(async move { Self::__gom_insert_with_mutation_context(hook_ctx, input).await })
            }
        }

        impl ::graphql_orm::graphql::orm::MutationContextUpdateById for #struct_name {
            type Id = #pk_type_ty;
            type UpdateInput = #update_input;

            fn update_by_id_in_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_>,
                id: &'a Self::Id,
                input: Self::UpdateInput,
            ) -> ::graphql_orm::futures::future::BoxFuture<'a, Result<Option<Self>, ::graphql_orm::sqlx::Error>> {
                Box::pin(async move { Self::__gom_update_by_id_with_mutation_context(hook_ctx, id, input).await })
            }
        }

        impl ::graphql_orm::graphql::orm::MutationContextUpdateWhere for #struct_name {
            type WhereInput = #where_input;
            type UpdateInput = #update_input;

            fn update_where_in_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_>,
                where_input: Self::WhereInput,
                input: Self::UpdateInput,
            ) -> ::graphql_orm::futures::future::BoxFuture<'a, Result<i64, ::graphql_orm::sqlx::Error>> {
                Box::pin(async move { Self::__gom_update_where_with_mutation_context(hook_ctx, where_input, input).await })
            }
        }

        impl ::graphql_orm::graphql::orm::MutationContextDeleteById for #struct_name {
            type Id = #pk_type_ty;

            fn delete_by_id_in_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_>,
                id: &'a Self::Id,
            ) -> ::graphql_orm::futures::future::BoxFuture<'a, Result<bool, ::graphql_orm::sqlx::Error>> {
                Box::pin(async move { Self::__gom_delete_by_id_with_mutation_context(hook_ctx, id).await })
            }
        }

        impl ::graphql_orm::graphql::orm::MutationContextDeleteWhere for #struct_name {
            type WhereInput = #where_input;

            fn delete_where_in_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_>,
                where_input: Self::WhereInput,
            ) -> ::graphql_orm::futures::future::BoxFuture<'a, Result<i64, ::graphql_orm::sqlx::Error>> {
                Box::pin(async move { Self::__gom_delete_where_with_mutation_context(hook_ctx, where_input).await })
            }
        }

        impl ::graphql_orm::graphql::orm::MutationContextFindById for #struct_name {
            type Id = #pk_type_ty;

            fn find_by_id_in_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_>,
                id: &'a Self::Id,
            ) -> ::graphql_orm::futures::future::BoxFuture<'a, Result<Option<Self>, ::graphql_orm::sqlx::Error>> {
                Box::pin(async move { Self::__gom_fetch_by_id_on(hook_ctx.executor(), id).await })
            }
        }
    })
}
