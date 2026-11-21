use proc_macro2::TokenStream;
use quote::quote;

pub(in crate::operations) struct CreateInput<'a> {
    pub(in crate::operations) create_input_str: &'a str,
    pub(in crate::operations) field_case_rule: &'a str,
    pub(in crate::operations) graphql_create_input: &'a syn::Ident,
    pub(in crate::operations) graphql_create_input_fields: &'a [TokenStream],
    pub(in crate::operations) create_input: &'a syn::Ident,
    pub(in crate::operations) create_input_from_graphql_fields: &'a [TokenStream],
    pub(in crate::operations) create_operation_description: &'a str,
    pub(in crate::operations) create_mutation_name: &'a str,
    pub(in crate::operations) create_requires_scopes: &'a TokenStream,
    pub(in crate::operations) authenticated_directive: &'a TokenStream,
    pub(in crate::operations) input_arg_name: &'a str,
    pub(in crate::operations) input_arg_description: &'a str,
    pub(in crate::operations) result_type: &'a syn::Ident,
    pub(in crate::operations) resolver_auth_mode: &'a TokenStream,
    pub(in crate::operations) rls_request_guard: &'a TokenStream,
    pub(in crate::operations) create_scope_enforcement: &'a TokenStream,
    pub(in crate::operations) backend_marker: &'a TokenStream,
    pub(in crate::operations) entity_name_lit: &'a str,
    pub(in crate::operations) struct_name: &'a syn::Ident,
    pub(in crate::operations) create_client_supplied: &'a TokenStream,
    pub(in crate::operations) created_key_init: &'a TokenStream,
    pub(in crate::operations) insert_sql: &'a str,
    pub(in crate::operations) create_policy_checks: &'a [TokenStream],
    pub(in crate::operations) prepend_pk_bind: &'a TokenStream,
    pub(in crate::operations) insert_binds_graphql: &'a [TokenStream],
    pub(in crate::operations) create_mutation_field_literals: &'a [syn::LitStr],
    pub(in crate::operations) table_name: &'a str,
    pub(in crate::operations) created_key_id: &'a TokenStream,
    pub(in crate::operations) graphql_post_image_check: &'a TokenStream,
    pub(in crate::operations) create_graphql_entity_id: &'a TokenStream,
    pub(in crate::operations) create_graphql_event_entity: &'a TokenStream,
    pub(in crate::operations) create_output_binding: &'a TokenStream,
    pub(in crate::operations) create_output_authorization: &'a TokenStream,
    pub(in crate::operations) result_error_code_none: &'a TokenStream,
    pub(in crate::operations) notify_on_created: &'a TokenStream,
    pub(in crate::operations) generates_public_create: bool,
}

pub(in crate::operations) fn create(input: CreateInput<'_>) -> (TokenStream, TokenStream) {
    let CreateInput {
        create_input_str,
        field_case_rule,
        graphql_create_input,
        graphql_create_input_fields,
        create_input,
        create_input_from_graphql_fields,
        create_operation_description,
        create_mutation_name,
        create_requires_scopes,
        authenticated_directive,
        input_arg_name,
        input_arg_description,
        result_type,
        resolver_auth_mode,
        rls_request_guard,
        create_scope_enforcement,
        backend_marker,
        entity_name_lit,
        struct_name,
        create_client_supplied,
        created_key_init,
        insert_sql,
        create_policy_checks,
        prepend_pk_bind,
        insert_binds_graphql,
        create_mutation_field_literals,
        table_name,
        created_key_id,
        graphql_post_image_check,
        create_graphql_entity_id,
        create_graphql_event_entity,
        create_output_binding,
        create_output_authorization,
        result_error_code_none,
        notify_on_created,
        generates_public_create,
    } = input;
    // Only a create input with a public member is a valid GraphQL input. Upsert
    // shares it and is generated only where create is.
    let graphql_create_input_definition = if generates_public_create {
        quote! {
            #[derive(::graphql_orm::async_graphql::InputObject, Clone, Debug)]
            #[graphql(name = #create_input_str, rename_fields = #field_case_rule)]
            struct #graphql_create_input {
                #(#graphql_create_input_fields)*
            }

            impl ::std::convert::From<#graphql_create_input> for #create_input {
                fn from(input: #graphql_create_input) -> Self {
                    Self {
                        #(#create_input_from_graphql_fields)*
                    }
                }
            }
        }
    } else {
        quote! {}
    };
    let create_graphql_method = quote! {
            #[doc = #create_operation_description]
            #[graphql(name = #create_mutation_name, #create_requires_scopes #authenticated_directive)]
            async fn create(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #input_arg_name, desc = #input_arg_description)] input: #graphql_create_input,
            ) -> ::graphql_orm::async_graphql::Result<#result_type> {
                // The root's error reports what its write did (opt-in).
                let __gom_outcome = ::graphql_orm::graphql::errors::MutationOutcomeTracker::new();
                let __gom_result: ::graphql_orm::async_graphql::Result<#result_type> = async {
                ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #create_scope_enforcement
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
                let mut input: #create_input = input.into();
                #create_client_supplied
                db.run_before_create(
                    ::std::option::Option::Some(ctx),
                    #entity_name_lit,
                    &mut input as &mut (dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync),
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                #created_key_init
                let sql = #struct_name::__gom_rebind_sql(#insert_sql, 1);

                // Collect all values first
                let mut bind_values: ::std::vec::Vec<::graphql_orm::graphql::orm::SqlValue> = ::std::vec::Vec::new();
                #(#create_policy_checks)*
                #prepend_pk_bind
                #(#insert_binds_graphql)*
                let mutation_fields = [#(#create_mutation_field_literals),*];
                let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&mutation_fields, &bind_values);
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin_with_auth(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::Default,
                    auth_context.as_ref(),
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                hook_ctx.run_mutation_hook(
                    ::std::option::Option::Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #created_key_id,
                        changes: mutation_changes.clone(),
                        before_state: ::std::option::Option::None,
                        after_state: ::std::option::Option::None,
                    },
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                __gom_outcome.write_attempted();
                hook_ctx.execute(&sql, &bind_values)
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                let entity = #struct_name::__gom_fetch_created_by_id_on(&mut hook_ctx, &created_pk)
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?
                    .ok_or_else(|| ::graphql_orm::async_graphql::Error::new("Entity not found after creation"))?;
                #graphql_post_image_check
                let after_state = ::std::option::Option::Some(
                    ::graphql_orm::graphql::orm::entity_state(&entity)
                        .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?
                );

                hook_ctx.run_mutation_hook(
                    ::std::option::Option::Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #create_graphql_entity_id,
                        changes: mutation_changes.clone(),
                        before_state: ::std::option::Option::None,
                        after_state,
                    },
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                #struct_name::__gom_queue_changed_event(
                    &mut hook_ctx,
                    ::graphql_orm::graphql::orm::ChangeAction::Created,
                    #create_graphql_event_entity,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                hook_ctx.commit_and_emit_tracked(::std::option::Option::Some(&__gom_outcome)).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                let result = {
                    #create_output_binding
                    #create_output_authorization
                    #result_type { success: true, error: ::std::option::Option::None, #result_error_code_none entity }
                };
                #notify_on_created
                ::std::result::Result::Ok(result)
            }.await;
                __gom_outcome.finish::<_, #backend_marker>(ctx, __gom_result)
            }

    };
    let create_graphql_method = if generates_public_create {
        create_graphql_method
    } else {
        quote! {}
    };
    (graphql_create_input_definition, create_graphql_method)
}
