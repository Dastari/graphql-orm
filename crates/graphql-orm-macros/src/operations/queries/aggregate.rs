use proc_macro2::TokenStream;
use quote::quote;

use super::QueryContext;
use crate::entity::EntityMetadata;

pub(in crate::operations) struct AggregateInput<'a> {
    pub(in crate::operations) source_entity_field_name: &'a str,
    pub(in crate::operations) aggregate_metric_input: &'a syn::Ident,
    pub(in crate::operations) aggregate_field: &'a syn::Ident,
    pub(in crate::operations) aggregate_operation_description: &'a str,
    pub(in crate::operations) aggregate_query_skip: &'a TokenStream,
    pub(in crate::operations) aggregate_query_name: &'a str,
    pub(in crate::operations) aggregate_requires_scopes: &'a TokenStream,
    pub(in crate::operations) group_by_arg_name: &'a str,
    pub(in crate::operations) group_by_arg_description: &'a str,
    pub(in crate::operations) metrics_arg_name: &'a str,
    pub(in crate::operations) metrics_arg_description: &'a str,
    pub(in crate::operations) group_limit_arg_name: &'a str,
    pub(in crate::operations) group_limit_arg_description: &'a str,
    pub(in crate::operations) aggregate_scope_enforcement: &'a TokenStream,
    pub(in crate::operations) entity_meta: &'a EntityMetadata,
}

pub(in crate::operations) fn aggregate(
    context: &QueryContext<'_>,
    input: AggregateInput<'_>,
) -> (TokenStream, TokenStream, TokenStream) {
    let AggregateInput {
        source_entity_field_name,
        aggregate_metric_input,
        aggregate_field,
        aggregate_operation_description,
        aggregate_query_skip,
        aggregate_query_name,
        aggregate_requires_scopes,
        group_by_arg_name,
        group_by_arg_description,
        metrics_arg_name,
        metrics_arg_description,
        group_limit_arg_name,
        group_limit_arg_description,
        aggregate_scope_enforcement,
        entity_meta,
    } = input;
    let QueryContext {
        struct_name,
        backend_marker,
        resolver_auth_mode,
        authenticated_directive,
        rls_request_guard,
        where_input,
        where_arg_name,
        where_arg_description,
        field_case_rule,
        ref filter_read_authorization,
        ..
    } = *context;
    // The optional source field retains the entity in a pulled schema.
    let aggregate_row = syn::Ident::new(
        &format!("{}AggregateResultRow", struct_name),
        struct_name.span(),
    );
    let aggregate_row_str = aggregate_row.to_string();
    let aggregate_row_type = if entity_meta.aggregate_source {
        quote! { #aggregate_row }
    } else {
        quote! { ::graphql_orm::graphql::orm::GraphqlAggregateResultRow }
    };
    let map_aggregate_row = if entity_meta.aggregate_source {
        quote! { #aggregate_row::from }
    } else {
        quote! { ::std::convert::identity }
    };
    let aggregate_row_object = if entity_meta.aggregate_source {
        let object = crate::wrapper_object::wrapper_object(quote! {
            /// One generated aggregate result group of this entity.
            #[derive(Clone, Debug)]
            #[graphql(name = #aggregate_row_str, rename_fields = #field_case_rule)]
            pub struct #aggregate_row {
                /// Group keys in requested order.
                pub groups: ::std::vec::Vec<::graphql_orm::graphql::orm::GraphqlAggregateResultValue>,
                /// Metrics in requested order.
                pub metrics: ::std::vec::Vec<::graphql_orm::graphql::orm::GraphqlAggregateResultValue>,
                /// Always null. The field's type is the entity this aggregate reads.
                #[graphql(name = #source_entity_field_name)]
                pub source_entity: ::std::option::Option<#struct_name>,
            }
        });
        quote! {
            #object

            impl ::std::convert::From<::graphql_orm::graphql::orm::GraphqlAggregateResultRow> for #aggregate_row {
                fn from(row: ::graphql_orm::graphql::orm::GraphqlAggregateResultRow) -> Self {
                    Self { groups: row.groups, metrics: row.metrics, source_entity: ::std::option::Option::None }
                }
            }
        }
    } else {
        quote! {}
    };
    let aggregate_definitions = if entity_meta.aggregate {
        quote! {
            #aggregate_row_object

            /// One reviewed aggregate expression for this entity.
            #[derive(Clone, Debug, ::graphql_orm::async_graphql::InputObject)]
            #[graphql(rename_fields = #field_case_rule)]
            pub struct #aggregate_metric_input {
                /// Canonical aggregate operator.
                pub operator: ::graphql_orm::graphql::orm::GraphqlAggregateInputOperator,
                /// Selected public field. Omit only for row count.
                pub field: ::std::option::Option<#aggregate_field>,
            }
        }
    } else {
        quote! {}
    };
    let aggregate_query_method = if entity_meta.aggregate {
        quote! {
            #[doc = #aggregate_operation_description]
            #[graphql(#aggregate_query_skip name = #aggregate_query_name, #aggregate_requires_scopes #authenticated_directive)]
            async fn aggregate(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[__gom_catalogue_type(Option<#where_input>)]
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: ::std::option::Option<#where_input>,
                #[__gom_catalogue_type(Vec<#aggregate_field>)]
                #[graphql(name = #group_by_arg_name, desc = #group_by_arg_description)] group_by: ::std::vec::Vec<#aggregate_field>,
                #[__gom_catalogue_type(Vec<#aggregate_metric_input>)]
                #[graphql(name = #metrics_arg_name, desc = #metrics_arg_description)] metrics: ::std::vec::Vec<#aggregate_metric_input>,
                #[graphql(name = #group_limit_arg_name, desc = #group_limit_arg_description)] group_limit: i32,
            ) -> ::graphql_orm::async_graphql::Result<::std::vec::Vec<#aggregate_row_type>> {
                ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #aggregate_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                )?;
                let db = ::graphql_orm::db::request_database::<#backend_marker>(ctx)?;
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                #filter_read_authorization
                ::graphql_orm::graphql::orm::__gom_fetch_aggregate(
                    ctx,
                    #struct_name::aggregate(db).with_auth(auth_context.as_ref()),
                    where_input,
                    group_by,
                    metrics.into_iter().map(|metric| (metric.operator, metric.field)),
                    group_limit,
                    #map_aggregate_row,
                )?.await
            }
        }
    } else {
        quote! {}
    };
    (
        aggregate_definitions,
        aggregate_query_method,
        aggregate_row_type,
    )
}
