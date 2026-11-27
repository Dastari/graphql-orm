//! Thin output impls for entities whose generated resolvers read stored values.

use proc_macro2::TokenStream;
use quote::quote;
use syn::Ident;

use crate::naming::async_graphql_rename;
use crate::wrapper_object::output_impl;

pub(crate) struct Field {
    pub getter: Ident,
    pub name: String,
    pub description: String,
    pub output_type: TokenStream,
    pub read: TokenStream,
    pub policy: TokenStream,
}

pub(crate) fn expand(
    entity: &Ident,
    entity_description: &str,
    backend: &TokenStream,
    fields: &[Field],
) -> TokenStream {
    let getters = fields.iter().map(|field| {
        let Field {
            getter,
            description,
            output_type,
            read,
            policy,
            ..
        } = field;
        quote! {
            #[doc = #description]
            #[allow(dead_code)]
            async fn #getter(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
            ) -> ::graphql_orm::async_graphql::Result<#output_type> {
                (#policy).authorize::<#backend>(ctx, self).await?;
                #read
            }
        }
    });
    let resolvers = fields.iter().map(|field| {
        let Field { name, output_type, read, policy, .. } = field;
        quote! {
            #name => ::graphql_orm::graphql::wrapper_object::resolve_scalar::<#backend, #output_type>(
                ctx, #policy, self, || { #read },
            ).await,
        }
    });
    let schema_fields = fields.iter().map(|field| {
        let Field { name, description, output_type, .. } = field;
        // The Object derive trims the entire single generated doc string.
        let description = description.trim();
        quote! {
            ::graphql_orm::graphql::wrapper_object::WrapperField {
                name: #name,
                description: ::std::option::Option::Some(#description),
                type_info: <#output_type as ::graphql_orm::async_graphql::OutputType>::create_type_info,
            }
        }
    });
    let type_name = async_graphql_rename(&entity.to_string(), Some("PascalCase"));
    let description = entity_description.trim();
    let output = output_impl(
        entity,
        &type_name,
        &quote!(::std::option::Option::Some(#description)),
        schema_fields,
    );

    quote! {
        #[doc = #entity_description]
        impl #entity {
            #(#getters)*
        }

        impl ::graphql_orm::async_graphql::resolver_utils::ContainerType for #entity {
            async fn resolve_field(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
            ) -> ::graphql_orm::async_graphql::ServerResult<
                ::std::option::Option<::graphql_orm::async_graphql::Value>,
            > {
                match ctx.item.node.name.node.as_str() {
                    #(#resolvers)*
                    _ => ::std::result::Result::Ok(::std::option::Option::None),
                }
            }

            async fn find_entity(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                params: &::graphql_orm::async_graphql::Value,
            ) -> ::graphql_orm::async_graphql::ServerResult<
                ::std::option::Option<::graphql_orm::async_graphql::Value>,
            > {
                ::graphql_orm::graphql::wrapper_object::find_unkeyed_entity(ctx, params)
            }
        }

        #output
    }
}
