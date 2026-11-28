//! Hand-emitted output impls for the generated wrapper objects (#677).
//!
//! The edge, connection, mutation result, change event, bulk delete and bulk
//! update result, and authorized scan page types used to derive
//! `async_graphql::SimpleObject`. That derive's expansion is type- and
//! borrow-checked in the host crate once per entity. These impls keep the
//! derive's observable surface (registry entry, field resolution and the
//! public `async fn` getters) and move the shared glue to
//! `graphql_orm::graphql::wrapper_object`.
//!
//! Only the attributes the generators use are accepted: `name` and
//! `rename_fields` on the struct, `name` on a field, and doc comments, which
//! become descriptions exactly as the derive reads them. Anything else is a
//! generator bug and fails the expansion.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Attribute, Expr, ExprLit, Fields, ItemStruct, Lit, LitStr, Meta};

use crate::naming::async_graphql_rename;

/// Rewrite one generated wrapper struct: drop its `#[graphql]` attributes and
/// emit the output impls the `SimpleObject` derive would have.
pub(crate) fn wrapper_object(tokens: TokenStream) -> TokenStream {
    match expand(tokens) {
        Ok(tokens) => tokens,
        Err(error) => error.to_compile_error(),
    }
}

fn expand(tokens: TokenStream) -> syn::Result<TokenStream> {
    let mut item = syn::parse2::<ItemStruct>(tokens)?;
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new(
            item.generics.span(),
            "generated wrapper objects are not generic",
        ));
    }
    let mut type_name = None;
    let mut rename_fields = None;
    for value in take_graphql_args(&mut item.attrs)? {
        match value {
            (key, value) if key == "name" => type_name = Some(value),
            (key, value) if key == "rename_fields" => rename_fields = Some(value),
            (key, _) => {
                return Err(syn::Error::new(
                    key.span(),
                    "unsupported wrapper object attribute",
                ));
            }
        }
    }
    let ident = item.ident.clone();
    let type_name = type_name.unwrap_or_else(|| {
        // The derive's default type name: Pascal case of the Rust name.
        async_graphql_rename(&ident.to_string(), Some("PascalCase"))
    });
    let type_description = optional_str(rustdoc(&item.attrs)?);

    let Fields::Named(fields) = &mut item.fields else {
        return Err(syn::Error::new(
            item.fields.span(),
            "generated wrapper objects have named fields",
        ));
    };
    let mut getters = Vec::new();
    let mut schema_fields = Vec::new();
    let mut resolvers = Vec::new();
    for field in &mut fields.named {
        let mut field_name = None;
        for (key, value) in take_graphql_args(&mut field.attrs)? {
            if key != "name" {
                return Err(syn::Error::new(
                    key.span(),
                    "unsupported wrapper object field attribute",
                ));
            }
            field_name = Some(value);
        }
        let field_ident = field.ident.clone().expect("named field");
        let field_name = field_name.unwrap_or_else(|| {
            let rust_name = syn::ext::IdentExt::unraw(&field_ident).to_string();
            async_graphql_rename(&rust_name, rename_fields.as_deref())
        });
        let description = optional_str(rustdoc(&field.attrs)?);
        let ty = &field.ty;
        let vis = &field.vis;
        getters.push(quote! {
            #[inline]
            #[allow(missing_docs)]
            #vis async fn #field_ident(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
            ) -> ::graphql_orm::async_graphql::Result<&#ty> {
                ::std::result::Result::Ok(&self.#field_ident)
            }
        });
        schema_fields.push(quote! {
            ::graphql_orm::graphql::wrapper_object::WrapperField {
                name: #field_name,
                description: #description,
                type_info: <#ty as ::graphql_orm::async_graphql::OutputType>::create_type_info,
            }
        });
        resolvers.push(quote! {
            (#field_name, &self.#field_ident as &dyn ::graphql_orm::graphql::wrapper_object::WrapperFieldValue)
        });
    }
    if resolvers.is_empty() {
        return Err(syn::Error::new(
            ident.span(),
            "A GraphQL Object type must define one or more fields.",
        ));
    }

    let output = output_impl(&ident, &type_name, &type_description, schema_fields);
    Ok(quote! {
        #item

        #[allow(clippy::all, clippy::pedantic, unused_variables)]
        impl #ident {
            #(#getters)*
        }

        #[allow(clippy::all, clippy::pedantic)]
        impl ::graphql_orm::async_graphql::resolver_utils::ContainerType for #ident {
            async fn resolve_field(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
            ) -> ::graphql_orm::async_graphql::ServerResult<
                ::std::option::Option<::graphql_orm::async_graphql::Value>,
            > {
                let value =
                    ::graphql_orm::graphql::wrapper_object::select(ctx, &[#(#resolvers),*]);
                ::graphql_orm::graphql::wrapper_object::resolve_field(ctx, value).await
            }
        }

        #output
    })
}

/// Emit the output traits shared by plain generated objects and wrappers.
pub(crate) fn output_impl(
    ident: &syn::Ident,
    type_name: &str,
    type_description: &TokenStream,
    schema_fields: impl IntoIterator<Item = TokenStream>,
) -> TokenStream {
    let schema_fields = schema_fields.into_iter();
    quote! {
        #[allow(clippy::all, clippy::pedantic)]
        impl ::graphql_orm::async_graphql::OutputType for #ident {
            fn type_name() -> ::std::borrow::Cow<'static, ::std::primitive::str> {
                ::std::borrow::Cow::Borrowed(#type_name)
            }

            fn create_type_info(
                registry: &mut ::graphql_orm::async_graphql::registry::Registry,
            ) -> ::std::string::String {
                ::graphql_orm::graphql::wrapper_object::create_type_info::<Self>(
                    registry,
                    #type_name,
                    #type_description,
                    &[#(#schema_fields),*],
                )
            }

            async fn resolve(
                &self,
                ctx: &::graphql_orm::async_graphql::ContextSelectionSet<'_>,
                _field: &::graphql_orm::async_graphql::Positioned<
                    ::graphql_orm::async_graphql::parser::types::Field,
                >,
            ) -> ::graphql_orm::async_graphql::ServerResult<::graphql_orm::async_graphql::Value> {
                ::graphql_orm::async_graphql::resolver_utils::resolve_container(ctx, self).await
            }
        }

        impl ::graphql_orm::async_graphql::ObjectType for #ident {}
    }
}

/// Remove every `#[graphql(...)]` attribute and return its `key = "value"`
/// arguments in order.
fn take_graphql_args(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<(syn::Ident, String)>> {
    let mut args = Vec::new();
    let mut error = None;
    attrs.retain(|attr| {
        if !attr.path().is_ident("graphql") {
            return true;
        }
        let parsed = attr.parse_nested_meta(|meta| {
            let key = meta
                .path
                .get_ident()
                .cloned()
                .ok_or_else(|| meta.error("unsupported wrapper object attribute"))?;
            let value: LitStr = meta.value()?.parse()?;
            args.push((key, value.value()));
            Ok(())
        });
        if let Err(parse_error) = parsed {
            error.get_or_insert(parse_error);
        }
        false
    });
    match error {
        Some(error) => Err(error),
        None => Ok(args),
    }
}

/// The description async-graphql-derive reads from doc comments: each line
/// trimmed, joined by newlines, and the whole trimmed.
fn rustdoc(attrs: &[Attribute]) -> syn::Result<Option<String>> {
    let mut combined = String::new();
    for attr in attrs {
        if let Meta::NameValue(name_value) = &attr.meta
            && name_value.path.is_ident("doc")
        {
            match &name_value.value {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(doc), ..
                }) => {
                    combined.push('\n');
                    combined.push_str(doc.value().trim());
                }
                other => {
                    return Err(syn::Error::new(
                        other.span(),
                        "generated wrapper object docs must be literals",
                    ));
                }
            }
        }
    }
    let trimmed = combined.trim();
    Ok((!trimmed.is_empty()).then(|| trimmed.to_owned()))
}

fn optional_str(value: Option<String>) -> TokenStream {
    match value {
        Some(value) => quote! { ::std::option::Option::Some(#value) },
        None => quote! { ::std::option::Option::None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::ToTokens;

    fn expand_ok(tokens: TokenStream) -> String {
        expand(tokens).expect("wrapper expands").to_string()
    }

    #[test]
    fn graphql_attributes_are_removed_and_other_attributes_kept() {
        let output = expand_ok(quote! {
            /// Edge containing a node and cursor
            #[derive(Debug, Clone)]
            #[graphql(name = "PostEdge", rename_fields = "camelCase")]
            pub struct PostEdge {
                /// The item at the end of the edge
                pub node: Post,
                #[graphql(name = "pageCursor")]
                pub cursor: String,
            }
        });
        let item: ItemStruct = syn::parse_str(output.split("# [allow").next().unwrap()).unwrap();
        assert!(!item.to_token_stream().to_string().contains("graphql ("));
        assert!(output.contains("derive (Debug , Clone)"));
        assert!(output.contains("\"PostEdge\""));
        assert!(output.contains("\"node\""));
        assert!(output.contains("\"pageCursor\""));
        assert!(output.contains("\"Edge containing a node and cursor\""));
        assert!(output.contains("\"The item at the end of the edge\""));
        assert!(!output.contains("SimpleObject"));
    }

    #[test]
    fn rename_rule_and_default_type_name_follow_the_derive() {
        let output = expand_ok(quote! {
            #[graphql(rename_fields = "PascalCase")]
            pub struct SynthRecord0AuthorizedScanPage {
                pub page_info: PageInfo,
                pub error_code: Option<String>,
            }
        });
        assert!(output.contains("\"SynthRecord0AuthorizedScanPage\""));
        assert!(output.contains("\"PageInfo\""));
        assert!(output.contains("\"ErrorCode\""));
    }

    #[test]
    fn multi_line_docs_are_trimmed_per_line_and_overall() {
        let attrs: Vec<Attribute> = vec![
            syn::parse_quote!(#[doc = ""]),
            syn::parse_quote!(#[doc = "  first  "]),
            syn::parse_quote!(#[doc = ""]),
            syn::parse_quote!(#[doc = " second"]),
        ];
        assert_eq!(rustdoc(&attrs).unwrap().as_deref(), Some("first\n\nsecond"));
        assert_eq!(rustdoc(&[]).unwrap(), None);
    }

    /// #677: the seven per-entity wrappers no longer expand `SimpleObject`
    /// in the host crate; they carry the hand-emitted output impls instead.
    #[test]
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    fn generated_wrappers_are_hand_emitted() {
        let input: syn::DeriveInput = syn::parse_quote! {
            #[graphql_entity(table = "records", plural = "Records", keyset = "id asc")]
            struct Record { #[primary_key] #[sortable] id: String }
        };
        let output = crate::operations::generate_graphql_operations(&input)
            .unwrap()
            .to_string();
        assert!(
            !output.contains("SimpleObject"),
            "a wrapper still derives SimpleObject"
        );
        for name in [
            "RecordEdge",
            "RecordConnection",
            "RecordResult",
            "RecordChangedEvent",
            "DeleteRecordsResult",
            "UpdateRecordsResult",
            "RecordAuthorizedScanPage",
        ] {
            assert!(
                output.contains(&format!(
                    "impl :: graphql_orm :: async_graphql :: OutputType for {name} "
                )),
                "{name} has no hand-emitted OutputType impl"
            );
        }
    }

    #[test]
    fn unknown_attributes_are_rejected() {
        let error = expand(quote! {
            #[graphql(name = "X", shareable)]
            pub struct X { pub a: i32 }
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("unsupported") || error.to_string().contains("expected")
        );
        let error = expand(quote! {
            #[graphql(name = "X")]
            pub struct X { #[graphql(skip = "true")] pub a: i32 }
        })
        .unwrap_err();
        assert!(error.to_string().contains("unsupported"));
    }
}
