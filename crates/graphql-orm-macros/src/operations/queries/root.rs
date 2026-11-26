use proc_macro2::TokenStream;
use quote::quote;

use super::QueryContext;

pub(in crate::operations) struct RootInput<'a> {
    pub(in crate::operations) queries_struct: &'a syn::Ident,
    pub(in crate::operations) aggregate_query_method: &'a TokenStream,
    pub(in crate::operations) list_operation_description: &'a str,
    pub(in crate::operations) list_query_skip: &'a TokenStream,
    pub(in crate::operations) list_query_name: &'a str,
    pub(in crate::operations) list_requires_scopes: &'a TokenStream,
    pub(in crate::operations) order_by_input: &'a syn::Ident,
    pub(in crate::operations) list_scope_enforcement: &'a TokenStream,
    pub(in crate::operations) search_query_method: &'a TokenStream,
    pub(in crate::operations) keyset_graphql_method: &'a TokenStream,
    pub(in crate::operations) single_operation_description: &'a str,
    pub(in crate::operations) single_query_skip: &'a TokenStream,
    pub(in crate::operations) single_query_name: &'a str,
    pub(in crate::operations) single_read_requires_scopes: &'a TokenStream,
    pub(in crate::operations) single_query_args: &'a [TokenStream],
    pub(in crate::operations) single_read_scope_enforcement: &'a TokenStream,
    pub(in crate::operations) single_query_key_init: &'a TokenStream,
    pub(in crate::operations) federation_entity_resolvers: &'a TokenStream,
    pub(in crate::operations) node_id_entity_resolver: &'a TokenStream,
    pub(in crate::operations) full_write_surface: bool,
}

pub(in crate::operations) fn root(context: &QueryContext<'_>, input: RootInput<'_>) -> TokenStream {
    let list = list(context, &input);
    let point = point(context, &input);
    let RootInput {
        queries_struct,
        aggregate_query_method,
        search_query_method,
        keyset_graphql_method,
        federation_entity_resolvers,
        node_id_entity_resolver,
        full_write_surface,
        ..
    } = input;
    let query_docs = if full_write_surface {
        quote! { /// Generated queries for this entity
        }
    } else {
        quote! {}
    };
    quote! {
        #query_docs
        #[derive(Default)]
        pub struct #queries_struct;

        #[::graphql_orm::async_graphql::Object]
        impl #queries_struct {
            #aggregate_query_method
            #list
            #search_query_method
            #keyset_graphql_method
            #point
            #federation_entity_resolvers
            #node_id_entity_resolver
        }
    }
}

fn list(context: &QueryContext<'_>, input: &RootInput<'_>) -> TokenStream {
    let RootInput {
        list_operation_description,
        list_query_skip,
        list_query_name,
        list_requires_scopes,
        order_by_input,
        list_scope_enforcement,
        ..
    } = *input;
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
        #[doc = #list_operation_description]
        #[graphql(#list_query_skip name = #list_query_name, #list_requires_scopes #authenticated_directive)]
        async fn list(
            &self,
            ctx: &::graphql_orm::async_graphql::Context<'_>,
            #[__gom_catalogue_type(Option<#where_input>)]
            #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: ::std::option::Option<#where_input>,
            #[__gom_catalogue_type(Option<Vec<#order_by_input>>)]
            #[graphql(name = #order_by_arg_name, desc = #order_by_arg_description)] order_by: ::std::option::Option<::std::vec::Vec<#order_by_input>>,
            #[__gom_catalogue_type(Option<::graphql_orm::graphql::orm::PageInput>)]
            #[graphql(name = #page_arg_name, desc = #page_arg_description)] page: ::std::option::Option<::graphql_orm::graphql::orm::PageInput>,
        ) -> ::graphql_orm::async_graphql::Result<#connection_type> {
            ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
            let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
            #list_scope_enforcement
            ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                ctx,
                ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
            )?;
            let db = ::graphql_orm::db::request_database::<#backend_marker>(ctx)?;
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
            let connection = ::graphql_orm::graphql::orm::__gom_fetch_list::<#struct_name, #backend_marker, _, _>(
                db, ctx, auth_context.as_ref(), where_input.as_ref(), order_by.as_deref().unwrap_or_default(), page.as_ref(),
            ).await?;
            ::std::result::Result::Ok(#connection_type::from_generic(connection))
        }
    }
}

fn point(context: &QueryContext<'_>, input: &RootInput<'_>) -> TokenStream {
    let RootInput {
        single_operation_description,
        single_query_skip,
        single_query_name,
        single_read_requires_scopes,
        single_query_args,
        single_read_scope_enforcement,
        single_query_key_init,
        ..
    } = *input;
    let QueryContext {
        struct_name,
        backend_marker,
        resolver_auth_mode,
        authenticated_directive,
        rls_request_guard,
        entity_name_lit,
        ..
    } = *context;
    quote! {
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
            let db = ::graphql_orm::db::request_database::<#backend_marker>(ctx)?;
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
    }
}
