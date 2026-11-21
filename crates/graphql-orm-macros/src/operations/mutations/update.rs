use crate::dependent_deletes::Boundary;

use proc_macro2::TokenStream;
use quote::quote;

pub(in crate::operations) struct UpdateInput<'a> {
    pub(in crate::operations) update_input_str: &'a str,
    pub(in crate::operations) field_case_rule: &'a str,
    pub(in crate::operations) graphql_update_input: &'a syn::Ident,
    pub(in crate::operations) graphql_update_input_fields: &'a [TokenStream],
    pub(in crate::operations) update_input: &'a syn::Ident,
    pub(in crate::operations) update_input_from_graphql_fields: &'a [TokenStream],
    pub(in crate::operations) update_operation_description: &'a str,
    pub(in crate::operations) update_mutation_name: &'a str,
    pub(in crate::operations) update_requires_scopes: &'a TokenStream,
    pub(in crate::operations) authenticated_directive: &'a TokenStream,
    pub(in crate::operations) id_arg_name: &'a str,
    pub(in crate::operations) id_arg_description: &'a str,
    pub(in crate::operations) pk_type: &'a TokenStream,
    pub(in crate::operations) input_arg_name: &'a str,
    pub(in crate::operations) input_arg_description: &'a str,
    pub(in crate::operations) expected_version_param: &'a TokenStream,
    pub(in crate::operations) result_type: &'a syn::Ident,
    pub(in crate::operations) expected_version_binding: &'a TokenStream,
    pub(in crate::operations) resolver_auth_mode: &'a TokenStream,
    pub(in crate::operations) rls_request_guard: &'a TokenStream,
    pub(in crate::operations) update_scope_enforcement: &'a TokenStream,
    pub(in crate::operations) dependent_deletes: bool,
    pub(in crate::operations) backend_marker: &'a TokenStream,
    pub(in crate::operations) entity_name_lit: &'a str,
    pub(in crate::operations) struct_name: &'a syn::Ident,
    pub(in crate::operations) update_client_supplied: &'a TokenStream,
    pub(in crate::operations) sql_dialect: &'a TokenStream,
    pub(in crate::operations) pk_bind_value: &'a TokenStream,
    pub(in crate::operations) update_policy_checks: &'a [TokenStream],
    pub(in crate::operations) update_assignments_graphql_call: &'a TokenStream,
    pub(in crate::operations) updated_at_assignment: &'a TokenStream,
    pub(in crate::operations) version_assignment: &'a TokenStream,
    pub(in crate::operations) table_name: &'a str,
    pub(in crate::operations) pk_id_for_id: &'a TokenStream,
    pub(in crate::operations) sql_table_name: &'a str,
    pub(in crate::operations) graphql_post_image_check: &'a TokenStream,
    pub(in crate::operations) pk_id_for_entity: &'a TokenStream,
    pub(in crate::operations) notify_on_updated: &'a TokenStream,
    pub(in crate::operations) mutation_output_release: &'a TokenStream,
    pub(in crate::operations) update_many_operation_description: &'a str,
    pub(in crate::operations) update_many_mutation_name: &'a str,
    pub(in crate::operations) update_many_requires_scopes: &'a TokenStream,
    pub(in crate::operations) where_arg_name: &'a str,
    pub(in crate::operations) where_arg_description: &'a str,
    pub(in crate::operations) where_input: &'a syn::Ident,
    pub(in crate::operations) update_many_result_type: &'a syn::Ident,
    pub(in crate::operations) update_many_scope_enforcement: &'a TokenStream,
    pub(in crate::operations) notify_bulk_updated: &'a TokenStream,
    pub(in crate::operations) generates_public_update: bool,
}

pub(in crate::operations) fn update(
    input: UpdateInput<'_>,
) -> (TokenStream, TokenStream, TokenStream) {
    let UpdateInput {
        update_input_str,
        field_case_rule,
        graphql_update_input,
        graphql_update_input_fields,
        update_input,
        update_input_from_graphql_fields,
        update_operation_description,
        update_mutation_name,
        update_requires_scopes,
        authenticated_directive,
        id_arg_name,
        id_arg_description,
        pk_type,
        input_arg_name,
        input_arg_description,
        expected_version_param,
        result_type,
        expected_version_binding,
        resolver_auth_mode,
        rls_request_guard,
        update_scope_enforcement,
        dependent_deletes,
        backend_marker,
        entity_name_lit,
        struct_name,
        update_client_supplied,
        sql_dialect,
        pk_bind_value,
        update_policy_checks,
        update_assignments_graphql_call,
        updated_at_assignment,
        version_assignment,
        table_name,
        pk_id_for_id,
        sql_table_name,
        graphql_post_image_check,
        pk_id_for_entity,
        notify_on_updated,
        mutation_output_release,
        update_many_operation_description,
        update_many_mutation_name,
        update_many_requires_scopes,
        where_arg_name,
        where_arg_description,
        where_input,
        update_many_result_type,
        update_many_scope_enforcement,
        notify_bulk_updated,
        generates_public_update,
    } = input;
    let transaction_start = Boundary::TransactionStart.emit(dependent_deletes);
    let transaction_end = Boundary::TransactionEnd.emit(dependent_deletes);
    let authority_boundary = Boundary::Authority.emit(dependent_deletes);
    let write_boundary = Boundary::BeforeWrite.emit(dependent_deletes);
    let after_transform = Boundary::AfterTransform.emit(dependent_deletes);
    let field_checks_start = Boundary::FieldChecksStart.emit(dependent_deletes);
    let field_checks_end = Boundary::FieldChecksEnd.emit(dependent_deletes);

    // A GraphQL input object must declare at least one field, so the update
    // input and the roots that accept it exist only with a public member.
    let graphql_update_input_definition = if generates_public_update {
        quote! {
            #[derive(::graphql_orm::async_graphql::InputObject, Clone, Debug, Default)]
            #[graphql(name = #update_input_str, rename_fields = #field_case_rule)]
            struct #graphql_update_input {
                #(#graphql_update_input_fields)*
            }

            impl ::std::convert::From<#graphql_update_input> for #update_input {
                fn from(input: #graphql_update_input) -> Self {
                    Self {
                        #(#update_input_from_graphql_fields)*
                    }
                }
            }
        }
    } else {
        quote! {}
    };
    let update_graphql_method = if generates_public_update {
        quote! {
            #[doc = #update_operation_description]
            #[graphql(name = #update_mutation_name, #update_requires_scopes #authenticated_directive)]
            async fn update(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #id_arg_name, desc = #id_arg_description)] id: #pk_type,
                #[graphql(name = #input_arg_name, desc = #input_arg_description)] input: #graphql_update_input,
                #expected_version_param
            ) -> ::graphql_orm::async_graphql::Result<#result_type> {
                // The root's error reports what its write did (opt-in).
                let __gom_outcome = ::graphql_orm::graphql::errors::MutationOutcomeTracker::new();
                let __gom_result: ::graphql_orm::async_graphql::Result<#result_type> = async {
                ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};
                let __gom_expected_version: ::std::option::Option<i64> = #expected_version_binding;

                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #update_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation,
                )?;
                let db = ::graphql_orm::db::request_database::<#backend_marker>(ctx)?;
                let pool = db.pool();
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                db.ensure_entity_access(
                    ::std::option::Option::Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                let mut input: #update_input = input.into();
                #update_client_supplied
                #transaction_start
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin_with_auth(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                    auth_context.as_ref(),
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                #transaction_end
                let current_entity = ::std::boxed::Box::pin(::graphql_orm::graphql::orm::with_write_target_prefilter(
                    db,
                    ::std::option::Option::Some(ctx),
                    EntityQuery::<#struct_name, #backend_marker>::new()
                        .where_clause(
                            &::std::format!("{} = {}", #sql_dialect.render_identifier_path(#struct_name::PRIMARY_KEY), #struct_name::__gom_placeholder(1)),
                            #pk_bind_value
                        ),
                ))
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?
                    .fetch_one_for_write_in_transaction(&mut hook_ctx)
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                if current_entity.is_none() {
                    return ::std::result::Result::Ok(#result_type::__gom_refusal(::graphql_orm::graphql::errors::OrmErrorCode::NotFound, "Entity not found"));
                }
                let current_entity = current_entity.expect("checked above");
                if !db.authorize_write_target(
                    ::std::option::Option::Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                    &current_entity as &(dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync),
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)? {
                    // Neither readable nor writable: indistinguishable from a missing row.
                    return ::std::result::Result::Ok(#result_type::__gom_refusal(::graphql_orm::graphql::errors::OrmErrorCode::NotFound, "Entity not found"));
                }
                if __gom_expected_version.is_some()
                    && #struct_name::__gom_managed_version_of(&current_entity) != __gom_expected_version
                {
                    return ::std::result::Result::Err(::graphql_orm::graphql::errors::OrmPublicError::new(
                        ::graphql_orm::graphql::errors::OrmErrorCode::Conflict,
                    ).into_graphql_error());
                }
                #authority_boundary
                {
                    let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::graphql_in_transaction(
                        ctx,
                        #entity_name_lit,
                        &mut hook_ctx,
                    );
                    db.run_before_update_with_context(
                        &mut write_ctx,
                        ::std::option::Option::Some(&current_entity as &(dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync)),
                        &mut input as &mut (dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                }

                #after_transform

                // Build dynamic UPDATE SQL based on provided fields
                let mut set_clauses: ::std::vec::Vec<::std::string::String> = ::std::vec::Vec::new();
                let mut changed_fields: ::std::vec::Vec<&str> = ::std::vec::Vec::new();
                let mut values: ::std::vec::Vec<::graphql_orm::graphql::orm::SqlValue> = ::std::vec::Vec::new();

                #field_checks_start
                #(#update_policy_checks)*
                #field_checks_end
                #update_assignments_graphql_call

                // Update timestamp column when this entity defines one
                #updated_at_assignment
                #version_assignment

                if set_clauses.is_empty() {
                    return ::std::result::Result::Ok(#result_type::__gom_refusal(::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput, "No fields to update"));
                }
                #struct_name::__gom_require_managed_version_headroom(&current_entity)
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                let before_state = #struct_name::__gom_capture_entity_state(&current_entity)
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                hook_ctx.run_mutation_hook(
                    ::std::option::Option::Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #pk_id_for_id,
                        changes: mutation_changes.clone(),
                        before_state: ::std::option::Option::Some(before_state.clone()),
                        after_state: ::std::option::Option::None,
                    },
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                #write_boundary
                let sql = #struct_name::__gom_rebind_sql(&::std::format!(
                    "UPDATE {} SET {} WHERE {} = ?{}",
                    #sql_table_name,
                    set_clauses.join(", "),
                    #sql_dialect.render_identifier_path(#struct_name::PRIMARY_KEY),
                    ::std::format!(
                        "{}{}",
                        #struct_name::__gom_managed_version_guard(),
                        #struct_name::__gom_managed_version_expectation(__gom_expected_version),
                    ),
                ), 1);

                // Add the ID to the values for the WHERE clause
                values.push(#pk_bind_value);

                __gom_outcome.write_attempted();
                let result = hook_ctx.execute(&sql, &values).await;

                match result {
                    ::std::result::Result::Ok(r) if <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&r) == 1 => {
                        let entity = #struct_name::__gom_fetch_by_id_on(&mut hook_ctx, &id)
                            .await
                            .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                        match entity {
                            ::std::option::Option::Some(entity) => {
                                #graphql_post_image_check
                                let after_state = ::std::option::Option::Some(
                                    #struct_name::__gom_capture_entity_state(&entity)
                                        .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?
                                );

                                hook_ctx.run_mutation_hook(
                                    ::std::option::Option::Some(ctx),
                                    &::graphql_orm::graphql::orm::MutationEvent {
                                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                                        entity_name: #entity_name_lit,
                                        table_name: #table_name,
                                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                                        id: #pk_id_for_entity,
                                        changes: mutation_changes.clone(),
                                        before_state: ::std::option::Option::Some(before_state),
                                        after_state,
                                    },
                                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                                #struct_name::__gom_queue_changed_event(
                                    &mut hook_ctx,
                                    ::graphql_orm::graphql::orm::ChangeAction::Updated,
                                    ::std::option::Option::Some(&entity),
                                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                                hook_ctx.commit_and_emit_tracked(::std::option::Option::Some(&__gom_outcome)).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                                // Invoke optional post-mutation hook.
                                #notify_on_updated

                                let mut result = #result_type::ok(entity);
                                #mutation_output_release
                                ::std::result::Result::Ok(result)
                            },
                            ::std::option::Option::None => ::std::result::Result::Ok(#result_type::__gom_refusal(::graphql_orm::graphql::errors::OrmErrorCode::NotFound, "Entity not found after update")),
                        }
                    }
                    ::std::result::Result::Ok(_) => ::std::result::Result::Err(::graphql_orm::async_graphql::Error::new("authorized update did not affect the exact locked row")),
                    ::std::result::Result::Err(e) => ::std::result::Result::Ok(#result_type::__gom_from_graphql_error(::graphql_orm::graphql::errors::graphql_error_from_sqlx(e))),
                }
            }.await;
                __gom_outcome.finish::<_, #backend_marker>(ctx, __gom_result)
            }
        }
    } else {
        quote! {}
    };
    let update_many_graphql_method = if generates_public_update {
        quote! {
            #[doc = #update_many_operation_description]
            #[graphql(name = #update_many_mutation_name, #update_many_requires_scopes #authenticated_directive)]
            async fn update_many(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[__gom_catalogue_type(Option<#where_input>)]
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: ::std::option::Option<#where_input>,
                #[graphql(name = #input_arg_name, desc = #input_arg_description)] input: #graphql_update_input,
            ) -> ::graphql_orm::async_graphql::Result<#update_many_result_type> {
                // The root's error reports what its write did (opt-in).
                let __gom_outcome = ::graphql_orm::graphql::errors::MutationOutcomeTracker::new();
                let __gom_result: ::graphql_orm::async_graphql::Result<#update_many_result_type> = async {
                ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
                use ::graphql_orm::graphql::orm::{DatabaseFilter, EntityQuery};

                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #update_many_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation,
                )?;
                let db = ::graphql_orm::db::request_database::<#backend_marker>(ctx)?;
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                if let ::std::result::Result::Err(error) = db.ensure_entity_access(
                    ::std::option::Option::Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                ).await {
                    return ::std::result::Result::Ok(#update_many_result_type::from_graphql_error(::graphql_orm::graphql::errors::graphql_error_from_callback(error)));
                }
                if let ::std::result::Result::Err(error) = db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy) {
                    return ::std::result::Result::Ok(#update_many_result_type::from_graphql_error(error));
                }
                let input: #update_input = input.into();

                let filter = match where_input {
                    ::std::option::Option::Some(ref f) if !f.is_empty() => f,
                    _ => return ::std::result::Result::Ok(#update_many_result_type::__gom_refusal(::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput, "Where filter is required for bulk update and must not be empty")),
                };

                if let ::std::result::Result::Err(error) = filter.__gom_require_scalar_write_filter() {
                    return ::std::result::Result::Ok(#update_many_result_type::from_graphql_error(error.into_graphql_error()));
                }
                // A bulk-write filter reveals the fields it names through the
                // affected count: it needs the same read authority as a query.
                if let ::std::result::Result::Err(error) = ::graphql_orm::graphql::orm::authorize_read_fields::<#struct_name, #backend_marker>(
                    db, ctx, ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter),
                ).await {
                    return ::std::result::Result::Ok(#update_many_result_type::from_graphql_error(error));
                }
                let mut notification_rows = ::std::vec::Vec::new();
                match #struct_name::__gom_update_where_with_auth(db, filter.clone(), input, auth_context.as_ref(), ::std::option::Option::Some(ctx), ::std::option::Option::Some(&mut notification_rows), ::std::option::Option::Some(&__gom_outcome)).await {
                    ::std::result::Result::Ok(affected_count) => {
                        #[allow(unused_mut)]
                        let mut result = #update_many_result_type::ok(affected_count);
                        #notify_bulk_updated
                        ::std::result::Result::Ok(result)
                    },
                    ::std::result::Result::Err(e) => {
                        let error = ::graphql_orm::graphql::errors::graphql_error_from_sqlx(e);
                        // `success: false` must mean nothing was written: a failed
                        // commit is reported as a top-level `unknown` error instead.
                        if __gom_outcome.outcome() == ::graphql_orm::graphql::errors::MutationOutcome::Unknown
                            && ::graphql_orm::graphql::errors::MutationOutcomeTracker::enabled::<#backend_marker>(ctx)
                        {
                            return ::std::result::Result::Err(error);
                        }
                        ::std::result::Result::Ok(#update_many_result_type::from_graphql_error(error))
                    },
                }
            }.await;
                __gom_outcome.finish::<_, #backend_marker>(ctx, __gom_result)
            }
        }
    } else {
        quote! {}
    };
    (
        graphql_update_input_definition,
        update_graphql_method,
        update_many_graphql_method,
    )
}
