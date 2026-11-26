use proc_macro2::TokenStream;
use quote::quote;

use super::QueryContext;

pub(in crate::operations) struct SearchInput<'a> {
    pub(in crate::operations) search_edge_type_str: &'a str,
    pub(in crate::operations) search_edge_type: &'a syn::Ident,
    pub(in crate::operations) search_connection_type_str: &'a str,
    pub(in crate::operations) search_connection_type: &'a syn::Ident,
    pub(in crate::operations) search_operation_description: &'a str,
    pub(in crate::operations) search_query_skip: &'a TokenStream,
    pub(in crate::operations) search_query_name: &'a str,
    pub(in crate::operations) search_requires_scopes: &'a TokenStream,
    pub(in crate::operations) search_arg_name: &'a str,
    pub(in crate::operations) search_arg_description: &'a str,
    pub(in crate::operations) search_scope_enforcement: &'a TokenStream,
    pub(in crate::operations) pool_type: &'a TokenStream,
    pub(in crate::operations) has_search: bool,
}

pub(in crate::operations) fn search(
    context: &QueryContext<'_>,
    input: SearchInput<'_>,
) -> (TokenStream, TokenStream, TokenStream) {
    let SearchInput {
        search_edge_type_str,
        search_edge_type,
        search_connection_type_str,
        search_connection_type,
        search_operation_description,
        search_query_skip,
        search_query_name,
        search_requires_scopes,
        search_arg_name,
        search_arg_description,
        search_scope_enforcement,
        pool_type,
        has_search,
    } = input;
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
        field_case_rule,
        page_info_field_name,
        ref filter_read_authorization,
        ..
    } = *context;
    let search_connection_definitions = if has_search {
        quote! {
            #[derive(::graphql_orm::async_graphql::SimpleObject, Debug, Clone)]
            #[graphql(name = #search_edge_type_str, rename_fields = #field_case_rule)]
            pub struct #search_edge_type {
                pub node: #struct_name,
                pub cursor: ::std::string::String,
                pub score: f64,
            }

            #[derive(::graphql_orm::async_graphql::SimpleObject, Debug, Clone)]
            #[graphql(name = #search_connection_type_str, rename_fields = #field_case_rule)]
            pub struct #search_connection_type {
                pub edges: ::std::vec::Vec<#search_edge_type>,
                #[graphql(name = #page_info_field_name)]
                pub page_info: ::graphql_orm::graphql::pagination::PageInfo,
            }

            impl #search_connection_type {
                pub fn from_generic(conn: ::graphql_orm::graphql::orm::SearchConnection<#struct_name>) -> Self {
                    Self {
                        edges: conn.edges.into_iter().map(|edge| #search_edge_type {
                            node: edge.node,
                            cursor: edge.cursor,
                            score: edge.score,
                        }).collect(),
                        page_info: conn.page_info,
                    }
                }
            }
        }
    } else {
        quote! {}
    };

    let search_query_method = if has_search {
        quote! {
            #[doc = #search_operation_description]
            #[graphql(#search_query_skip name = #search_query_name, #search_requires_scopes #authenticated_directive)]
            async fn search(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #search_arg_name, desc = #search_arg_description)] search: ::graphql_orm::graphql::filters::SearchInput,
                #[__gom_catalogue_type(Option<#where_input>)]
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: ::std::option::Option<#where_input>,
                #[__gom_catalogue_type(Option<::graphql_orm::graphql::orm::PageInput>)]
                #[graphql(name = #page_arg_name, desc = #page_arg_description)] page: ::std::option::Option<::graphql_orm::graphql::orm::PageInput>,
            ) -> ::graphql_orm::async_graphql::Result<#search_connection_type> {
                ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #search_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                )?;
                let db = ::graphql_orm::db::request_database::<#backend_marker>(ctx)?;
                let pool = db.pool();
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

                let mut query = #struct_name::search(pool, search)
                    .client_pagination_config(db.client_pagination_config());
                if let ::std::option::Option::Some(ref filter) = where_input {
                    query = query.filter(filter.clone());
                }
                query = query.default_order();

                let generic_conn = query
                    .fetch_authorized_connection(db, ctx, auth_context.as_ref(), page)
                    .await?;

                ::std::result::Result::Ok(#search_connection_type::from_generic(generic_conn))
            }
        }
    } else {
        quote! {}
    };

    let search_repository_method = if has_search {
        quote! {
            /// Build a trusted, unchecked search over a raw pool.
            ///
            /// This builder has no request and no policy provider: it applies
            /// no entity, row, field or search-source authorization. Use
            /// `search_db` for policy-aware searches.
            pub fn search<'a>(
                pool: &'a #pool_type,
                search: ::graphql_orm::graphql::filters::SearchInput,
            ) -> ::graphql_orm::graphql::orm::EntitySearchQuery<'a, Self, #where_input, #backend_marker> {
                ::graphql_orm::graphql::orm::EntitySearchQuery::new(pool, search)
            }

            /// Build a policy-aware search from a `Database` handle.
            ///
            /// The search authorizes entity access and every search source
            /// before any SQL, and row and field policies on every hit, like
            /// the other repository reads. `search(pool, …)` is the trusted,
            /// unchecked builder.
            pub fn search_db<'a>(
                db: &'a ::graphql_orm::db::Database<#backend_marker>,
                search: ::graphql_orm::graphql::filters::SearchInput,
            ) -> ::graphql_orm::graphql::orm::RepositorySearchQuery<'a, Self, #where_input, #backend_marker> {
                ::graphql_orm::graphql::orm::RepositorySearchQuery::new(db, search)
            }
        }
    } else {
        quote! {}
    };
    (
        search_connection_definitions,
        search_query_method,
        search_repository_method,
    )
}
