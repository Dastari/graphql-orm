use super::*;
use crate::naming::{apply_graphql_case, selected_argument_case};

/// One canonical generated to-many relationship argument contract.
///
/// Both the `GraphQLRelations` resolver signature and `GraphQLEntity`
/// semantic metadata consume this value. Keeping the public names, Rust input
/// types, and semantic type references together prevents either emitted
/// contract from drifting independently.
pub(crate) struct GeneratedRelationshipArgumentContract {
    where_name: String,
    order_by_name: String,
    page_name: String,
    where_input: Ident,
    order_by_input: Ident,
}

impl GeneratedRelationshipArgumentContract {
    pub(crate) fn to_many(target_type: &str, span: proc_macro2::Span) -> Self {
        let argument_case = selected_argument_case();
        Self {
            where_name: apply_graphql_case("where", argument_case),
            order_by_name: apply_graphql_case("orderBy", argument_case),
            page_name: apply_graphql_case("page", argument_case),
            where_input: Ident::new(&format!("{target_type}WhereInput"), span),
            order_by_input: Ident::new(&format!("{target_type}OrderByInput"), span),
        }
    }

    pub(crate) fn where_name(&self) -> &str {
        &self.where_name
    }

    pub(crate) fn order_by_name(&self) -> &str {
        &self.order_by_name
    }

    pub(crate) fn page_name(&self) -> &str {
        &self.page_name
    }

    pub(crate) fn where_rust_type(&self) -> proc_macro2::TokenStream {
        let input = &self.where_input;
        quote! { Option<#input> }
    }

    pub(crate) fn order_by_rust_type(&self) -> proc_macro2::TokenStream {
        let input = &self.order_by_input;
        quote! { Option<#input> }
    }

    pub(crate) fn page_rust_type(&self) -> proc_macro2::TokenStream {
        quote! { Option<::graphql_orm::graphql::orm::PageInput> }
    }

    pub(crate) fn semantic_descriptors(&self) -> proc_macro2::TokenStream {
        let where_name = self.where_name();
        let order_by_name = self.order_by_name();
        let page_name = self.page_name();
        let where_input = self.where_input.to_string();
        let order_by_input = self.order_by_input.to_string();
        quote! {
            vec![
                ::graphql_orm::graphql::orm::GraphqlSemanticArgumentDescriptor {
                    graphql_name: ::std::string::ToString::to_string(#where_name),
                    description: ::std::string::ToString::to_string("Filter related records"),
                    type_ref: ::graphql_orm::graphql::orm::GraphqlSemanticTypeRef::named(
                        #where_input,
                        ::graphql_orm::graphql::orm::GraphqlSemanticTypeKind::Object,
                        true,
                    ),
                },
                ::graphql_orm::graphql::orm::GraphqlSemanticArgumentDescriptor {
                    graphql_name: ::std::string::ToString::to_string(#order_by_name),
                    description: ::std::string::ToString::to_string("Order related records"),
                    type_ref: ::graphql_orm::graphql::orm::GraphqlSemanticTypeRef::named(
                        #order_by_input,
                        ::graphql_orm::graphql::orm::GraphqlSemanticTypeKind::Object,
                        true,
                    ),
                },
                ::graphql_orm::graphql::orm::GraphqlSemanticArgumentDescriptor {
                    graphql_name: ::std::string::ToString::to_string(#page_name),
                    description: ::std::string::ToString::to_string("Bound the related record page"),
                    type_ref: ::graphql_orm::graphql::orm::GraphqlSemanticTypeRef::named(
                        "PageInput",
                        ::graphql_orm::graphql::orm::GraphqlSemanticTypeKind::Object,
                        true,
                    ),
                },
            ]
        }
    }
}
