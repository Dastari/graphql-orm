use proc_macro2::TokenStream;
use quote::quote;

pub(in crate::operations) struct ResolverInput<'a> {
    pub(in crate::operations) upsert_operation_description: &'a str,
    pub(in crate::operations) upsert_mutation_name: &'a str,
    pub(in crate::operations) upsert_requires_scopes: &'a TokenStream,
    pub(in crate::operations) authenticated_directive: &'a TokenStream,
    pub(in crate::operations) input_arg_name: &'a str,
    pub(in crate::operations) input_arg_description: &'a str,
    pub(in crate::operations) graphql_create_input: &'a syn::Ident,
    pub(in crate::operations) upsert_result_type: &'a syn::Ident,
    pub(in crate::operations) resolver_auth_mode: &'a TokenStream,
    pub(in crate::operations) rls_request_guard: &'a TokenStream,
    pub(in crate::operations) upsert_scope_enforcement: &'a TokenStream,
    pub(in crate::operations) backend_marker: &'a TokenStream,
    pub(in crate::operations) entity_name_lit: &'a str,
    pub(in crate::operations) struct_name: &'a syn::Ident,
    pub(in crate::operations) create_input: &'a syn::Ident,
    pub(in crate::operations) create_client_supplied: &'a TokenStream,
    pub(in crate::operations) upsert_policy_checks_graphql: &'a [TokenStream],
    pub(in crate::operations) created_pk_init: &'a TokenStream,
    pub(in crate::operations) pk_id_for_entity: &'a TokenStream,
    pub(in crate::operations) created_pk_id_string: &'a TokenStream,
    pub(in crate::operations) upsert_graphql_sql_expression: &'a TokenStream,
    pub(in crate::operations) prepend_pk_bind: &'a TokenStream,
    pub(in crate::operations) insert_binds_graphql: &'a [TokenStream],
    pub(in crate::operations) upsert_change_binds_graphql: &'a [TokenStream],
    pub(in crate::operations) upsert_change_field_literals: &'a [syn::LitStr],
    pub(in crate::operations) create_mutation_field_literals: &'a [syn::LitStr],
    pub(in crate::operations) table_name: &'a str,
    pub(in crate::operations) graphql_post_image_check: &'a TokenStream,
    pub(in crate::operations) notify_on_created: &'a TokenStream,
    pub(in crate::operations) notify_on_updated: &'a TokenStream,
    pub(in crate::operations) mutation_output_release: &'a TokenStream,
    pub(in crate::operations) generates_public_upsert: bool,
    /// Resolves each principal-bound key from the request context (#908).
    pub(in crate::operations) principal_key_resolution: &'a TokenStream,
    /// Writes the resolved keys into `input`.
    pub(in crate::operations) principal_key_assignment: &'a TokenStream,
    /// Refuses an existing row that does not hold the bound key exactly.
    pub(in crate::operations) principal_key_preimage_check: &'a TokenStream,
    /// The error when the written row cannot be read back.
    pub(in crate::operations) upsert_missing_post_image: &'a TokenStream,
    /// Refuses a defaulted variable in the transient argument.
    pub(in crate::operations) transient_default_check: &'a TokenStream,
    /// Remembers the ID an insert would create, for the post-image check.
    pub(in crate::operations) principal_key_created_id: &'a TokenStream,
    /// Refuses a written row that does not hold the bound key exactly.
    pub(in crate::operations) principal_key_postimage_check: &'a TokenStream,
    /// The typed transient argument, with its trailing comma.
    pub(in crate::operations) transient_argument: &'a TokenStream,
    /// Stores the transient argument on the mutation transaction.
    pub(in crate::operations) transient_store: &'a TokenStream,
    /// Whether the `Before` hook may acknowledge without writing.
    pub(in crate::operations) acknowledge: bool,
}

pub(in crate::operations) fn resolver(input: ResolverInput<'_>) -> TokenStream {
    let ResolverInput {
        upsert_operation_description,
        upsert_mutation_name,
        upsert_requires_scopes,
        authenticated_directive,
        input_arg_name,
        input_arg_description,
        graphql_create_input,
        upsert_result_type,
        resolver_auth_mode,
        rls_request_guard,
        upsert_scope_enforcement,
        backend_marker,
        entity_name_lit,
        struct_name,
        create_input,
        create_client_supplied,
        upsert_policy_checks_graphql,
        created_pk_init,
        pk_id_for_entity,
        created_pk_id_string,
        upsert_graphql_sql_expression,
        prepend_pk_bind,
        insert_binds_graphql,
        upsert_change_binds_graphql,
        upsert_change_field_literals,
        create_mutation_field_literals,
        table_name,
        graphql_post_image_check,
        notify_on_created,
        notify_on_updated,
        mutation_output_release,
        generates_public_upsert,
        principal_key_resolution,
        principal_key_assignment,
        principal_key_preimage_check,
        principal_key_created_id,
        upsert_missing_post_image,
        transient_default_check,
        principal_key_postimage_check,
        transient_argument,
        transient_store,
        acknowledge,
    } = input;
    // An acknowledging `Before` hook answers with the locked existing row:
    // no statement, no `After` hook, no `Changed` event and no notification.
    let (acknowledge_arm, acknowledge_finish) = if acknowledge {
        (
            quote! {
                let __gom_acknowledgement = current_entity
                    .is_some()
                    .then(|| hook_ctx.__arm_write_acknowledgement());
            },
            quote! {
                let __gom_acknowledged = match __gom_acknowledgement {
                    ::std::option::Option::Some(scope) => hook_ctx.__finish_write_acknowledgement(scope),
                    ::std::option::Option::None => false,
                };
                __gom_before_hook?;
                if let (true, ::std::option::Option::Some(existing)) = (__gom_acknowledged, current_entity.as_ref()) {
                    let existing = ::std::clone::Clone::clone(existing);
                    hook_ctx
                        .commit_and_emit_tracked(::std::option::Option::Some(&__gom_outcome))
                        .await
                        .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                    let mut result = #upsert_result_type::unchanged(existing);
                    #mutation_output_release
                    return ::std::result::Result::Ok(result);
                }
            },
        )
    } else {
        (quote! {}, quote! { __gom_before_hook?; })
    };
    if generates_public_upsert {
        quote! {
            #[doc = #upsert_operation_description]
            #[graphql(name = #upsert_mutation_name, #upsert_requires_scopes #authenticated_directive)]
            async fn upsert(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #input_arg_name, desc = #input_arg_description)] input: #graphql_create_input,
                #transient_argument
            ) -> ::graphql_orm::async_graphql::Result<#upsert_result_type> {
                // The root's error reports what its write did (opt-in).
                let __gom_outcome = ::graphql_orm::graphql::errors::MutationOutcomeTracker::new();
                let __gom_result: ::graphql_orm::async_graphql::Result<#upsert_result_type> = async {
                ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
                #transient_default_check
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #upsert_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation,
                )?;
                // A principal-bound key is resolved from the verified request
                // before the entity policy, the transaction and any read or
                // write; a failed binding refuses the mutation.
                #principal_key_resolution
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
                let mut input: #create_input = input.into();
                #principal_key_assignment
                #create_client_supplied
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin_with_auth(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                    auth_context.as_ref(),
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                #transient_store
                {
                    let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::graphql_in_transaction(
                        ctx,
                        #entity_name_lit,
                        &mut hook_ctx,
                    );
                    db.run_before_upsert_with_context(
                        &mut write_ctx,
                        &mut input as &mut (dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                }
                // The input transform cannot replace a principal-bound key.
                #principal_key_assignment

                let current_entity = #struct_name::__gom_fetch_by_upsert_on(&mut hook_ctx, &input)
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                // Before any policy callback, hook or statement.
                #principal_key_preimage_check
                if let ::std::option::Option::Some(current_entity) = current_entity.as_ref() {
                    db.ensure_writable_row(
                        ::std::option::Option::Some(ctx),
                        #entity_name_lit,
                        <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                        current_entity as &(dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                }
                // Update candidates whose write the policy denied and the host
                // chose to ignore: they stay out of the conflict update.
                #[allow(unused_mut)]
                let mut __gom_denied_upsert_columns: ::std::vec::Vec<&'static str> = ::std::vec::Vec::new();
                #(#upsert_policy_checks_graphql)*

                #created_pk_init
                let action = if current_entity.is_some() {
                    ::graphql_orm::graphql::orm::ChangeAction::Updated
                } else {
                    ::graphql_orm::graphql::orm::ChangeAction::Created
                };
                let before_state = current_entity
                    .as_ref()
                    .map(#struct_name::__gom_capture_entity_state)
                    .transpose()
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                let event_id = current_entity
                    .as_ref()
                    .map(|entity| #pk_id_for_entity)
                    .unwrap_or_else(|| #created_pk_id_string);
                #principal_key_created_id
                let lookup_input = input.clone();
                let change_input = input.clone();
                let sql = #struct_name::__gom_rebind_sql(&#upsert_graphql_sql_expression, 1);
                let mut bind_values: ::std::vec::Vec<::graphql_orm::graphql::orm::SqlValue> = ::std::vec::Vec::new();
                #prepend_pk_bind
                #(#insert_binds_graphql)*
                let mutation_changes = if action == ::graphql_orm::graphql::orm::ChangeAction::Updated {
                    let input = change_input;
                    let mut bind_values: ::std::vec::Vec<::graphql_orm::graphql::orm::SqlValue> = ::std::vec::Vec::new();
                    #(#upsert_change_binds_graphql)*
                    let mut changes = ::graphql_orm::graphql::orm::mutation_changes(&[#(#upsert_change_field_literals),*], &bind_values);
                    changes.retain(|change| !__gom_denied_upsert_columns.contains(&change.field.as_str()));
                    changes
                } else {
                    ::graphql_orm::graphql::orm::mutation_changes(&[#(#create_mutation_field_literals),*], &bind_values)
                };

                #acknowledge_arm
                let __gom_before_hook = hook_ctx.run_mutation_hook(
                    ::std::option::Option::Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: event_id,
                        changes: mutation_changes.clone(),
                        before_state: before_state.clone(),
                        after_state: ::std::option::Option::None,
                    },
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback);
                #acknowledge_finish

                if let ::std::option::Option::Some(current_entity) = current_entity.as_ref() {
                    #struct_name::__gom_require_managed_version_headroom(current_entity)
                        .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                }
                __gom_outcome.write_attempted();
                hook_ctx.execute(&sql, &bind_values)
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                let entity = #struct_name::__gom_fetch_by_upsert_on(&mut hook_ctx, &lookup_input)
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?
                    .ok_or_else(|| #upsert_missing_post_image)?;
                #principal_key_postimage_check
                #graphql_post_image_check
                #struct_name::__gom_require_managed_version_advanced(current_entity.as_ref(), &entity)
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                let after_state = ::std::option::Option::Some(
                    #struct_name::__gom_capture_entity_state(&entity)
                        .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?
                );

                hook_ctx.run_mutation_hook(
                    ::std::option::Option::Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #pk_id_for_entity,
                        changes: mutation_changes.clone(),
                        before_state,
                        after_state,
                    },
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                #struct_name::__gom_queue_changed_event(&mut hook_ctx, action, ::std::option::Option::Some(&entity))
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                hook_ctx
                    .commit_and_emit_tracked(::std::option::Option::Some(&__gom_outcome))
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                match action {
                    ::graphql_orm::graphql::orm::ChangeAction::Created => {
                        #notify_on_created
                    }
                    ::graphql_orm::graphql::orm::ChangeAction::Updated => {
                        #notify_on_updated
                    }
                    ::graphql_orm::graphql::orm::ChangeAction::Deleted => {}
                }

                let mut result = #upsert_result_type::ok(action, entity);
                #mutation_output_release
                ::std::result::Result::Ok(result)
            }.await;
                __gom_outcome.finish::<_, #backend_marker>(ctx, __gom_result)
            }
        }
    } else {
        quote! {}
    }
}
