use crate::dependent_deletes::Boundary;

use proc_macro2::TokenStream;
use quote::quote;

pub(in crate::operations) struct SingleInput<'a> {
    pub(in crate::operations) delete_operation_description: &'a str,
    pub(in crate::operations) delete_mutation_name: &'a str,
    pub(in crate::operations) delete_requires_scopes: &'a TokenStream,
    pub(in crate::operations) authenticated_directive: &'a TokenStream,
    pub(in crate::operations) id_arg_name: &'a str,
    pub(in crate::operations) id_arg_description: &'a str,
    pub(in crate::operations) pk_type: &'a TokenStream,
    pub(in crate::operations) expected_version_param: &'a TokenStream,
    pub(in crate::operations) result_type: &'a syn::Ident,
    pub(in crate::operations) expected_version_binding: &'a TokenStream,
    pub(in crate::operations) reject_unplanned_delete_effects: &'a TokenStream,
    pub(in crate::operations) resolver_auth_mode: &'a TokenStream,
    pub(in crate::operations) rls_request_guard: &'a TokenStream,
    pub(in crate::operations) delete_scope_enforcement: &'a TokenStream,
    pub(in crate::operations) dependent_deletes: bool,
    pub(in crate::operations) backend_marker: &'a TokenStream,
    pub(in crate::operations) entity_name_lit: &'a str,
    pub(in crate::operations) struct_name: &'a syn::Ident,
    pub(in crate::operations) sql_dialect: &'a TokenStream,
    pub(in crate::operations) pk_bind_value: &'a TokenStream,
    pub(in crate::operations) table_name: &'a str,
    pub(in crate::operations) pk_id_for_entity: &'a TokenStream,
    pub(in crate::operations) sql_table_name: &'a str,
    pub(in crate::operations) notify_on_deleted: &'a TokenStream,
}

pub(in crate::operations) fn single(input: SingleInput<'_>) -> TokenStream {
    let SingleInput {
        delete_operation_description,
        delete_mutation_name,
        delete_requires_scopes,
        authenticated_directive,
        id_arg_name,
        id_arg_description,
        pk_type,
        expected_version_param,
        result_type,
        expected_version_binding,
        reject_unplanned_delete_effects,
        resolver_auth_mode,
        rls_request_guard,
        delete_scope_enforcement,
        dependent_deletes,
        backend_marker,
        entity_name_lit,
        struct_name,
        sql_dialect,
        pk_bind_value,
        table_name,
        pk_id_for_entity,
        sql_table_name,
        notify_on_deleted,
    } = input;
    let transaction_start = Boundary::TransactionStart.emit(dependent_deletes);
    let transaction_end = Boundary::TransactionEnd.emit(dependent_deletes);
    let authority_boundary = Boundary::Authority.emit(dependent_deletes);
    let write_boundary = Boundary::BeforeWrite.emit(dependent_deletes);

    quote! {
            #[doc = #delete_operation_description]
            #[graphql(name = #delete_mutation_name, #delete_requires_scopes #authenticated_directive)]
            async fn delete(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #id_arg_name, desc = #id_arg_description)] id: #pk_type,
                #expected_version_param
            ) -> ::graphql_orm::async_graphql::Result<#result_type> {
                // The root's error reports what its write did (opt-in).
                let __gom_outcome = ::graphql_orm::graphql::errors::MutationOutcomeTracker::new();
                let __gom_result: ::graphql_orm::async_graphql::Result<#result_type> = async {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, SqlValue};
                let __gom_expected_version: ::std::option::Option<i64> = #expected_version_binding;

                #reject_unplanned_delete_effects
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #delete_scope_enforcement
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

                #transaction_start
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin_with_auth(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                    auth_context.as_ref(),
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                #transaction_end

                // Fetch entity before deletion for notification purposes
                let entity = ::std::boxed::Box::pin(::graphql_orm::graphql::orm::with_write_target_prefilter(
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

                if entity.is_none() {
                    return ::std::result::Result::Ok(#result_type::__gom_refusal(::graphql_orm::graphql::errors::OrmErrorCode::NotFound, "Entity not found"));
                }
                let entity = entity.unwrap();
                if !db.authorize_write_target(
                    ::std::option::Option::Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                    &entity as &(dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync),
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)? {
                    // Neither readable nor writable: indistinguishable from a missing row.
                    return ::std::result::Result::Ok(#result_type::__gom_refusal(::graphql_orm::graphql::errors::OrmErrorCode::NotFound, "Entity not found"));
                }
                if __gom_expected_version.is_some()
                    && #struct_name::__gom_managed_version_of(&entity) != __gom_expected_version
                {
                    return ::std::result::Result::Err(::graphql_orm::graphql::errors::OrmPublicError::new(
                        ::graphql_orm::graphql::errors::OrmErrorCode::Conflict,
                    ).into_graphql_error());
                }
                #authority_boundary
                let before_state = #struct_name::__gom_capture_entity_state(&entity)
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                hook_ctx.run_mutation_hook(
                    ::std::option::Option::Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #pk_id_for_entity,
                        changes: ::std::vec::Vec::new(),
                        before_state: ::std::option::Option::Some(before_state.clone()),
                        after_state: ::std::option::Option::None,
                    },
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                #write_boundary
                let sql = #struct_name::__gom_rebind_sql(
                    &::std::format!(
                        "DELETE FROM {} WHERE {} = ?{}",
                        #sql_table_name,
                        #sql_dialect.render_identifier_path(#struct_name::PRIMARY_KEY),
                        #struct_name::__gom_managed_version_expectation(__gom_expected_version),
                    ),
                    1
                );
                let values = [#pk_bind_value];
                __gom_outcome.write_attempted();
                let result = hook_ctx.execute(&sql, &values).await;

                match result {
                    ::std::result::Result::Ok(r) if <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&r) == 1 => {
                        hook_ctx.run_mutation_hook(
                            ::std::option::Option::Some(ctx),
                            &::graphql_orm::graphql::orm::MutationEvent {
                                phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                                entity_name: #entity_name_lit,
                                table_name: #table_name,
                                metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                                id: #pk_id_for_entity,
                                changes: ::std::vec::Vec::new(),
                                before_state: ::std::option::Option::Some(before_state),
                                after_state: ::std::option::Option::None,
                            },
                        ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                        #struct_name::__gom_queue_changed_event(
                            &mut hook_ctx,
                            ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                            ::std::option::Option::Some(&entity),
                        ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                        hook_ctx.commit_and_emit_tracked(::std::option::Option::Some(&__gom_outcome)).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                        // Invoke optional post-mutation hook.
                        #notify_on_deleted

                        ::std::result::Result::Ok(#result_type::__gom_deleted())
                    },
                    ::std::result::Result::Ok(_) => ::std::result::Result::Err(::graphql_orm::async_graphql::Error::new("authorized delete did not affect the exact locked row")),
                    ::std::result::Result::Err(e) => ::std::result::Result::Ok(#result_type::__gom_from_graphql_error(::graphql_orm::graphql::errors::graphql_error_from_sqlx(e))),
                }
            }.await;
                __gom_outcome.finish::<_, #backend_marker>(ctx, __gom_result)
            }
    }
}

pub(in crate::operations) struct BulkInput<'a> {
    pub(in crate::operations) delete_many_operation_description: &'a str,
    pub(in crate::operations) delete_many_mutation_name: &'a str,
    pub(in crate::operations) delete_many_requires_scopes: &'a TokenStream,
    pub(in crate::operations) authenticated_directive: &'a TokenStream,
    pub(in crate::operations) where_arg_name: &'a str,
    pub(in crate::operations) where_arg_description: &'a str,
    pub(in crate::operations) where_input: &'a syn::Ident,
    pub(in crate::operations) delete_many_result_type: &'a syn::Ident,
    pub(in crate::operations) reject_unplanned_delete_effects: &'a TokenStream,
    pub(in crate::operations) resolver_auth_mode: &'a TokenStream,
    pub(in crate::operations) rls_request_guard: &'a TokenStream,
    pub(in crate::operations) delete_many_scope_enforcement: &'a TokenStream,
    pub(in crate::operations) backend_marker: &'a TokenStream,
    pub(in crate::operations) entity_name_lit: &'a str,
    pub(in crate::operations) struct_name: &'a syn::Ident,
    pub(in crate::operations) notify_bulk_deleted: &'a TokenStream,
}

pub(in crate::operations) fn bulk(input: BulkInput<'_>) -> TokenStream {
    let BulkInput {
        delete_many_operation_description,
        delete_many_mutation_name,
        delete_many_requires_scopes,
        authenticated_directive,
        where_arg_name,
        where_arg_description,
        where_input,
        delete_many_result_type,
        reject_unplanned_delete_effects,
        resolver_auth_mode,
        rls_request_guard,
        delete_many_scope_enforcement,
        backend_marker,
        entity_name_lit,
        struct_name,
        notify_bulk_deleted,
    } = input;
    quote! {
            #[doc = #delete_many_operation_description]
            #[graphql(name = #delete_many_mutation_name, #delete_many_requires_scopes #authenticated_directive)]
            async fn delete_many(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[__gom_catalogue_type(Option<#where_input>)]
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: ::std::option::Option<#where_input>,
            ) -> ::graphql_orm::async_graphql::Result<#delete_many_result_type> {
                // The root's error reports what its write did (opt-in).
                let __gom_outcome = ::graphql_orm::graphql::errors::MutationOutcomeTracker::new();
                let __gom_result: ::graphql_orm::async_graphql::Result<#delete_many_result_type> = async {
                ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, EntityQuery};

                #reject_unplanned_delete_effects
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #delete_many_scope_enforcement
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
                    return ::std::result::Result::Ok(#delete_many_result_type::from_graphql_error(::graphql_orm::graphql::errors::graphql_error_from_callback(error)));
                }
                if let ::std::result::Result::Err(error) = db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy) {
                    return ::std::result::Result::Ok(#delete_many_result_type::from_graphql_error(error));
                }

                let filter = match where_input {
                    ::std::option::Option::Some(ref f) if !f.is_empty() => f,
                    _ => return ::std::result::Result::Ok(#delete_many_result_type::__gom_refusal(::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput, "Where filter is required for bulk delete and must not be empty")),
                };

                if let ::std::result::Result::Err(error) = filter.__gom_require_scalar_write_filter() {
                    return ::std::result::Result::Ok(#delete_many_result_type::from_graphql_error(error.into_graphql_error()));
                }
                // A bulk-write filter reveals the fields it names through the
                // affected count: it needs the same read authority as a query.
                if let ::std::result::Result::Err(error) = ::graphql_orm::graphql::orm::authorize_read_fields::<#struct_name, #backend_marker>(
                    db, ctx, ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter),
                ).await {
                    return ::std::result::Result::Ok(#delete_many_result_type::from_graphql_error(error));
                }
                let mut notification_rows = ::std::vec::Vec::new();
                match #struct_name::__gom_delete_where_with_auth(db, filter.clone(), auth_context.as_ref(), ::std::option::Option::Some(ctx), ::std::option::Option::Some(&mut notification_rows), ::std::option::Option::Some(&__gom_outcome)).await {
                    ::std::result::Result::Ok(deleted_count) => {
                        #[allow(unused_mut)]
                        let mut result = #delete_many_result_type::ok(deleted_count);
                        #notify_bulk_deleted
                        ::std::result::Result::Ok(result)
                    },
                    ::std::result::Result::Err(e) => {
                        let error = ::graphql_orm::graphql::errors::graphql_error_from_sqlx(e);
                        // As for updateMany: a failed commit is a top-level `unknown` error.
                        if __gom_outcome.outcome() == ::graphql_orm::graphql::errors::MutationOutcome::Unknown
                            && ::graphql_orm::graphql::errors::MutationOutcomeTracker::enabled::<#backend_marker>(ctx)
                        {
                            return ::std::result::Result::Err(error);
                        }
                        ::std::result::Result::Ok(#delete_many_result_type::from_graphql_error(error))
                    },
                }
            }.await;
                __gom_outcome.finish::<_, #backend_marker>(ctx, __gom_result)
            }
    }
}
