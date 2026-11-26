use super::{Assurance, Hook, admit_request, admitted_request, write_entity};
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
    pub(in crate::operations) delete_scope_arguments: &'a TokenStream,
    pub(in crate::operations) dependent_deletes: bool,
    pub(in crate::operations) backend_marker: &'a TokenStream,
    pub(in crate::operations) struct_name: &'a syn::Ident,
    pub(in crate::operations) sql_dialect: &'a TokenStream,
    pub(in crate::operations) pk_bind_value: &'a TokenStream,
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
        delete_scope_arguments,
        dependent_deletes,
        backend_marker,
        struct_name,
        sql_dialect,
        pk_bind_value,
        pk_id_for_entity,
        sql_table_name,
        notify_on_deleted,
    } = input;
    let entity = write_entity(struct_name);
    let request = admitted_request();
    let admit = admit_request(
        struct_name,
        backend_marker,
        "Delete",
        delete_scope_arguments,
        Assurance::Resolver,
    );
    let hook = |phase, before_state| {
        Hook {
            request: &request,
            phase,
            action: quote! { ::graphql_orm::graphql::orm::ChangeAction::Deleted },
            id: pk_id_for_entity.clone(),
            changes: quote! { ::std::vec::Vec::new() },
            before_state,
            after_state: quote! { ::std::option::Option::None },
        }
        .dispatch()
    };
    let before_hook = hook(
        "Before",
        quote! { ::std::option::Option::Some(before_state.clone()) },
    );
    let after_hook = hook(
        "After",
        quote! { ::std::option::Option::Some(before_state) },
    );
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
                #entity
                #admit
                let __gom_write = __gom_request.__admit_entity::<#backend_marker>(ctx).await?;
                let __gom_authority = __gom_write.authority();
                let db = __gom_write.db();
                let pool = db.pool();

                #transaction_start
                let mut hook_ctx = __gom_write.begin(::graphql_orm::graphql::orm::TransactionMode::StateMachine).await?;
                #transaction_end

                // The locked row is also the notification's entity. Missing,
                // filtered out, or neither readable nor writable:
                // indistinguishable to the client.
                let entity = __gom_write.__lock_target(
                    ctx,
                    &mut hook_ctx,
                    EntityQuery::<#struct_name, #backend_marker>::new()
                        .where_clause(
                            &::std::format!("{} = {}", #sql_dialect.render_identifier_path(#struct_name::PRIMARY_KEY), #struct_name::__gom_placeholder(1)),
                            #pk_bind_value
                        ),
                ).await?;
                let ::std::option::Option::Some(entity) = entity else {
                    return ::std::result::Result::Ok(#result_type::__gom_refusal(::graphql_orm::graphql::errors::OrmErrorCode::NotFound, "Entity not found"));
                };
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
                #before_hook.await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

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
                        #after_hook.await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
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
    pub(in crate::operations) delete_many_scope_arguments: &'a TokenStream,
    pub(in crate::operations) backend_marker: &'a TokenStream,
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
        delete_many_scope_arguments,
        backend_marker,
        struct_name,
        notify_bulk_deleted,
    } = input;
    let entity = write_entity(struct_name);
    let request = admitted_request();
    let admit = admit_request(
        struct_name,
        backend_marker,
        "DeleteMany",
        delete_many_scope_arguments,
        Assurance::Resolver,
    );
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
                #entity
                #admit
                let __gom_write = match __gom_request.__admit_entity_bulk::<#backend_marker>(ctx).await? {
                    ::std::result::Result::Ok(admission) => admission,
                    ::std::result::Result::Err(error) => {
                        return ::std::result::Result::Ok(#delete_many_result_type::from_graphql_error(error));
                    }
                };
                let __gom_authority = __gom_write.authority();
                let db = __gom_write.db();

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
                match #struct_name::__gom_delete_where_with_auth(db, filter.clone(), __gom_write.auth_context(), #request, ::std::option::Option::Some(&mut notification_rows), ::std::option::Option::Some(&__gom_outcome)).await {
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
