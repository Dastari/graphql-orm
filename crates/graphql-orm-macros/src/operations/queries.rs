use proc_macro2::TokenStream;
use quote::quote;

mod keyset;
pub(super) use keyset::{KeysetEmission, KeysetInput, KeysetPart, keyset};
mod search;
pub(super) use search::{SearchInput, search};
mod aggregate;
pub(super) use aggregate::{AggregateInput, aggregate};
mod repository;
pub(super) use repository::{RepositoryKeyMethodsInput, repository_key_methods};
mod root;
pub(super) use root::{RootInput, root};

pub(super) struct QueryContext<'a> {
    pub(super) struct_name: &'a syn::Ident,
    pub(super) backend_marker: &'a TokenStream,
    pub(super) resolver_auth_mode: &'a TokenStream,
    pub(super) authenticated_directive: &'a TokenStream,
    pub(super) rls_request_guard: &'a TokenStream,
    pub(super) entity_name_lit: &'a str,
    pub(super) where_input: &'a syn::Ident,
    pub(super) where_arg_name: &'a str,
    pub(super) where_arg_description: &'a str,
    pub(super) page_arg_name: &'a str,
    pub(super) page_arg_description: &'a str,
    pub(super) order_by_arg_name: &'a str,
    pub(super) order_by_arg_description: &'a str,
    pub(super) field_case_rule: &'a str,
    pub(super) page_info_field_name: &'a str,
    pub(super) connection_type: &'a syn::Ident,
    pub(super) filter_read_authorization: TokenStream,
}

pub(super) fn filter_read_authorization(
    struct_name: &syn::Ident,
    backend_marker: &TokenStream,
) -> TokenStream {
    // Filtering reveals field information even when the response selects only
    // unrelated fields. Authorize original recursive predicate intent before
    // any list/count/scan SQL, using the same public identities as field reads.
    quote! {
        let mut where_input = where_input;
        // One budget per statement: where-input nodes, and the relation
        // policy SQL that relation filters and count orderings add.
        let mut __gom_statement_budget = ::graphql_orm::graphql::orm::RelationFilterBudget::new();
        if let ::std::option::Option::Some(filter) = where_input.as_mut() {
            filter.__gom_prepare_read(ctx, 0, &mut __gom_statement_budget).await?;
        }

        if let ::std::option::Option::Some(filter) = where_input.as_ref() {
            ::graphql_orm::graphql::orm::authorize_read_fields::<#struct_name, #backend_marker>(
                db, ctx, ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter),
            ).await?;
        }
    }
}

pub(super) struct ConnectionInput<'a> {
    pub(super) edge_type_str: &'a str,
    pub(super) edge_type: &'a syn::Ident,
    pub(super) connection_type_str: &'a str,
    pub(super) full_write_surface: bool,
}

pub(super) fn connection(context: &QueryContext<'_>, input: ConnectionInput<'_>) -> TokenStream {
    let ConnectionInput {
        edge_type_str,
        edge_type,
        connection_type_str,
        full_write_surface,
    } = input;
    let QueryContext {
        struct_name,
        field_case_rule,
        page_info_field_name,
        connection_type,
        ..
    } = *context;
    let (
        edge_docs,
        node_docs,
        cursor_docs,
        connection_docs,
        edges_docs,
        page_info_docs,
        conversion_docs,
        empty_docs,
    ) = if full_write_surface {
        (
            quote! { /// Edge containing a node and cursor
            },
            quote! { /// The item at the end of the edge
            },
            quote! { /// A cursor for pagination
            },
            quote! { /// Connection containing edges and page info
            },
            quote! { /// The edges in this connection
            },
            quote! { /// Pagination information
            },
            quote! { /// Create from a generic Connection
            },
            quote! { /// Create an empty connection
            },
        )
    } else {
        Default::default()
    };

    let edge_object = crate::wrapper_object::wrapper_object(quote! {
        #edge_docs
        #[derive(Debug, Clone)]
        #[graphql(name = #edge_type_str, rename_fields = #field_case_rule)]
        pub struct #edge_type {
            #node_docs
            pub node: #struct_name,
            #cursor_docs
            pub cursor: ::std::string::String,
        }
    });
    let connection_object = crate::wrapper_object::wrapper_object(quote! {
        #connection_docs
        #[derive(Debug, Clone)]
        #[graphql(name = #connection_type_str, rename_fields = #field_case_rule)]
        pub struct #connection_type {
            #edges_docs
            pub edges: ::std::vec::Vec<#edge_type>,
            #page_info_docs
            #[graphql(name = #page_info_field_name)]
            pub page_info: ::graphql_orm::graphql::pagination::PageInfo,
        }
    });
    let connection_impl = quote! {
        impl #connection_type {
            #conversion_docs
            pub fn from_generic(conn: ::graphql_orm::graphql::pagination::Connection<#struct_name>) -> Self {
                Self {
                    edges: conn.edges.into_iter().map(|e| #edge_type {
                        node: e.node,
                        cursor: e.cursor,
                    }).collect(),
                    page_info: conn.page_info,
                }
            }

            #empty_docs
            pub fn empty() -> Self {
                Self {
                    edges: ::std::vec::Vec::new(),
                    page_info: ::graphql_orm::graphql::pagination::PageInfo::default(),
                }
            }
        }

    };
    quote! { #edge_object #connection_object #connection_impl }
}

#[cfg(test)]
mod tests;
