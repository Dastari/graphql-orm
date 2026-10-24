use crate::backend::{backend_marker_tokens, resolve_backend};
use crate::entity::{has_graphql_complex, parse_entity_metadata, parse_field_metadata};
use quote::quote;
use syn::{Attribute, Expr, ItemStruct, Lit, Meta, Token, punctuated::Punctuated};

fn guard(attributes: &[Attribute]) -> syn::Result<Option<Expr>> {
    let mut result = None;
    for attribute in attributes
        .iter()
        .filter(|attr| attr.path().is_ident("graphql"))
    {
        let options = attribute.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
        for option in options {
            if let Meta::NameValue(value) = option {
                if value.path.is_ident("guard") {
                    let Expr::Lit(literal) = &value.value else {
                        return Err(syn::Error::new_spanned(
                            &value.value,
                            "guard requires a string expression",
                        ));
                    };
                    let Lit::Str(value) = &literal.lit else {
                        return Err(syn::Error::new_spanned(
                            &literal.lit,
                            "guard requires a string expression",
                        ));
                    };
                    if result.is_some() {
                        return Err(syn::Error::new_spanned(
                            value,
                            "guard may only be declared once",
                        ));
                    }
                    result = Some(value.parse()?);
                }
            }
        }
    }
    Ok(result)
}

fn install_guard(
    attributes: &mut Vec<Attribute>,
    expression: proc_macro2::TokenStream,
) -> syn::Result<()> {
    let mut retained = Vec::new();
    for attribute in attributes.drain(..) {
        if !attribute.path().is_ident("graphql") {
            retained.push(attribute);
            continue;
        }
        let options = attribute
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?
            .into_iter()
            .filter(|option| !option.path().is_ident("guard"))
            .collect::<Vec<_>>();
        if !options.is_empty() {
            retained.push(syn::parse_quote!(#[graphql(#(#options),*)]));
        }
    }
    let expression = syn::LitStr::new(&expression.to_string(), proc_macro2::Span::call_site());
    retained.push(syn::parse_quote!(#[graphql(guard = #expression)]));
    *attributes = retained;
    Ok(())
}

fn combine(
    existing: Option<Expr>,
    generated: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    match existing {
        Some(existing) => {
            quote!(::graphql_orm::async_graphql::GuardExt::and(#existing, #generated))
        }
        None => generated.clone(),
    }
}

pub(crate) fn expand(mut input: ItemStruct) -> syn::Result<proc_macro2::TokenStream> {
    if !has_graphql_complex(&input.attrs) {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "graphql_policy_object requires #[graphql(complex)] and SimpleObject",
        ));
    }
    let simple = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("derive"))
        .any(|attr| {
            attr.parse_args_with(Punctuated::<syn::Path, Token![,]>::parse_terminated)
                .is_ok_and(|paths| {
                    paths.iter().any(|path| {
                        path.segments
                            .last()
                            .is_some_and(|segment| segment.ident == "SimpleObject")
                    })
                })
        });
    if !simple {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "place graphql_policy_object before #[derive(..., SimpleObject, ...)]",
        ));
    }
    let metadata = parse_entity_metadata(&input.attrs)?;
    let backend = resolve_backend(
        metadata.backend.as_deref(),
        input.ident.span(),
        "graphql_policy_object",
    )?;
    let backend = backend_marker_tokens(backend);
    let generated =
        quote!(::graphql_orm::graphql::orm::EntityFieldPolicyGuard::<_, #backend>::new(self));
    let object_guard = guard(&input.attrs)?;
    install_guard(&mut input.attrs, combine(object_guard, &generated))?;
    for field in input.fields.iter_mut() {
        let metadata = parse_field_metadata(field)?;
        if metadata.is_relation || metadata.graphql_skip {
            continue;
        }
        if !metadata.read || metadata.is_private || metadata.input_only {
            field.attrs.push(syn::parse_quote!(#[graphql(skip)]));
            continue;
        }
        if let Some(existing) = guard(&field.attrs)? {
            install_guard(&mut field.attrs, combine(Some(existing), &generated))?;
        }
    }
    Ok(quote!(#input))
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;

    #[test]
    fn preserves_object_and_field_guards_and_naming() {
        let input = syn::parse_quote! {
            #[derive(GraphQLEntity, SimpleObject, GraphQLRelations)]
            #[graphql(guard = "ObjectGuard", complex, rename_fields = "PascalCase")]
            #[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
            struct Record {
                #[primary_key]
                id: String,
                #[graphql(guard = "FieldGuard")]
                #[graphql_orm(read_policy = "admin")]
                secret: String,
                #[graphql_orm(private)]
                custody: String,
            }
        };
        let expanded = expand(input).unwrap().to_string();
        assert!(expanded.contains("EntityFieldPolicyGuard"));
        assert!(expanded.contains("GuardExt :: and (ObjectGuard"));
        assert!(expanded.contains("GuardExt :: and (FieldGuard"));
        assert!(expanded.contains("PascalCase"));
        assert!(expanded.contains("graphql (skip)"));
    }

    #[test]
    fn rejects_missing_simple_object_contract() {
        let input = syn::parse_quote! { struct Record { id: String } };
        assert!(expand(input).is_err());
    }
}
