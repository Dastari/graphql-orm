//! Derives generated operation-catalogue arguments from resolver signatures.
//!
//! The SDL arguments of a generated root field come from its async-graphql
//! resolver parameters. Catalogue descriptors therefore name only their root
//! coordinate through [`placeholder`], and [`fill`] replaces each placeholder
//! with the argument list read from the one resolver that defines that field.
//! A descriptor without a resolver, or a resolver without a descriptor, is a
//! compile error, so the catalogue cannot drift from the SDL.
use proc_macro2::{Span, TokenStream};
use quote::quote;
use std::collections::BTreeMap;
use syn::{Token, punctuated::Punctuated, spanned::Spanned, visit_mut::VisitMut};

const PLACEHOLDER: &str = "__gom_catalog_root_arguments";

/// Root object kinds that carry generated operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RootKind {
    Query,
    Mutation,
    Subscription,
}

impl RootKind {
    fn label(self) -> &'static str {
        match self {
            Self::Query => "Query",
            Self::Mutation => "Mutation",
            Self::Subscription => "Subscription",
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        match label {
            "Query" => Some(Self::Query),
            "Mutation" => Some(Self::Mutation),
            "Subscription" => Some(Self::Subscription),
            _ => None,
        }
    }
}

/// Stands in for one descriptor's argument list until [`fill`] runs.
pub(crate) fn placeholder(kind: RootKind, field_name: &str) -> TokenStream {
    let macro_name = syn::Ident::new(PLACEHOLDER, Span::call_site());
    let kind = syn::Ident::new(kind.label(), Span::call_site());
    quote! { #macro_name!(#kind, #field_name) }
}

struct Resolver {
    arguments: Vec<TokenStream>,
    span: Span,
}

fn graphql_attribute_metas(attrs: &[syn::Attribute]) -> syn::Result<Vec<syn::Meta>> {
    let mut metas = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("graphql")) {
        if matches!(attr.meta, syn::Meta::Path(_)) {
            continue;
        }
        metas.extend(attr.parse_args_with(Punctuated::<syn::Meta, Token![,]>::parse_terminated)?);
    }
    Ok(metas)
}

fn string_value(metas: &[syn::Meta], key: &str) -> syn::Result<Option<syn::LitStr>> {
    for meta in metas {
        if let syn::Meta::NameValue(value) = meta
            && value.path.is_ident(key)
        {
            return match &value.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(literal),
                    ..
                }) => Ok(Some(literal.clone())),
                other => Err(syn::Error::new(
                    other.span(),
                    format!("generated resolver `{key}` must be a string literal"),
                )),
            };
        }
    }
    Ok(None)
}

fn has_flag(metas: &[syn::Meta], flag: &str) -> bool {
    metas
        .iter()
        .any(|meta| matches!(meta, syn::Meta::Path(path) if path.is_ident(flag)))
}

fn resolver_arguments(method: &syn::ImplItemFn) -> syn::Result<Vec<TokenStream>> {
    let mut arguments = Vec::new();
    for input in &method.sig.inputs {
        let syn::FnArg::Typed(argument) = input else {
            continue;
        };
        // The async-graphql context is the only borrowed resolver parameter.
        if matches!(argument.ty.as_ref(), syn::Type::Reference(_)) {
            continue;
        }
        let metas = graphql_attribute_metas(&argument.attrs)?;
        let name = string_value(&metas, "name")?.ok_or_else(|| {
            syn::Error::new(
                argument.span(),
                "generated root arguments must declare an explicit GraphQL name",
            )
        })?;
        let description = string_value(&metas, "desc")?.ok_or_else(|| {
            syn::Error::new(
                argument.span(),
                "generated root arguments must declare a GraphQL description",
            )
        })?;
        let ty = &argument.ty;
        arguments.push(quote! {
            ::graphql_orm::graphql::orm::GraphqlOperationArgumentDescriptor::generated_with_description(
                #name,
                #description,
                stringify!(#ty),
                <#ty as ::graphql_orm::async_graphql::InputType>::qualified_type_name(),
            )
        });
    }
    Ok(arguments)
}

fn root_kind_of(implementation: &syn::ItemImpl, entity: &str) -> Option<RootKind> {
    if implementation.trait_.is_some() {
        return None;
    }
    let syn::Type::Path(target) = implementation.self_ty.as_ref() else {
        return None;
    };
    let name = target.path.segments.last()?.ident.to_string();
    let root = name.strip_prefix(entity)?;
    let async_graphql_root = |kind: &str| {
        implementation.attrs.iter().any(|attr| {
            attr.path()
                .segments
                .last()
                .is_some_and(|segment| segment.ident == kind)
        })
    };
    match root {
        "Queries" if async_graphql_root("Object") => Some(RootKind::Query),
        "Mutations" if async_graphql_root("Object") => Some(RootKind::Mutation),
        "Subscriptions" if async_graphql_root("Subscription") => Some(RootKind::Subscription),
        _ => None,
    }
}

fn collect_resolvers(
    file: &syn::File,
    entity: &str,
) -> syn::Result<BTreeMap<(RootKind, String), Resolver>> {
    let mut resolvers = BTreeMap::new();
    for item in &file.items {
        let syn::Item::Impl(implementation) = item else {
            continue;
        };
        let Some(kind) = root_kind_of(implementation, entity) else {
            continue;
        };
        for item in &implementation.items {
            let syn::ImplItem::Fn(method) = item else {
                continue;
            };
            let metas = graphql_attribute_metas(&method.attrs)?;
            // Federation entity resolvers are not root fields.
            if has_flag(&metas, "entity") {
                continue;
            }
            let name = string_value(&metas, "name")?.ok_or_else(|| {
                syn::Error::new(
                    method.sig.span(),
                    "generated root resolvers must declare an explicit GraphQL name",
                )
            })?;
            let field_name = name.value();
            // Every generated root field is described, underscore-prefixed
            // or not: routers exempt only the federation `_entities` and
            // `_service` fields, which these objects never define.
            let resolver = Resolver {
                arguments: resolver_arguments(method)?,
                span: method.sig.span(),
            };
            // Two resolvers share a name when, for example, `plural` equals the
            // entity name. async-graphql's registry keeps the later field's
            // arguments, and so does the SDL, so the later resolver wins here.
            resolvers.insert((kind, field_name), resolver);
        }
    }
    Ok(resolvers)
}

struct Fill {
    resolvers: BTreeMap<(RootKind, String), Resolver>,
    used: std::collections::BTreeSet<(RootKind, String)>,
    error: Option<syn::Error>,
}

impl Fill {
    fn replacement(&mut self, mac: &syn::Macro) -> syn::Result<syn::Expr> {
        let parser = |input: syn::parse::ParseStream<'_>| {
            let kind: syn::Ident = input.parse()?;
            input.parse::<Token![,]>()?;
            let field: syn::LitStr = input.parse()?;
            Ok((kind, field))
        };
        let (kind_ident, field) = mac.parse_body_with(parser)?;
        let kind = RootKind::from_label(&kind_ident.to_string())
            .ok_or_else(|| syn::Error::new(kind_ident.span(), "unknown generated root kind"))?;
        let key = (kind, field.value());
        let resolver = self.resolvers.get(&key).ok_or_else(|| {
            syn::Error::new(
                field.span(),
                format!(
                    "generated catalogue describes {}.{} but no resolver defines it",
                    kind.label(),
                    key.1
                ),
            )
        })?;
        let arguments = &resolver.arguments;
        self.used.insert(key);
        Ok(syn::parse_quote! { vec![#(#arguments),*] })
    }
}

impl VisitMut for Fill {
    fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
        if let syn::Expr::Macro(mac) = expr
            && mac.mac.path.is_ident(PLACEHOLDER)
        {
            match self.replacement(&mac.mac) {
                Ok(replacement) => *expr = replacement,
                Err(error) => {
                    if self.error.is_none() {
                        self.error = Some(error);
                    }
                }
            }
            return;
        }
        syn::visit_mut::visit_expr_mut(self, expr);
    }
}

/// Replaces every catalogue placeholder with its resolver's arguments.
pub(crate) fn fill(file: &mut syn::File, entity: &syn::Ident) -> syn::Result<()> {
    let resolvers = collect_resolvers(file, &entity.to_string())?;
    let mut fill = Fill {
        resolvers,
        used: Default::default(),
        error: None,
    };
    fill.visit_file_mut(file);
    if let Some(error) = fill.error {
        return Err(error);
    }
    if let Some(((kind, field), resolver)) = fill
        .resolvers
        .iter()
        .find(|(key, _)| !fill.used.contains(*key))
    {
        return Err(syn::Error::new(
            resolver.span,
            format!(
                "generated resolver {}.{field} has no operation catalogue descriptor",
                kind.label()
            ),
        ));
    }
    Ok(())
}
