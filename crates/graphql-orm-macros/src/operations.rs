use super::*;
use crate::backend::{
    BackendKind, backend_dialect_tokens, backend_marker_tokens, backend_pool_type_tokens,
    backend_render_identifier_path, resolve_backend,
};
use crate::entity::{
    FieldMetadata, decimal_definition_tokens, is_bool_type, is_byte_vec_type, is_option_type,
    is_serde_json_value_or_option, is_string_type, is_uuid_type, is_vec_type, option_inner_type,
    parse_entity_metadata, parse_field_metadata, resolver_auth_mode_tokens,
    spatial_geometry_type_tokens, type_path_last_ident,
};
use crate::naming::{
    apply_graphql_case, graphql_field_name, selected_argument_case, selected_field_case,
    selected_field_case_rule, selected_resolver_case,
};
use std::collections::HashMap;
use syn::spanned::Spanned;

fn runtime_keyset_value(ty: &syn::Type) -> Option<proc_macro2::TokenStream> {
    let (inner, optional) = option_inner_type(ty)
        .map(|inner| (inner, true))
        .unwrap_or((ty, false));
    let type_name = quote! { #inner }.to_string().replace(' ', "");
    let value = if is_string_type(inner) {
        quote! { ::graphql_orm::graphql::pagination::KeysetValue::String(value.clone()) }
    } else if is_byte_vec_type(inner) {
        quote! { ::graphql_orm::graphql::pagination::KeysetValue::Bytes(value.clone()) }
    } else if is_uuid_type(inner) {
        quote! { ::graphql_orm::graphql::pagination::KeysetValue::Uuid(value.to_string()) }
    } else if is_bool_type(inner) {
        quote! { ::graphql_orm::graphql::pagination::KeysetValue::Bool(*value) }
    } else if matches!(type_name.as_str(), "f32" | "f64") {
        quote! { ::graphql_orm::graphql::pagination::KeysetValue::Float(*value as f64) }
    } else if matches!(
        type_name.as_str(),
        "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32"
    ) {
        quote! { ::graphql_orm::graphql::pagination::KeysetValue::Int(*value as i64) }
    } else {
        return None;
    };
    Some(if optional {
        quote! { match value.as_ref() { Some(value) => #value, None => ::graphql_orm::graphql::pagination::KeysetValue::Null } }
    } else {
        value
    })
}

fn semantic_argument_description(value: &str) -> String {
    let mut output = String::new();
    for (index, character) in value.chars().enumerate() {
        if index > 0 && character.is_uppercase() {
            output.push(' ');
        }
        if index == 0 {
            output.extend(character.to_uppercase());
        } else {
            output.extend(character.to_lowercase());
        }
    }
    output
}

#[derive(Clone)]
enum FixedOperationAuthorization {
    AllScopes(Vec<syn::LitStr>),
    AnyScopes(Vec<Vec<syn::LitStr>>),
    AllScopeTemplates(Vec<syn::LitStr>),
    AnyScopeTemplates(Vec<Vec<syn::LitStr>>),
}

impl FixedOperationAuthorization {
    fn requirement_tokens(&self) -> proc_macro2::TokenStream {
        match self {
            Self::AllScopes(scopes) | Self::AllScopeTemplates(scopes) => quote! {
                ::graphql_orm::graphql::orm::GraphqlAuthorizationRequirement::AllScopes {
                    scopes: vec![#(::std::string::ToString::to_string(#scopes)),*],
                }
            },
            Self::AnyScopes(alternatives) | Self::AnyScopeTemplates(alternatives) => {
                let alternatives = alternatives.iter().map(|alternative| {
                    quote! {
                        ::graphql_orm::graphql::orm::GraphqlAuthorizationScopeSet {
                            scopes: vec![#(::std::string::ToString::to_string(#alternative)),*],
                        }
                    }
                });
                quote! {
                    ::graphql_orm::graphql::orm::GraphqlAuthorizationRequirement::AnyScopes {
                        alternatives: vec![#(#alternatives),*],
                    }
                }
            }
        }
    }

    fn enforcement_tokens(&self) -> proc_macro2::TokenStream {
        let alternatives = match self {
            Self::AllScopes(scopes) => vec![quote! { &[#(#scopes),*] }],
            Self::AnyScopes(alternatives) => alternatives
                .iter()
                .map(|alternative| quote! { &[#(#alternative),*] })
                .collect(),
            Self::AllScopeTemplates(_) | Self::AnyScopeTemplates(_) => return quote! {},
        };
        quote! {
            let _scope_subject = ::graphql_orm::graphql::auth::enforce_resolver_scopes(
                ctx,
                &[#(#alternatives),*],
            )?;
        }
    }

    fn requires_scopes_tokens(&self) -> proc_macro2::TokenStream {
        let alternatives: Vec<&[syn::LitStr]> = match self {
            Self::AllScopes(scopes) => vec![scopes],
            Self::AnyScopes(alternatives) => alternatives.iter().map(Vec::as_slice).collect(),
            Self::AllScopeTemplates(_) | Self::AnyScopeTemplates(_) => return quote! {},
        };
        let alternatives = alternatives.into_iter().map(|alternative| {
            let scopes = alternative
                .iter()
                .map(syn::LitStr::value)
                .collect::<Vec<_>>()
                .join(" ");
            let scopes = syn::LitStr::new(&scopes, alternative[0].span());
            quote! { requires_scopes = #scopes, }
        });
        quote! { #(#alternatives)* }
    }

    fn federation_requires_scopes_directive_tokens(&self) -> proc_macro2::TokenStream {
        let alternatives: Vec<&[syn::LitStr]> = match self {
            Self::AllScopes(scopes) => vec![scopes],
            Self::AnyScopes(alternatives) => alternatives.iter().map(Vec::as_slice).collect(),
            Self::AllScopeTemplates(_) | Self::AnyScopeTemplates(_) => return quote! {},
        };
        let alternatives = alternatives.into_iter().map(|alternative| {
            quote! {
                vec![#(::std::string::ToString::to_string(#alternative)),*]
            }
        });
        quote! {
            directive = ::graphql_orm::graphql::federation::federation_requires_scopes::apply(
                vec![#(#alternatives),*]
            ),
        }
    }

    fn template_alternatives(&self) -> Option<Vec<&[syn::LitStr]>> {
        match self {
            Self::AllScopeTemplates(scopes) => Some(vec![scopes]),
            Self::AnyScopeTemplates(alternatives) => {
                Some(alternatives.iter().map(Vec::as_slice).collect())
            }
            Self::AllScopes(_) | Self::AnyScopes(_) => None,
        }
    }
}

fn parse_fixed_scope_array(expr: &syn::Expr, description: &str) -> syn::Result<Vec<syn::LitStr>> {
    let syn::Expr::Array(array) = expr else {
        return Err(syn::Error::new_spanned(
            expr,
            format!("operation_authorization {description} must be an array of string literals"),
        ));
    };

    array
        .elems
        .iter()
        .map(|element| match element {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(scope),
                ..
            }) => Ok(scope.clone()),
            _ => Err(syn::Error::new_spanned(
                element,
                format!("operation_authorization {description} must contain only string literals"),
            )),
        })
        .collect()
}

fn validate_fixed_scope(scope: &syn::LitStr) -> syn::Result<()> {
    let value = scope.value();
    if value.is_empty() {
        return Err(syn::Error::new_spanned(
            scope,
            "operation_authorization scope strings must not be empty",
        ));
    }
    if value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(syn::Error::new_spanned(
            scope,
            "operation_authorization scope strings must not contain whitespace or control characters",
        ));
    }
    if value.contains('{') || value.contains('}') {
        return Err(syn::Error::new_spanned(
            scope,
            "operation_authorization scope strings must not contain braces",
        ));
    }
    Ok(())
}

fn scope_template_references(scope: &syn::LitStr) -> syn::Result<Vec<String>> {
    let value = scope.value();
    if value.is_empty() {
        return Err(syn::Error::new_spanned(
            scope,
            "operation_authorization scope templates must not be empty",
        ));
    }
    if value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(syn::Error::new_spanned(
            scope,
            "operation_authorization scope templates must not contain whitespace or control characters",
        ));
    }

    let mut references = Vec::new();
    let mut remaining = value.as_str();
    while let Some(start) = remaining.find(['{', '}']) {
        if remaining.as_bytes()[start] == b'}' {
            return Err(syn::Error::new_spanned(
                scope,
                "operation_authorization scope templates must contain balanced `{argument}` placeholders",
            ));
        }
        let after_start = &remaining[start + 1..];
        let Some(end) = after_start.find('}') else {
            return Err(syn::Error::new_spanned(
                scope,
                "operation_authorization scope templates must contain balanced `{argument}` placeholders",
            ));
        };
        let argument = &after_start[..end];
        let valid_name = argument
            .bytes()
            .enumerate()
            .all(|(index, byte)| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'_' => true,
                b'0'..=b'9' => index > 0,
                _ => false,
            });
        if argument.is_empty() || argument.contains('{') || !valid_name {
            return Err(syn::Error::new_spanned(
                scope,
                "operation_authorization scope template placeholders must be GraphQL argument names",
            ));
        }
        references.push(argument.to_string());
        remaining = &after_start[end + 1..];
    }
    Ok(references)
}

/// Read categories whose requirements guard relation traversal when the
/// category a traversal maps to (`list` or `single_read`) is not declared.
const TRAVERSAL_READ_CATEGORIES: &[&str] = &[
    "list",
    "single_read",
    "keyset_list",
    "search",
    "subscription",
];

/// Emit the entity's relation traversal requirement.
///
/// Relation traversal into an entity needs the same auth mode and fixed
/// operation scopes as the corresponding root read: `list` for to-many and
/// `single_read` for to-one traversal. When that category is not declared but
/// another read category is, every declared read category's requirement
/// applies. Scope templates bind root arguments that a traversal does not
/// have, so they refuse it. `GraphQLEntity` and
/// `RepositoryEntity` emit this so every relation target carries it. A
/// malformed declaration, which `GraphQLOperations` reports, refuses every
/// traversal rather than adding a second diagnostic here.
pub(crate) fn relation_traversal_authorization_tokens(
    input: &DeriveInput,
) -> proc_macro2::TokenStream {
    let struct_name = &input.ident;
    let requirement = (|| -> syn::Result<proc_macro2::TokenStream> {
        let entity_meta = parse_entity_metadata(&input.attrs)?;
        let resolver_auth_mode =
            resolver_auth_mode_tokens(entity_meta.auth.as_deref(), struct_name.span())?;
        let operation_authorizations = parse_operation_authorizations(&input.attrs)?;
        let refuse = quote! {
            return Err(::graphql_orm::graphql::errors::OrmPublicError::forbidden().into_graphql_error());
        };
        let enforce = |authorization: &FixedOperationAuthorization| {
            if authorization.template_alternatives().is_some() {
                refuse.clone()
            } else {
                authorization.enforcement_tokens()
            }
        };
        // When the mapped category is not declared but other read categories
        // are, fail closed: require every declared read category's
        // requirement, so a narrower query shape cannot be bypassed.
        let declared_reads = TRAVERSAL_READ_CATEGORIES
            .iter()
            .filter_map(|category| operation_authorizations.get(*category))
            .map(&enforce)
            .collect::<Vec<_>>();
        let scope = |category: &str| match operation_authorizations.get(category) {
            Some(authorization) => enforce(authorization),
            None => quote! { #(#declared_reads)* },
        };
        let list_scope = scope("list");
        let single_scope = scope("single_read");
        Ok(quote! {
            let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?;
            if multiple {
                #list_scope
            } else {
                #single_scope
            }
            Ok(())
        })
    })()
    .unwrap_or_else(|_| {
        quote! {
            let _ = (ctx, multiple);
            Err(::graphql_orm::graphql::errors::OrmPublicError::forbidden().into_graphql_error())
        }
    });
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    quote! {
        impl #impl_generics ::graphql_orm::graphql::orm::RelationTraversalAuthorization
            for #struct_name #type_generics #where_clause
        {
            #[allow(unreachable_code)]
            fn authorize_relation_traversal(
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                multiple: bool,
            ) -> ::graphql_orm::async_graphql::Result<()> {
                #requirement
            }
        }
    }
}

fn parse_operation_authorizations(
    attrs: &[syn::Attribute],
) -> syn::Result<HashMap<String, FixedOperationAuthorization>> {
    let mut authorizations = HashMap::new();

    for attr in attrs {
        if !attr.path().is_ident("graphql_orm") {
            continue;
        }
        let syn::Meta::List(list) = &attr.meta else {
            continue;
        };
        let nested = list.parse_args_with(
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
        )?;

        for meta in nested {
            if !meta.path().is_ident("operation_authorization") {
                continue;
            }
            let syn::Meta::List(operation_authorization) = meta else {
                return Err(syn::Error::new_spanned(
                    meta,
                    "operation_authorization must be declared as operation_authorization(...)",
                ));
            };

            let options = operation_authorization.parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            )?;
            let mut categories = None;
            let mut all_scopes = None;
            let mut any_scopes = None;
            let mut all_scope_templates = None;
            let mut any_scope_templates = None;

            for option in options {
                let option_name = option
                    .path()
                    .get_ident()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "<unknown>".to_string());
                let syn::Meta::NameValue(name_value) = option else {
                    return Err(syn::Error::new_spanned(
                        option,
                        format!("unsupported operation_authorization option `{option_name}`"),
                    ));
                };
                match option_name.as_str() {
                    "categories" => {
                        if categories.is_some() {
                            return Err(syn::Error::new_spanned(
                                name_value,
                                "operation_authorization categories may only be declared once",
                            ));
                        }
                        categories =
                            Some(parse_fixed_scope_array(&name_value.value, "categories")?);
                    }
                    "all_scopes" => {
                        if all_scopes.is_some() {
                            return Err(syn::Error::new_spanned(
                                name_value,
                                "operation_authorization all_scopes may only be declared once",
                            ));
                        }
                        let scopes = parse_fixed_scope_array(&name_value.value, "all_scopes")?;
                        if scopes.is_empty() {
                            return Err(syn::Error::new_spanned(
                                name_value,
                                "operation_authorization all_scopes must not be empty",
                            ));
                        }
                        for scope in &scopes {
                            validate_fixed_scope(scope)?;
                        }
                        all_scopes = Some(scopes);
                    }
                    "any_scopes" => {
                        if any_scopes.is_some() {
                            return Err(syn::Error::new_spanned(
                                name_value,
                                "operation_authorization any_scopes may only be declared once",
                            ));
                        }
                        let syn::Expr::Array(alternatives) = &name_value.value else {
                            return Err(syn::Error::new_spanned(
                                name_value.value,
                                "operation_authorization any_scopes must be an array of nonempty scope arrays",
                            ));
                        };
                        let alternatives = alternatives
                            .elems
                            .iter()
                            .map(|alternative| {
                                let scopes = parse_fixed_scope_array(
                                    alternative,
                                    "any_scopes alternatives",
                                )?;
                                if scopes.is_empty() {
                                    return Err(syn::Error::new_spanned(
                                        alternative,
                                        "operation_authorization any_scopes alternatives must not be empty",
                                    ));
                                }
                                for scope in &scopes {
                                    validate_fixed_scope(scope)?;
                                }
                                Ok(scopes)
                            })
                            .collect::<syn::Result<Vec<_>>>()?;
                        if alternatives.is_empty() {
                            return Err(syn::Error::new_spanned(
                                name_value,
                                "operation_authorization any_scopes must contain at least one nonempty alternative",
                            ));
                        }
                        any_scopes = Some(alternatives);
                    }
                    "all_scope_templates" => {
                        if all_scope_templates.is_some() {
                            return Err(syn::Error::new_spanned(
                                name_value,
                                "operation_authorization all_scope_templates may only be declared once",
                            ));
                        }
                        let templates =
                            parse_fixed_scope_array(&name_value.value, "all_scope_templates")?;
                        if templates.is_empty() {
                            return Err(syn::Error::new_spanned(
                                name_value,
                                "operation_authorization all_scope_templates must not be empty",
                            ));
                        }
                        for template in &templates {
                            scope_template_references(template)?;
                        }
                        all_scope_templates = Some(templates);
                    }
                    "any_scope_templates" => {
                        if any_scope_templates.is_some() {
                            return Err(syn::Error::new_spanned(
                                name_value,
                                "operation_authorization any_scope_templates may only be declared once",
                            ));
                        }
                        let syn::Expr::Array(alternatives) = &name_value.value else {
                            return Err(syn::Error::new_spanned(
                                name_value.value,
                                "operation_authorization any_scope_templates must be an array of nonempty template arrays",
                            ));
                        };
                        let alternatives = alternatives
                            .elems
                            .iter()
                            .map(|alternative| {
                                let templates = parse_fixed_scope_array(
                                    alternative,
                                    "any_scope_templates alternatives",
                                )?;
                                if templates.is_empty() {
                                    return Err(syn::Error::new_spanned(
                                        alternative,
                                        "operation_authorization any_scope_templates alternatives must not be empty",
                                    ));
                                }
                                for template in &templates {
                                    scope_template_references(template)?;
                                }
                                Ok(templates)
                            })
                            .collect::<syn::Result<Vec<_>>>()?;
                        if alternatives.is_empty() {
                            return Err(syn::Error::new_spanned(
                                name_value,
                                "operation_authorization any_scope_templates must contain at least one nonempty alternative",
                            ));
                        }
                        any_scope_templates = Some(alternatives);
                    }
                    _ => {
                        return Err(syn::Error::new_spanned(
                            name_value,
                            format!("unsupported operation_authorization option `{option_name}`"),
                        ));
                    }
                }
            }

            let categories = categories.ok_or_else(|| {
                syn::Error::new_spanned(
                    &operation_authorization,
                    "operation_authorization requires a nonempty categories array",
                )
            })?;
            if categories.is_empty() {
                return Err(syn::Error::new_spanned(
                    &operation_authorization,
                    "operation_authorization categories must not be empty",
                ));
            }
            let authorization = match (
                all_scopes,
                any_scopes,
                all_scope_templates,
                any_scope_templates,
            ) {
                (Some(scopes), None, None, None) => FixedOperationAuthorization::AllScopes(scopes),
                (None, Some(alternatives), None, None) => {
                    FixedOperationAuthorization::AnyScopes(alternatives)
                }
                (None, None, Some(templates), None) => {
                    FixedOperationAuthorization::AllScopeTemplates(templates)
                }
                (None, None, None, Some(alternatives)) => {
                    FixedOperationAuthorization::AnyScopeTemplates(alternatives)
                }
                (None, None, None, None) => {
                    return Err(syn::Error::new_spanned(
                        &operation_authorization,
                        "operation_authorization requires one scope or scope-template mode",
                    ));
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        &operation_authorization,
                        "operation_authorization must declare exactly one of all_scopes, any_scopes, all_scope_templates, or any_scope_templates",
                    ));
                }
            };

            for category in categories {
                let category_value = category.value();
                if !matches!(
                    category_value.as_str(),
                    "create"
                        | "delete"
                        | "delete_many"
                        | "keyset_list"
                        | "list"
                        | "search"
                        | "single_read"
                        | "subscription"
                        | "update"
                        | "update_many"
                        | "upsert"
                ) {
                    return Err(syn::Error::new_spanned(
                        category,
                        "unsupported operation authorization category; expected a generated GraphQL operation category",
                    ));
                }
                if authorizations
                    .insert(category_value.clone(), authorization.clone())
                    .is_some()
                {
                    return Err(syn::Error::new_spanned(
                        category,
                        format!("duplicate operation authorization category `{category_value}`"),
                    ));
                }
            }
        }
    }

    Ok(authorizations)
}

#[derive(Clone)]
struct ScopeTemplateArgumentSpec {
    graphql_name: String,
    rust_ident: Option<syn::Ident>,
    rust_type: Option<syn::Type>,
}

impl ScopeTemplateArgumentSpec {
    fn scalar(
        graphql_name: impl Into<String>,
        rust_ident: syn::Ident,
        rust_type: syn::Type,
    ) -> Self {
        Self {
            graphql_name: graphql_name.into(),
            rust_ident: Some(rust_ident),
            rust_type: Some(rust_type),
        }
    }

    fn complex(graphql_name: impl Into<String>) -> Self {
        Self {
            graphql_name: graphql_name.into(),
            rust_ident: None,
            rust_type: None,
        }
    }
}

fn is_supported_scope_template_scalar(ty: &syn::Type) -> bool {
    if is_option_type(ty) || is_vec_type(ty) {
        return false;
    }
    type_path_last_ident(ty).is_some_and(|ident| {
        matches!(
            ident.to_string().as_str(),
            "String"
                | "Uuid"
                | "bool"
                | "f32"
                | "f64"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "isize"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "usize"
        )
    })
}

fn scope_enforcement_tokens(
    category: &str,
    authorization: Option<&FixedOperationAuthorization>,
    arguments: &[ScopeTemplateArgumentSpec],
) -> syn::Result<proc_macro2::TokenStream> {
    let Some(authorization) = authorization else {
        return Ok(quote! {});
    };
    let Some(alternatives) = authorization.template_alternatives() else {
        return Ok(authorization.enforcement_tokens());
    };

    let mut referenced_arguments: Vec<&ScopeTemplateArgumentSpec> = Vec::new();
    for template in alternatives.iter().flat_map(|alternative| *alternative) {
        for reference in scope_template_references(template)? {
            let Some(argument) = arguments
                .iter()
                .find(|argument| argument.graphql_name == reference)
            else {
                return Err(syn::Error::new_spanned(
                    template,
                    format!(
                        "operation authorization category `{category}` scope template references unknown GraphQL argument `{reference}`"
                    ),
                ));
            };
            let Some(rust_type) = argument.rust_type.as_ref() else {
                return Err(syn::Error::new_spanned(
                    template,
                    format!(
                        "operation authorization category `{category}` scope template argument `{reference}` is a complex input; only non-null scalar arguments are supported"
                    ),
                ));
            };
            if !is_supported_scope_template_scalar(rust_type) {
                return Err(syn::Error::new_spanned(
                    rust_type,
                    format!(
                        "operation authorization category `{category}` scope template argument `{reference}` must be String, UUID, Boolean, integer, or float"
                    ),
                ));
            }
            if !referenced_arguments
                .iter()
                .any(|existing| existing.graphql_name == argument.graphql_name)
            {
                referenced_arguments.push(argument);
            }
        }
    }

    let value_bindings = referenced_arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| {
            let value_ident = quote::format_ident!("__gom_scope_argument_{index}");
            let rust_ident = argument
                .rust_ident
                .as_ref()
                .expect("scalar template arguments have Rust identifiers");
            quote! {
                let #value_ident = ::std::string::ToString::to_string(&#rust_ident);
            }
        })
        .collect::<Vec<_>>();
    let argument_values = referenced_arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| {
            let value_ident = quote::format_ident!("__gom_scope_argument_{index}");
            let graphql_name = &argument.graphql_name;
            quote! { (#graphql_name, #value_ident.as_str()) }
        })
        .collect::<Vec<_>>();
    let alternatives = alternatives.into_iter().map(|alternative| {
        quote! { &[#(#alternative),*] }
    });

    Ok(quote! {
        #(#value_bindings)*
        let _scope_subject =
            ::graphql_orm::graphql::auth::enforce_resolver_scope_templates(
                ctx,
                &[#(#alternatives),*],
                &[#(#argument_values),*],
            )?;
    })
}

#[derive(Copy, Clone)]
enum PropagationValueKind {
    String,
    Uuid,
    Int,
    Float,
    Bool,
}

fn classify_propagation_value_type(ty: &syn::Type) -> Option<(PropagationValueKind, bool)> {
    let mut current = ty;
    let mut is_option = false;

    loop {
        match current {
            syn::Type::Path(type_path) => {
                let segment = type_path.path.segments.last()?;
                let name = segment.ident.to_string();
                if name == "Option" {
                    if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                        if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                            current = inner;
                            is_option = true;
                            continue;
                        }
                    }
                    return None;
                }

                let kind = match name.as_str() {
                    "String" => PropagationValueKind::String,
                    "Uuid" => PropagationValueKind::Uuid,
                    "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64"
                    | "usize" => PropagationValueKind::Int,
                    "f32" | "f64" => PropagationValueKind::Float,
                    "bool" => PropagationValueKind::Bool,
                    _ => return None,
                };
                return Some((kind, is_option));
            }
            _ => return None,
        }
    }
}

struct PropagatedRelation {
    graphql_name: String,
    source_fields: Vec<(syn::Ident, syn::Type, FieldMetadata)>,
    target_fields: Vec<String>,
    target_type: syn::Ident,
}

#[derive(Clone)]
struct ResolvedUpsertField {
    field_name: syn::Ident,
    field_type: syn::Type,
    meta: FieldMetadata,
    rust_name: String,
    db_column: String,
    graphql_create_visible: bool,
}

struct ResolvedUpsertConfig {
    fields: Vec<ResolvedUpsertField>,
}

#[derive(Clone)]
pub(crate) struct PrimaryKeyField {
    pub(crate) field_name: syn::Ident,
    pub(crate) field_type: syn::Type,
    pub(crate) meta: FieldMetadata,
    pub(crate) rust_name: String,
    pub(crate) graphql_name: String,
}

fn create_input_visibility(meta: &FieldMetadata, rust_name: &str, auto_generated_pk: bool) -> bool {
    let is_timestamp = meta.timestamp_behavior(rust_name).exclude_input;
    let include_in_create =
        (!meta.is_primary_key || !auto_generated_pk) && !is_timestamp && meta.write;
    include_in_create && (!meta.skip_input || meta.input_only)
}

pub(crate) fn collect_primary_key_fields(
    struct_name: &syn::Ident,
    fields: &syn::punctuated::Punctuated<Field, syn::token::Comma>,
    graphql_rename_fields: Option<&str>,
    serde_rename_all: Option<&str>,
) -> syn::Result<Vec<PrimaryKeyField>> {
    let mut primary_keys = Vec::new();
    let mut fallback_id: Option<PrimaryKeyField> = None;

    for field in fields {
        let Some(field_name) = field.ident.clone() else {
            continue;
        };
        let meta = parse_field_metadata(field)?;
        if meta.is_relation || meta.skip_db {
            continue;
        }

        let rust_name = field_name.to_string();
        let graphql_name =
            graphql_field_name(&meta, &rust_name, graphql_rename_fields, serde_rename_all);
        let key = PrimaryKeyField {
            field_name,
            field_type: field.ty.clone(),
            meta: meta.clone(),
            rust_name,
            graphql_name,
        };

        if meta.is_primary_key {
            primary_keys.push(key);
        } else if key.rust_name == "id" {
            fallback_id = Some(key);
        }
    }

    if primary_keys.is_empty() {
        if let Some(fallback_id) = fallback_id {
            primary_keys.push(fallback_id);
        } else {
            primary_keys.push(PrimaryKeyField {
                field_name: syn::Ident::new("id", struct_name.span()),
                field_type: syn::parse_quote!(String),
                meta: FieldMetadata::default(),
                rust_name: "id".to_string(),
                graphql_name: "id".to_string(),
            });
        }
    }

    Ok(primary_keys)
}

fn resolve_schema_column_name(name: &str, field_columns: &HashMap<String, String>) -> String {
    field_columns
        .get(name)
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

fn resolve_upsert_config(
    struct_name: &syn::Ident,
    fields: &syn::punctuated::Punctuated<Field, syn::token::Comma>,
    entity_meta: &crate::entity::EntityMetadata,
    backend: BackendKind,
    auto_generated_pk: bool,
) -> syn::Result<Option<ResolvedUpsertConfig>> {
    let Some(requested_targets) = entity_meta.upsert.as_ref() else {
        return Ok(None);
    };

    if backend == BackendKind::Mssql
        && entity_meta.schema_policy.as_deref() != Some("external_writable")
    {
        return Err(syn::Error::new(
            struct_name.span(),
            "graphql-orm upsert is only supported for sqlite and postgres in v1",
        ));
    }

    #[allow(unreachable_code)]
    {
        let mut field_columns = HashMap::new();
        let mut resolved_fields = Vec::new();

        for field in fields {
            let field_name = field.ident.clone().expect("named field");
            let meta = parse_field_metadata(field)?;
            let rust_name = field_name.to_string();
            let db_column = meta.db_column.clone().unwrap_or_else(|| rust_name.clone());
            field_columns.insert(rust_name.clone(), db_column.clone());
            field_columns.insert(db_column.clone(), db_column.clone());
            resolved_fields.push(ResolvedUpsertField {
                field_name,
                field_type: field.ty.clone(),
                meta,
                rust_name,
                db_column,
                graphql_create_visible: false,
            });
        }

        for field in &mut resolved_fields {
            field.graphql_create_visible =
                create_input_visibility(&field.meta, &field.rust_name, auto_generated_pk);
        }

        let mut target_fields = Vec::new();
        for requested in requested_targets {
            let resolved = resolved_fields
                .iter()
                .find(|field| field.rust_name == *requested)
                .or_else(|| {
                    resolved_fields
                        .iter()
                        .find(|field| field.db_column == *requested)
                })
                .ok_or_else(|| {
                    syn::Error::new(
                        struct_name.span(),
                        format!(
                            "graphql_entity upsert target '{}' does not match any persisted field",
                            requested
                        ),
                    )
                })?;

            if resolved.meta.is_relation || resolved.meta.skip_db {
                return Err(syn::Error::new(
                    struct_name.span(),
                    format!(
                        "graphql_entity upsert target '{}' must reference a persisted scalar field",
                        requested
                    ),
                ));
            }

            target_fields.push(resolved.clone());
        }

        // The whole primary key (every column, in order) is a unique target;
        // without `#[primary_key]`, the implicit `id` field's column is.
        let mut primary_key = resolved_fields
            .iter()
            .filter(|field| field.meta.is_primary_key)
            .map(|field| field.db_column.clone())
            .collect::<Vec<_>>();
        if primary_key.is_empty() {
            primary_key.push(
                resolved_fields
                    .iter()
                    .find(|field| {
                        field.rust_name == "id" && !field.meta.is_relation && !field.meta.skip_db
                    })
                    .map_or_else(|| "id".to_string(), |field| field.db_column.clone()),
            );
        }
        let target_columns = target_fields
            .iter()
            .map(|field| field.db_column.clone())
            .collect::<Vec<_>>();

        let mut unique_targets = vec![primary_key];
        unique_targets.extend(
            resolved_fields
                .iter()
                .filter(|field| field.meta.unique)
                .map(|field| vec![field.db_column.clone()]),
        );
        unique_targets.extend(entity_meta.unique_composite.iter().map(|columns| {
            columns
                .iter()
                .map(|column| resolve_schema_column_name(column, &field_columns))
                .collect::<Vec<_>>()
        }));
        unique_targets.extend(entity_meta.indexes.iter().filter(|index| index.unique).map(
            |index| {
                index
                    .columns
                    .iter()
                    .map(|column| resolve_schema_column_name(column, &field_columns))
                    .collect::<Vec<_>>()
            },
        ));

        if !unique_targets.contains(&target_columns) {
            return Err(syn::Error::new(
                struct_name.span(),
                format!(
                    "graphql_entity upsert target '{}' must match a unique field, primary key, unique_index, or unique_composite declaration",
                    requested_targets.join(", ")
                ),
            ));
        }

        Ok(Some(ResolvedUpsertConfig {
            fields: target_fields,
        }))
    }
}

fn push_create_input_sql_value_tokens(
    backend: BackendKind,
    struct_name: &syn::Ident,
    field_access: proc_macro2::TokenStream,
    field_type: &syn::Type,
    meta: &FieldMetadata,
    error_ty: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    if meta.decimal.is_some() {
        let definition = decimal_definition_tokens(meta);
        if is_option_type(field_type) {
            quote! {
                match #field_access {
                    Some(value) => bind_values.push(
                        ::graphql_orm::graphql::orm::decimal_sql_value::<#error_ty>(value, #definition)?
                    ),
                    None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::DecimalNull(#definition)),
                }
            }
        } else {
            quote! {
                bind_values.push(
                    ::graphql_orm::graphql::orm::decimal_sql_value::<#error_ty>(#field_access, #definition)?
                );
            }
        }
    } else if let Some(spatial) = &meta.spatial {
        if backend == BackendKind::Sqlite {
            let geometry_type = spatial_geometry_type_tokens(
                &spatial.geometry_type,
                proc_macro2::Span::call_site(),
            )
            .expect("spatial metadata was already validated");
            let srid = spatial.srid;
            if is_option_type(field_type) {
                quote! {
                    match &#field_access {
                        Some(value) => bind_values.push(
                            ::graphql_orm::graphql::orm::spatial::canonical_geojson_sql_value(
                                value,
                                ::graphql_orm::graphql::orm::SpatialColumnDef::geometry(#geometry_type, #srid),
                            )
                            .map_err(<#error_ty as ::graphql_orm::graphql::orm::OrmResultError>::from_sqlx_error)?
                        ),
                        None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::JsonNull),
                    }
                }
            } else {
                quote! {
                    bind_values.push(
                        ::graphql_orm::graphql::orm::spatial::canonical_geojson_sql_value(
                            &#field_access,
                            ::graphql_orm::graphql::orm::SpatialColumnDef::geometry(#geometry_type, #srid),
                        )
                        .map_err(<#error_ty as ::graphql_orm::graphql::orm::OrmResultError>::from_sqlx_error)?
                    );
                }
            }
        } else if is_option_type(field_type) {
            quote! {
                match &#field_access {
                    Some(value) => bind_values.push(::graphql_orm::graphql::orm::json_sql_value::<_, #error_ty>(value)?),
                    None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::JsonNull),
                }
            }
        } else {
            quote! {
                bind_values.push(::graphql_orm::graphql::orm::json_sql_value::<_, #error_ty>(&#field_access)?);
            }
        }
    } else if meta.is_boolean_field || is_bool_type(field_type) {
        if is_option_type(field_type) {
            quote! {
                match #field_access {
                    Some(value) => bind_values.push(#struct_name::__gom_bool_sql_value(value)),
                    None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::BoolNull),
                }
            }
        } else {
            quote! {
                bind_values.push(#struct_name::__gom_bool_sql_value(#field_access));
            }
        }
    } else if meta.is_json_field || (is_vec_type(field_type) && !is_byte_vec_type(field_type)) {
        if is_option_type(field_type) {
            quote! {
                match &#field_access {
                    Some(value) => bind_values.push(::graphql_orm::graphql::orm::json_sql_value::<_, #error_ty>(value)?),
                    None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::JsonNull),
                }
            }
        } else {
            quote! {
                bind_values.push(::graphql_orm::graphql::orm::json_sql_value::<_, #error_ty>(&#field_access)?);
            }
        }
    } else if is_uuid_type(field_type) {
        if is_option_type(field_type) {
            quote! {
                match #field_access {
                    Some(value) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(value)),
                    None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::UuidNull),
                }
            }
        } else {
            quote! {
                bind_values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(#field_access));
            }
        }
    } else if option_inner_type(field_type)
        .and_then(type_path_last_ident)
        .is_some_and(|ident| {
            matches!(
                ident.to_string().as_str(),
                "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize"
            )
        })
    {
        quote! {
            match #field_access {
                Some(value) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Int(value as i64)),
                None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::IntNull),
            }
        }
    } else if option_inner_type(field_type)
        .and_then(type_path_last_ident)
        .is_some_and(|ident| matches!(ident.to_string().as_str(), "f32" | "f64"))
    {
        quote! {
            match #field_access {
                Some(value) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Float(value.into())),
                None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::FloatNull),
            }
        }
    } else if type_path_last_ident(field_type).is_some_and(|ident| {
        matches!(
            ident.to_string().as_str(),
            "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize"
        )
    }) {
        quote! {
            bind_values.push(::graphql_orm::graphql::orm::SqlValue::Int((#field_access) as i64));
        }
    } else if type_path_last_ident(field_type)
        .is_some_and(|ident| matches!(ident.to_string().as_str(), "f32" | "f64"))
    {
        quote! {
            bind_values.push(::graphql_orm::graphql::orm::SqlValue::Float((#field_access).into()));
        }
    } else if is_byte_vec_type(field_type) {
        quote! {
            bind_values.push(::graphql_orm::graphql::orm::SqlValue::Bytes((#field_access).clone()));
        }
    } else if is_option_type(field_type)
        && option_inner_type(field_type).is_some_and(is_byte_vec_type)
    {
        quote! {
            match &#field_access {
                Some(value) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Bytes(value.clone())),
                None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::BytesNull),
            }
        }
    } else if is_option_type(field_type) {
        let value_expr = quote! { value.to_string() };

        quote! {
            match &#field_access {
                Some(value) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr)),
                None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::StringNull),
            }
        }
    } else {
        let value_expr = quote! { (#field_access).to_string() };

        quote! {
            bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr));
        }
    }
}

fn generated_resolver_schema_signature(
    struct_name: &syn::Ident,
    fields: &syn::punctuated::Punctuated<Field, syn::token::Comma>,
    entity_meta: &crate::entity::EntityMetadata,
    backend: BackendKind,
    graphql_rename_fields: Option<&str>,
    serde_rename_all: Option<&str>,
    auto_generated_pk: bool,
) -> syn::Result<String> {
    let mut field_signatures = Vec::new();
    for field in fields {
        let field_name = field.ident.as_ref().expect("named fields validated");
        let rust_name = field_name.to_string();
        let meta = parse_field_metadata(field)?;
        let graphql_name =
            graphql_field_name(&meta, &rust_name, graphql_rename_fields, serde_rename_all);
        let field_type = &field.ty;
        let rust_type = quote! { #field_type }.to_string();
        let is_timestamp = meta.timestamp_behavior(&rust_name).exclude_input;
        let create_visible = !meta.is_relation
            && !meta.skip_db
            && (!meta.is_primary_key || !auto_generated_pk)
            && !is_timestamp
            && meta.write
            && (!meta.skip_input || meta.input_only);
        let update_visible = !meta.is_relation
            && !meta.skip_db
            && !meta.is_primary_key
            && !is_timestamp
            && meta.write
            && (!meta.skip_input || meta.input_only);
        let output_visible = meta.graphql_readable();
        let search = meta
            .search
            .as_ref()
            .map(|search| format!("{}:{:?}:{:?}", search.weight, search.alias, search.policy));
        let search_json = meta
            .search_json
            .iter()
            .map(|search| format!("{}:{}:{:?}", search.path, search.weight, search.policy))
            .collect::<Vec<_>>();
        let search_relation = meta.search_relation.as_ref().map(|search| {
            format!(
                "{:?}:{}:{}:{:?}:{}",
                search.fields,
                search.weight,
                search.max_items,
                search.policy,
                search.propagate_change
            )
        });
        field_signatures.push(format!(
            concat!(
                "field graphql={:?} rust={:?} type={:?}",
                " output={} create={} update={}",
                " relation={} relation_target={:?} relation_multiple={} persisted={}",
                " primary_key={} private={} sensitive={} json={} spatial={}",
                " filter={} filterable={:?} order={} sortable={} subscribe={}",
                " read_policy={:?} write_policy={:?}",
                " search={:?} search_json={:?}",
                " search_relation={:?}"
            ),
            graphql_name,
            rust_name,
            rust_type,
            output_visible,
            create_visible,
            update_visible,
            meta.is_relation,
            meta.relation_target,
            meta.relation_multiple,
            !meta.skip_db,
            meta.is_primary_key,
            meta.is_private,
            meta.sensitive,
            meta.is_json_field,
            meta.spatial.is_some(),
            meta.filter,
            meta.filterable,
            meta.order,
            meta.sortable,
            meta.subscribe,
            meta.read_policy,
            meta.write_policy,
            search,
            search_json,
            search_relation,
        ));
    }
    field_signatures.sort();

    let mut signature = format!(
        concat!(
            "entity={:?}\nbackend={:?}\ntable={:?}\nplural={:?}\n",
            "schema_policy={:?}\nauth={:?}\nappend_only={}\n",
            "keyset={:?}\nupsert={:?}\nsearch={}\nfield_case={:?}\n"
        ),
        struct_name.to_string(),
        backend.name(),
        entity_meta.table_name.as_deref().unwrap_or("unknown"),
        entity_meta
            .plural_name
            .clone()
            .unwrap_or_else(|| format!("{}s", struct_name)),
        entity_meta.schema_policy,
        entity_meta.auth,
        entity_meta.append_only,
        entity_meta.keyset,
        entity_meta.upsert,
        entity_meta.search.is_some(),
        selected_field_case_rule(),
    );
    if let Some(shapes) = &entity_meta.query_shapes {
        signature.push_str(&format!("query_shapes={shapes:?}\n"));
    }
    if let Some(shapes) = &entity_meta.mutation_shapes {
        signature.push_str(&format!("mutation_shapes={shapes:?}\n"));
    }
    for field in field_signatures {
        signature.push_str(&field);
        signature.push('\n');
    }
    Ok(signature)
}

pub(crate) fn generate_graphql_operations(
    input: &DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    let mut generated: syn::File = syn::parse2(generate_graphql_operations_unfiltered(input)?)?;
    // Before dependent-delete and shape filtering remove resolvers, so every
    // runtime-filtered catalogue entry is still described by its resolver.
    crate::catalog_arguments::fill(&mut generated, &input.ident)?;
    let metadata = parse_entity_metadata(&input.attrs)?;
    if metadata.dependent_deletes {
        generated = syn::parse2(super::dependent_deletes::generate(
            input,
            quote! { #generated },
        )?)?;
    }
    if metadata.query_shapes.is_none() && metadata.mutation_shapes.is_none() {
        return Ok(quote! { #generated });
    }
    // Object's skipped methods retain ordinary Rust signatures. Removing a
    // disabled method also removes its GraphQL-only parameter attributes.
    let queries_name = format!("{}Queries", input.ident);
    let mutations_name = format!("{}Mutations", input.ident);
    for item in &mut generated.items {
        let syn::Item::Impl(implementation) = item else {
            continue;
        };
        let syn::Type::Path(target) = implementation.self_ty.as_ref() else {
            continue;
        };
        let Some(target) = target
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
        else {
            continue;
        };
        if let (Some(shapes), true) = (&metadata.query_shapes, target == queries_name) {
            implementation.items.retain(|item| {
                let syn::ImplItem::Fn(method) = item else {
                    return true;
                };
                let shape = match method.sig.ident.to_string().as_str() {
                    "get_by_id" => "single",
                    "list" => "list",
                    "keyset" => "keyset",
                    "authorized_scan" => "scan",
                    "search" => "search",
                    "aggregate" => "aggregate",
                    _ => return true,
                };
                shapes.iter().any(|selected| selected == shape)
            });
        }
        if let (Some(shapes), true) = (&metadata.mutation_shapes, target == mutations_name) {
            // The generated object method names are the selector names, so a
            // selector and its resolver cannot drift apart.
            implementation.items.retain(|item| {
                let syn::ImplItem::Fn(method) = item else {
                    return true;
                };
                let name = method.sig.ident.to_string();
                if !matches!(
                    name.as_str(),
                    "create" | "upsert" | "update" | "update_many" | "delete" | "delete_many"
                ) {
                    return true;
                }
                shapes.contains(&name)
            });
        }
    }
    Ok(quote! { #generated })
}

/// Whether a token stream contains the `?` operator at any depth.
fn contains_question_mark(tokens: proc_macro2::TokenStream) -> bool {
    tokens.into_iter().any(|token| match token {
        proc_macro2::TokenTree::Punct(punct) => punct.as_char() == '?',
        proc_macro2::TokenTree::Group(group) => contains_question_mark(group.stream()),
        _ => false,
    })
}

/// The first of `names` that occurs as a free-standing identifier at any depth.
///
/// A name that directly follows a `.` is a field or method name (`input.db`,
/// a field the entity itself declares) and is not reported, unless it is listed
/// in `also_after_dot` (`.await`). Raw identifiers (`r#return`) never match.
fn first_identifier_in(
    tokens: proc_macro2::TokenStream,
    names: &[&'static str],
    also_after_dot: &[&'static str],
) -> Option<&'static str> {
    let mut after_dot = false;
    for token in tokens {
        let follows_dot = std::mem::replace(
            &mut after_dot,
            matches!(&token, proc_macro2::TokenTree::Punct(punct) if punct.as_char() == '.'),
        );
        let found = match token {
            proc_macro2::TokenTree::Ident(ident) => names
                .iter()
                .copied()
                .find(|name| ident == name)
                .filter(|name| !follows_dot || also_after_dot.contains(name)),
            proc_macro2::TokenTree::Group(group) => {
                first_identifier_in(group.stream(), names, also_after_dot)
            }
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// The per-field UPDATE assignment lists are emitted into a synchronous helper
/// that every update path calls after its field write checks. A list entry
/// that awaits, returns, or reaches for the resolver context or the database
/// is a policy call in the wrong list: moved into the helper it would be
/// skipped or reordered, so it is refused here.
fn ensure_pure_update_assignments(
    list_name: &str,
    list: &[proc_macro2::TokenStream],
    span: proc_macro2::Span,
) -> syn::Result<()> {
    for tokens in list {
        if let Some(found) = first_identifier_in(
            tokens.clone(),
            &["await", "return", "ctx", "db"],
            &["await"],
        ) {
            return Err(syn::Error::new(
                span,
                format!(
                    "graphql-orm-macros internal error: `{list_name}` contains `{found}`. \
                     The per-field UPDATE assignment lists must be pure and synchronous, \
                     because they are emitted once into `__gom_push_update_assignments` \
                     and run after the field write checks. Put field policy calls in the \
                     inline policy lists instead: `update_policy_checks`, \
                     `update_runtime_policy_checks`, or the repository field checks \
                     (`authorize_repository_update_fields`)."
                ),
            ));
        }
    }
    Ok(())
}

fn generate_graphql_operations_unfiltered(
    input: &DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;
    let struct_name_str = struct_name.to_string();
    let entity_meta = parse_entity_metadata(&input.attrs)?;
    let operation_authorizations = parse_operation_authorizations(&input.attrs)?;
    let query_shape_enabled = |shape: &str| {
        entity_meta
            .query_shapes
            .as_ref()
            .is_none_or(|shapes| shapes.iter().any(|selected| selected == shape))
    };
    let query_shape_skip = |shape: &str| {
        if query_shape_enabled(shape) {
            quote! {}
        } else {
            quote! { skip, }
        }
    };
    let single_query_skip = query_shape_skip("single");
    let list_query_skip = query_shape_skip("list");
    let keyset_query_skip = query_shape_skip("keyset");
    let scan_query_skip = query_shape_skip("scan");
    let search_query_skip = query_shape_skip("search");
    let aggregate_query_skip = query_shape_skip("aggregate");
    let resolver_auth_mode =
        resolver_auth_mode_tokens(entity_meta.auth.as_deref(), struct_name.span())?;
    let authenticated_directive = match entity_meta.auth.as_deref() {
        Some("required") => quote! {
            directive = ::graphql_orm::graphql::federation::federation_authenticated::apply(),
        },
        _ => quote! {},
    };
    let backend = resolve_backend(
        entity_meta.backend.as_deref(),
        struct_name.span(),
        "graphql_entity",
    )?;
    let schema_policy_read_only = matches!(
        entity_meta.schema_policy.as_deref(),
        Some("external_read_only")
    );
    let backend_writable = backend != BackendKind::Mssql
        || entity_meta.schema_policy.as_deref() == Some("external_writable");
    let backend_marker = backend_marker_tokens(backend);
    let rls_request_guard = quote! {
        ::graphql_orm::graphql::orm::ensure_rls_request_context::<#struct_name, #backend_marker>(
            ctx,
            <#struct_name as ::graphql_orm::graphql::orm::Entity>::entity_name(),
        )?;
    };
    let is_mssql = backend == BackendKind::Mssql;
    let pool_type = backend_pool_type_tokens(backend);

    let undeclared_operation_authorization = match entity_meta.auth.as_deref() {
        Some("required") => quote! {
            ::graphql_orm::graphql::orm::GraphqlAuthorizationRequirement::Authenticated
        },
        Some("none") => quote! {
            ::graphql_orm::graphql::orm::GraphqlAuthorizationRequirement::Public
        },
        Some("optional") => quote! {
            ::graphql_orm::graphql::orm::GraphqlAuthorizationRequirement::SubgraphOnly {
                policy: ::graphql_orm::graphql::orm::GraphqlUnrepresentablePolicy::new(
                    ::graphql_orm::graphql::orm::GraphqlUnrepresentablePolicyCode::Unsupported,
                    "entity auth is optional",
                ),
            }
        },
        None => quote! {
            ::graphql_orm::graphql::orm::GraphqlAuthorizationRequirement::SubgraphOnly {
                policy: ::graphql_orm::graphql::orm::GraphqlUnrepresentablePolicy::new(
                    ::graphql_orm::graphql::orm::GraphqlUnrepresentablePolicyCode::Unsupported,
                    "entity auth is not declared",
                ),
            }
        },
        Some(_) => {
            unreachable!("entity auth is validated before authorization metadata is emitted")
        }
    };
    let authorization_tokens = |category: &str| {
        operation_authorizations.get(category).map_or_else(
            || undeclared_operation_authorization.clone(),
            FixedOperationAuthorization::requirement_tokens,
        )
    };
    let requires_scopes_tokens = |category: &str| {
        operation_authorizations.get(category).map_or_else(
            || quote! {},
            FixedOperationAuthorization::requires_scopes_tokens,
        )
    };
    let list_operation_authorization = authorization_tokens("list");
    let list_requires_scopes = requires_scopes_tokens("list");
    let aggregate_operation_authorization = authorization_tokens("aggregate");
    let aggregate_requires_scopes = requires_scopes_tokens("aggregate");
    let search_operation_authorization = authorization_tokens("search");
    let search_requires_scopes = requires_scopes_tokens("search");
    let keyset_list_operation_authorization = authorization_tokens("keyset_list");
    let keyset_list_requires_scopes = requires_scopes_tokens("keyset_list");
    let single_read_operation_authorization = authorization_tokens("single_read");
    let single_read_requires_scopes = requires_scopes_tokens("single_read");
    let create_operation_authorization = authorization_tokens("create");
    let create_requires_scopes = requires_scopes_tokens("create");
    let upsert_operation_authorization = authorization_tokens("upsert");
    let upsert_requires_scopes = requires_scopes_tokens("upsert");
    let update_operation_authorization = authorization_tokens("update");
    let update_requires_scopes = requires_scopes_tokens("update");
    let update_many_operation_authorization = authorization_tokens("update_many");
    let update_many_requires_scopes = requires_scopes_tokens("update_many");
    let delete_operation_authorization = authorization_tokens("delete");
    let delete_requires_scopes = requires_scopes_tokens("delete");
    let delete_many_operation_authorization = authorization_tokens("delete_many");
    let delete_many_requires_scopes = requires_scopes_tokens("delete_many");
    let subscription_operation_authorization = authorization_tokens("subscription");
    let subscription_requires_scopes = operation_authorizations.get("subscription").map_or_else(
        || quote! {},
        FixedOperationAuthorization::federation_requires_scopes_directive_tokens,
    );

    let data = match &input.data {
        Data::Struct(data) => data,
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "GraphQLOperations can only be derived for structs",
            ));
        }
    };

    let fields = match &data.fields {
        Fields::Named(fields) => &fields.named,
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "GraphQLOperations requires named fields",
            ));
        }
    };
    crate::entity::validate_host_timestamp_fields(fields, !entity_meta.schema_only)?;

    let graphql_rename_fields = entity_meta.graphql_rename_fields.as_deref();
    let serde_rename_all = entity_meta.serde_rename_all.as_deref();
    let table_name = entity_meta.table_name.as_deref().unwrap_or("unknown");
    // Generated DML names the table with the backend's rendered identifier;
    // `table_name` itself stays the raw identity used by hooks and descriptors.
    let sql_table_name = backend_render_identifier_path(backend, table_name);
    let sql_dialect = backend_dialect_tokens(backend);
    let plural_name = entity_meta
        .plural_name
        .clone()
        .unwrap_or_else(|| format!("{}s", struct_name));

    // Generate optional post-mutation hook code if a hook path is configured.
    //
    // Expected signature:
    // `async fn(&::graphql_orm::async_graphql::Context<'_>, &#pool_type, &#Entity, ChangeAction)
    //      -> ::graphql_orm::async_graphql::Result<()>`
    let notify_handler_path = if let Some(ref notify_handler) = entity_meta.notify_handler {
        Some(syn::parse_str::<syn::Path>(notify_handler).map_err(|_| {
            syn::Error::new(
                struct_name.span(),
                "graphql_entity notify/notify_with must be a valid Rust path string",
            )
        })?)
    } else {
        None
    };
    let notify_single = |action: proc_macro2::TokenStream| {
        if let Some(ref handler) = notify_handler_path {
            if entity_meta.notify_report_errors {
                quote! {
                    if let Err(error) = #handler(ctx, pool, &entity, #action).await {
                        db.report_post_commit_error(error.message).await;
                    }
                }
            } else {
                quote! { #handler(ctx, pool, &entity, #action).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?; }
            }
        } else {
            quote! {}
        }
    };
    let notify_on_created =
        notify_single(quote! { ::graphql_orm::graphql::orm::ChangeAction::Created });
    let notify_on_updated =
        notify_single(quote! { ::graphql_orm::graphql::orm::ChangeAction::Updated });
    let notify_on_deleted =
        notify_single(quote! { ::graphql_orm::graphql::orm::ChangeAction::Deleted });
    let has_notify_handler = notify_handler_path.is_some();
    let notify_bulk = |action: proc_macro2::TokenStream| {
        if let Some(ref handler) = notify_handler_path {
            quote! {
                for entity in &notification_rows {
                    if let Err(error) = #handler(ctx, db.pool(), entity, #action).await {
                        db.report_post_commit_error(error.message.clone()).await;
                        let safe_error = ::graphql_orm::graphql::errors::graphql_error_from_sqlx(
                            ::graphql_orm::graphql::errors::sqlx_error_from_graphql(error));
                        result.notification_errors.get_or_insert_with(Vec::new).push(
                            ::graphql_orm::graphql::orm::NotificationError::from_graphql_error(safe_error));
                    }
                }
            }
        } else {
            quote! {}
        }
    };
    let notify_bulk_updated =
        notify_bulk(quote! { ::graphql_orm::graphql::orm::ChangeAction::Updated });
    let notify_bulk_deleted =
        notify_bulk(quote! { ::graphql_orm::graphql::orm::ChangeAction::Deleted });
    let entity_name_lit = struct_name_str.clone();

    if entity_meta.repository_mutations {
        let declared_keys = fields
            .iter()
            .filter_map(|field| {
                parse_field_metadata(field)
                    .ok()
                    .filter(|metadata| metadata.is_primary_key)
                    .map(|metadata| (field, metadata))
            })
            .collect::<Vec<_>>();
        if declared_keys.len() < 2 {
            return Err(syn::Error::new_spanned(
                struct_name,
                "repository_mutations requires a complete declared composite primary key",
            ));
        }
        for (field, metadata) in declared_keys {
            if metadata.is_relation || metadata.skip_db {
                return Err(syn::Error::new_spanned(
                    field,
                    "repository mutation primary-key fields must be persisted scalar fields, not relations or skipped fields",
                ));
            }
        }
    }
    let primary_key_fields =
        collect_primary_key_fields(struct_name, fields, graphql_rename_fields, serde_rename_all)?;
    if !crate::entity::has_repository_entity_attribute(&input.attrs) {
        for key in &primary_key_fields {
            if !key.meta.graphql_readable() {
                return Err(syn::Error::new_spanned(
                    &key.field_name,
                    "generated Changed events disclose primary keys; remove private/input_only and set read = true on every primary-key field, or omit GraphQLOperations/use RepositoryEntity for private persistence",
                ));
            }
        }
    }
    let has_composite_primary_key = primary_key_fields.len() > 1;
    if entity_meta.append_only && schema_policy_read_only {
        return Err(syn::Error::new_spanned(
            struct_name,
            "append_only cannot be combined with a read-only schema policy; creates require a writable schema policy",
        ));
    }
    if entity_meta.append_only && has_composite_primary_key {
        for key in &primary_key_fields {
            if is_option_type(&key.field_type)
                || !key.meta.write
                || key.meta.auto_generated == Some(true)
            {
                return Err(syn::Error::new_spanned(
                    &key.field_name,
                    "append_only composite primary keys require non-nullable, host-supplied writable components (auto_generated = false)",
                ));
            }
        }
    }
    let composite_repository_mutations =
        has_composite_primary_key && entity_meta.repository_mutations;
    if entity_meta.retention_policy.is_some() && (!backend_writable || schema_policy_read_only) {
        return Err(syn::Error::new_spanned(
            struct_name,
            "retention_purge requires a managed writable sqlite or postgres entity",
        ));
    }
    if entity_meta.retention_policy.is_some() {
        for field in fields {
            let field_meta = parse_field_metadata(field)?;
            if field_meta.is_relation
                && field_meta.relation_target.as_deref() == Some(struct_name_str.as_str())
                && field_meta.relation_on_delete.as_deref() == Some("cascade")
                && field_meta.relation_emit_foreign_key != Some(false)
            {
                return Err(syn::Error::new_spanned(
                    field,
                    "retention_purge does not support a self-referential ON DELETE CASCADE because it could exceed the explicit row limit",
                ));
            }
        }
    }
    if entity_meta.repository_mutations && !has_composite_primary_key {
        return Err(syn::Error::new_spanned(
            struct_name,
            "repository_mutations is only needed for composite-primary-key entities",
        ));
    }
    if composite_repository_mutations {
        if !backend_writable {
            return Err(syn::Error::new_spanned(
                struct_name,
                "MSSQL repository_mutations require schema_policy = \"external_writable\"",
            ));
        }
        if schema_policy_read_only || entity_meta.append_only {
            return Err(syn::Error::new_spanned(
                struct_name,
                "repository_mutations conflict with read-only or append-only entity mode",
            ));
        }
        for key in &primary_key_fields {
            if is_option_type(&key.field_type) {
                return Err(syn::Error::new_spanned(
                    &key.field_type,
                    "repository mutation primary-key fields must be non-nullable",
                ));
            }
            if !key.meta.write || key.meta.auto_generated == Some(true) {
                return Err(syn::Error::new_spanned(
                    &key.field_name,
                    "repository mutation primary-key fields must be host-supplied writable fields",
                ));
            }
        }
    }
    let first_primary_key = primary_key_fields
        .first()
        .expect("primary key collection includes fallback id");
    let pk_field = first_primary_key.field_name.clone();
    let pk_column = first_primary_key
        .meta
        .db_column
        .as_deref()
        .unwrap_or(&first_primary_key.rust_name);
    let pk_type_ty: syn::Type = first_primary_key.field_type.clone();
    let pk_type = quote! { #pk_type_ty };
    let auto_generated_pk = !(has_composite_primary_key && entity_meta.append_only)
        && first_primary_key
            .meta
            .auto_generated
            .unwrap_or_else(|| pk_field == syn::Ident::new("id", pk_field.span()));
    if composite_repository_mutations && auto_generated_pk {
        return Err(syn::Error::new_spanned(
            &first_primary_key.field_name,
            "repository mutation composite keys cannot contain an auto-generated primary field; set auto_generated = false",
        ));
    }
    let version_fields = fields
        .iter()
        .filter_map(|field| {
            parse_field_metadata(field)
                .ok()
                .filter(|meta| meta.is_version)
                .map(|meta| (field, meta))
        })
        .collect::<Vec<_>>();
    if version_fields.len() > 1 {
        return Err(syn::Error::new_spanned(
            struct_name,
            "only one #[graphql_orm(version)] field is allowed per entity",
        ));
    }
    let version_field = version_fields.first().map(|(field, meta)| {
        let ident = field.ident.clone().expect("named entity field");
        let rust_name = ident.to_string();
        let column = meta.db_column.clone().unwrap_or(rust_name);
        (ident, field.ty.clone(), column)
    });
    let managed_version_field = version_fields
        .first()
        .filter(|(_, meta)| meta.managed_version)
        .and_then(|_| version_field.clone());
    let expected_version_arg_name =
        apply_graphql_case("expected_version", selected_argument_case());
    let (expected_version_param, expected_version_binding) = if managed_version_field.is_some() {
        (
            quote! {
                #[graphql(
                    name = #expected_version_arg_name,
                    desc = "Apply only if the record's current managed version equals this value; otherwise fail with CONFLICT and write nothing"
                )]
                expected_version: Option<i64>,
            },
            quote! { expected_version },
        )
    } else {
        (quote! {}, quote! { None })
    };
    let managed_version_column_option = match &managed_version_field {
        Some((_, _, column)) => quote! { Some(#column) },
        None => quote! { None },
    };
    // Managed versions advance on every generated write. CAS-only versions (bare
    // `version`) keep their historical behaviour: only compare-and-swap advances them.
    let managed_version_helper_methods = if let Some((version_ident, _, version_column)) =
        &managed_version_field
    {
        quote! {
            #[doc(hidden)]
            pub(crate) fn __gom_managed_version_assignment() -> Option<String> {
                use ::graphql_orm::graphql::orm::{OrmBackend, SqlDialect};
                let dialect = <#backend_marker as OrmBackend>::DIALECT;
                let column = dialect.quote_identifier_path(#version_column);
                Some(format!("{column} = {column} + 1"))
            }

            #[doc(hidden)]
            pub(crate) fn __gom_managed_version_guard() -> String {
                use ::graphql_orm::graphql::orm::{OrmBackend, SqlDialect};
                let dialect = <#backend_marker as OrmBackend>::DIALECT;
                format!(" AND {} < {}", dialect.quote_identifier_path(#version_column), i64::MAX)
            }

            #[doc(hidden)]
            pub(crate) fn __gom_managed_version_of(entity: &Self) -> Option<i64> {
                Some(entity.#version_ident)
            }

            #[doc(hidden)]
            pub(crate) fn __gom_managed_version_expectation(expected: Option<i64>) -> String {
                use ::graphql_orm::graphql::orm::{OrmBackend, SqlDialect};
                let dialect = <#backend_marker as OrmBackend>::DIALECT;
                // An i64 literal cannot carry SQL; it keeps the bind order unchanged.
                expected
                    .map(|expected| format!(" AND {} = {expected}", dialect.quote_identifier_path(#version_column)))
                    .unwrap_or_default()
            }

            #[doc(hidden)]
            pub(crate) fn __gom_require_managed_version_headroom(entity: &Self) -> ::graphql_orm::Result<()> {
                if entity.#version_ident >= i64::MAX {
                    return Err(::graphql_orm::sqlx::Error::Protocol("managed version cannot advance past i64::MAX".to_string()));
                }
                Ok(())
            }

            /// An upsert's update branch is guarded by `version < i64::MAX`, which
            /// SQLite and PostgreSQL treat as "update nothing" rather than an error.
            /// Require the committed row to have advanced exactly once.
            #[doc(hidden)]
            pub(crate) fn __gom_require_managed_version_advanced(before: Option<&Self>, after: &Self) -> ::graphql_orm::Result<()> {
                if let Some(before) = before
                    && before.#version_ident.checked_add(1) != Some(after.#version_ident)
                {
                    return Err(::graphql_orm::sqlx::Error::Protocol("managed version did not advance".to_string()));
                }
                Ok(())
            }
        }
    } else {
        quote! {
            #[doc(hidden)]
            pub(crate) fn __gom_managed_version_assignment() -> Option<String> {
                None
            }

            #[doc(hidden)]
            pub(crate) fn __gom_managed_version_guard() -> String {
                String::new()
            }

            #[doc(hidden)]
            pub(crate) fn __gom_managed_version_of(_entity: &Self) -> Option<i64> {
                None
            }

            #[doc(hidden)]
            pub(crate) fn __gom_managed_version_expectation(_expected: Option<i64>) -> String {
                String::new()
            }

            #[doc(hidden)]
            pub(crate) fn __gom_require_managed_version_headroom(_entity: &Self) -> ::graphql_orm::Result<()> {
                Ok(())
            }

            #[doc(hidden)]
            pub(crate) fn __gom_require_managed_version_advanced(_before: Option<&Self>, _after: &Self) -> ::graphql_orm::Result<()> {
                Ok(())
            }
        }
    };
    if let Some((_, ty, _)) = &version_field {
        if !matches!(ty, syn::Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "i64"))
        {
            return Err(syn::Error::new_spanned(
                ty,
                "#[graphql_orm(version)] currently requires an i64 field",
            ));
        }
    }
    let keyset_parts = if let Some(config) = entity_meta.keyset.as_deref() {
        let mut parts = Vec::new();
        for raw in config.split(',') {
            let tokens = raw.split_whitespace().collect::<Vec<_>>();
            if tokens.len() < 2
                || !matches!(tokens[1].to_ascii_lowercase().as_str(), "asc" | "desc")
            {
                return Err(syn::Error::new_spanned(
                    struct_name,
                    "keyset entries must use `field asc|desc [nulls first|last]`",
                ));
            }
            let field = fields
                .iter()
                .find(|field| field.ident.as_ref().is_some_and(|ident| ident == tokens[0]))
                .ok_or_else(|| {
                    syn::Error::new_spanned(
                        struct_name,
                        format!("keyset references unknown field `{}`", tokens[0]),
                    )
                })?;
            let meta = parse_field_metadata(field)?;
            if meta.is_relation || meta.skip_db {
                return Err(syn::Error::new_spanned(
                    field,
                    "keyset fields must be persisted scalar fields",
                ));
            }
            let nulls_first = match tokens.get(2..4) {
                Some([nulls, first])
                    if nulls.eq_ignore_ascii_case("nulls")
                        && first.eq_ignore_ascii_case("first") =>
                {
                    true
                }
                Some([nulls, last])
                    if nulls.eq_ignore_ascii_case("nulls") && last.eq_ignore_ascii_case("last") =>
                {
                    false
                }
                Some([]) | None => false,
                _ => {
                    return Err(syn::Error::new_spanned(
                        field,
                        "keyset null ordering must be `nulls first` or `nulls last`",
                    ));
                }
            };
            parts.push((
                field.ident.clone().expect("named field"),
                field.ty.clone(),
                meta.db_column
                    .clone()
                    .unwrap_or_else(|| tokens[0].to_string()),
                tokens[1].eq_ignore_ascii_case("asc"),
                nulls_first,
            ));
        }
        let complete_key_suffix = parts.len() >= primary_key_fields.len()
            && parts[parts.len().saturating_sub(primary_key_fields.len())..]
                .iter()
                .zip(&primary_key_fields)
                .all(|((field, _, _, _, _), key)| field == &key.field_name);
        if !complete_key_suffix {
            return Err(syn::Error::new_spanned(
                struct_name,
                if has_composite_primary_key {
                    "keyset order must end with every primary-key field in declaration order as a unique tiebreaker"
                } else {
                    "keyset order must end with the primary key as a unique tiebreaker"
                },
            ));
        }
        if has_composite_primary_key {
            let mut ordered_fields = std::collections::BTreeSet::new();
            for (field, _, _, _, _) in &parts {
                if !ordered_fields.insert(field.to_string()) {
                    return Err(syn::Error::new_spanned(
                        field,
                        "composite keyset order must not repeat a field",
                    ));
                }
            }
        }
        Some(parts)
    } else {
        None
    };
    let upsert_config = resolve_upsert_config(
        struct_name,
        fields,
        &entity_meta,
        backend,
        auto_generated_pk,
    )?;
    let graphql_upsert_enabled = upsert_config.as_ref().is_some_and(|config| {
        config
            .fields
            .iter()
            .all(|field| field.graphql_create_visible)
    });
    let pk_is_uuid = is_uuid_type(&pk_type_ty);
    let pk_is_bytes = is_byte_vec_type(&pk_type_ty);
    // Single-key write/readback paths use the same typed binder as key-based
    // repository reads. Display/event IDs below remain strings; SQL keys do not.
    let pk_bind_value = quote! {
        #struct_name::__gom_key_values(&id).remove(0)
    };
    let pk_bind_value_ref = quote! {
        #struct_name::__gom_key_values(id).remove(0)
    };
    let pk_bind_value_for_entity = quote! {
        #struct_name::__gom_key_values(&entity.#pk_field).remove(0)
    };
    let created_pk_value = quote! {
        #struct_name::__gom_key_values(&created_pk).remove(0)
    };
    let created_pk_id_string = if pk_is_bytes {
        quote! { ::graphql_orm::graphql::orm::binary_key_id(&created_pk) }
    } else {
        quote! { created_pk.to_string() }
    };
    let pk_id_for_entity = if pk_is_bytes {
        quote! { ::graphql_orm::graphql::orm::binary_key_id(&entity.#pk_field) }
    } else {
        quote! { entity.#pk_field.to_string() }
    };
    let pk_id_for_current = if pk_is_bytes {
        quote! { ::graphql_orm::graphql::orm::binary_key_id(&current_entity.#pk_field) }
    } else {
        quote! { current_entity.#pk_field.to_string() }
    };
    let pk_id_for_id = if pk_is_bytes {
        quote! { ::graphql_orm::graphql::orm::binary_key_id(&id) }
    } else {
        quote! { id.to_string() }
    };

    let mut propagated_relations: Vec<PropagatedRelation> = Vec::new();
    for field in fields {
        let meta = parse_field_metadata(field)?;
        if !meta.is_relation || meta.relation_propagate_change.as_deref() != Some("up") {
            continue;
        }

        let field_name = field.ident.clone().unwrap();
        let rust_name = field_name.to_string();
        let graphql_name =
            graphql_field_name(&meta, &rust_name, graphql_rename_fields, serde_rename_all);
        let source_names = meta.relation_from_fields.clone().unwrap_or_else(|| {
            vec![
                meta.relation_from
                    .clone()
                    .unwrap_or_else(|| pk_field.to_string()),
            ]
        });
        let target_fields = meta
            .relation_to_fields
            .clone()
            .unwrap_or_else(|| vec![meta.relation_to.clone().unwrap_or_else(|| "id".to_string())]);
        if source_names.is_empty() || source_names.len() != target_fields.len() {
            return Err(syn::Error::new_spanned(
                field,
                "propagate_change requires equal nonempty from/to tuples",
            ));
        }
        let mut source_fields = Vec::new();
        for source in source_names {
            let candidate = fields.iter().find(|candidate| candidate.ident.as_ref().is_some_and(|ident| ident == source.as_str()))
                .ok_or_else(|| syn::Error::new_spanned(field, format!("Relation '{rust_name}' references unknown propagation source field '{source}'")))?;
            let metadata = parse_field_metadata(candidate)?;
            if metadata.is_relation
                || metadata.skip_db
                || classify_propagation_value_type(&candidate.ty).is_none()
            {
                return Err(syn::Error::new_spanned(
                    candidate,
                    "propagate_change source fields must be persisted scalar fields",
                ));
            }
            source_fields.push((
                candidate.ident.clone().expect("named field"),
                candidate.ty.clone(),
                metadata,
            ));
        }
        let target_type = syn::Ident::new(
            meta.relation_target.as_deref().unwrap_or("Unknown"),
            field.span(),
        );
        propagated_relations.push(PropagatedRelation {
            graphql_name,
            source_fields,
            target_fields,
            target_type,
        });
    }

    // Generate type names
    let queries_struct = syn::Ident::new(&format!("{}Queries", struct_name), struct_name.span());
    let mutations_struct =
        syn::Ident::new(&format!("{}Mutations", struct_name), struct_name.span());
    let subscriptions_struct =
        syn::Ident::new(&format!("{}Subscriptions", struct_name), struct_name.span());
    let where_input = syn::Ident::new(&format!("{}WhereInput", struct_name), struct_name.span());
    let aggregate_field = syn::Ident::new(
        &format!("{}AggregateField", struct_name),
        struct_name.span(),
    );
    let aggregate_metric_input = syn::Ident::new(
        &format!("{}AggregateMetricInput", struct_name),
        struct_name.span(),
    );
    let order_by_input =
        syn::Ident::new(&format!("{}OrderByInput", struct_name), struct_name.span());
    let create_input = syn::Ident::new(&format!("Create{}Input", struct_name), struct_name.span());
    let update_input = syn::Ident::new(&format!("Update{}Input", struct_name), struct_name.span());
    let graphql_create_input = syn::Ident::new(
        &format!("GraphQLCreate{}Input", struct_name),
        struct_name.span(),
    );
    let graphql_update_input = syn::Ident::new(
        &format!("GraphQLUpdate{}Input", struct_name),
        struct_name.span(),
    );
    let result_type = syn::Ident::new(&format!("{}Result", struct_name), struct_name.span());
    let changed_event =
        syn::Ident::new(&format!("{}ChangedEvent", struct_name), struct_name.span());

    // GraphQL type names remain PascalCase; public fields follow crate naming features.
    let resolver_case = selected_resolver_case();
    let argument_case = selected_argument_case();
    let field_case = selected_field_case();
    let field_case_rule = selected_field_case_rule();
    let list_query_name = apply_graphql_case(&plural_name, resolver_case);
    let aggregate_query_name =
        apply_graphql_case(&format!("{}Aggregate", plural_name), resolver_case);
    let search_query_name = apply_graphql_case(&format!("{}Search", plural_name), resolver_case);
    let single_query_name = apply_graphql_case(&struct_name_str, resolver_case);
    let create_mutation_name = apply_graphql_case(&format!("create{}", struct_name), resolver_case);
    let upsert_mutation_name = apply_graphql_case(&format!("upsert{}", struct_name), resolver_case);
    let update_mutation_name = apply_graphql_case(&format!("update{}", struct_name), resolver_case);
    let update_many_mutation_name =
        apply_graphql_case(&format!("update{}", plural_name), resolver_case);
    let delete_mutation_name = apply_graphql_case(&format!("delete{}", struct_name), resolver_case);
    let delete_many_mutation_name =
        apply_graphql_case(&format!("delete{}", plural_name), resolver_case);
    let subscription_name = apply_graphql_case(&format!("{}Changed", struct_name), resolver_case);
    let keyset_query_name = apply_graphql_case(&format!("{}Keyset", plural_name), resolver_case);
    let entity_result_field_name = apply_graphql_case(&struct_name_str, field_case);
    // Updates and upserts release their committed row through the same check.
    // Dependent-delete rewriting strips this statement, keeping the internal row.
    let mutation_output_release = quote! {
        result.entity = db.__release_graphql_mutation_output(
            ctx,
            #entity_result_field_name,
            result.entity.take(),
            &#struct_name::__gom_key_where_clause(),
            #struct_name::__gom_subscription_key_values,
        ).await?;
    };
    // The written row must itself satisfy the row write policy. Checked inside the
    // transaction before the After hook; a refusal rolls the write back.
    let graphql_post_image_check = quote! {
        hook_ctx.__ensure_written_row(
            Some(ctx),
            #entity_name_lit,
            <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
            ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
            &entity as &(dyn ::std::any::Any + Send + Sync),
        ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
    };
    let repository_post_image_check = quote! {
        hook_ctx.__ensure_written_row(
            None,
            #entity_name_lit,
            <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
            ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
            &entity as &(dyn ::std::any::Any + Send + Sync),
        ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
    };

    let where_arg_name = apply_graphql_case("where", argument_case);
    let search_arg_name = apply_graphql_case("search", argument_case);
    let order_by_arg_name = apply_graphql_case("orderBy", argument_case);
    let page_arg_name = apply_graphql_case("page", argument_case);
    let group_by_arg_name = apply_graphql_case("groupBy", argument_case);
    let metrics_arg_name = apply_graphql_case("metrics", argument_case);
    let group_limit_arg_name = apply_graphql_case("groupLimit", argument_case);
    let id_arg_name = apply_graphql_case("id", argument_case);
    let input_arg_name = apply_graphql_case("input", argument_case);
    let filter_arg_name = apply_graphql_case("filter", argument_case);
    let where_arg_description = semantic_argument_description(&where_arg_name);
    let search_arg_description = semantic_argument_description(&search_arg_name);
    let order_by_arg_description = semantic_argument_description(&order_by_arg_name);
    let page_arg_description = semantic_argument_description(&page_arg_name);
    let group_by_arg_description = semantic_argument_description(&group_by_arg_name);
    let metrics_arg_description = semantic_argument_description(&metrics_arg_name);
    let group_limit_arg_description = semantic_argument_description(&group_limit_arg_name);
    let id_arg_description = semantic_argument_description(&id_arg_name);
    let input_arg_description = semantic_argument_description(&input_arg_name);
    let filter_arg_description = semantic_argument_description(&filter_arg_name);
    let list_operation_description = format!("List {struct_name_str} records");
    let aggregate_operation_description = format!("Aggregate {struct_name_str} records");
    let single_operation_description = format!("Read one {struct_name_str} record");
    let search_operation_description = format!("Search {struct_name_str} records");
    let keyset_operation_description = format!("Page through {struct_name_str} records");
    let scan_operation_description =
        "Bounded forward authorization scan; continuation does not promise another visible row.";
    let create_operation_description = format!("Create one {struct_name_str} record");
    let upsert_operation_description = format!("Create or update one {struct_name_str} record");
    let update_operation_description = format!("Update one {struct_name_str} record");
    let update_many_operation_description = format!("Update matching {struct_name_str} records");
    let delete_operation_description = format!("Delete one {struct_name_str} record");
    let delete_many_operation_description = format!("Delete matching {struct_name_str} records");
    let subscription_operation_description =
        format!("Observe changes to {struct_name_str} records");
    let subscription_operation_description = format!(
        "{} Payload selections support persisted subscribable scalars only; relation/computed/custom selections are rejected. Subscriber authority is rechecked on an event-independent jittered timer: definite denial ends with FORBIDDEN and host failures with SUBSCRIPTION_CONTINUITY_LOST. Per-event denials silently suppress events. Select exactly one root occurrence per operation. Refresh authorized queries for richer data.",
        subscription_operation_description
    );
    let page_info_field_name = apply_graphql_case("pageInfo", field_case);
    let change_kind_field_name = apply_graphql_case("changeKind", field_case);
    let source_entity_field_name = apply_graphql_case("sourceEntity", field_case);
    let source_id_field_name = apply_graphql_case("sourceId", field_case);
    let deleted_count_field_name = apply_graphql_case("deletedCount", field_case);
    let affected_count_field_name = apply_graphql_case("affectedCount", field_case);
    let upsert_result_type =
        syn::Ident::new(&format!("Upsert{}Result", struct_name), struct_name.span());
    let update_many_result_type =
        syn::Ident::new(&format!("Update{}Result", plural_name), struct_name.span());
    let update_many_result_type_str = format!("Update{}Result", plural_name);
    let delete_many_result_type =
        syn::Ident::new(&format!("Delete{}Result", plural_name), struct_name.span());
    let upsert_result_type_str = format!("Upsert{}Result", struct_name);
    let delete_many_result_type_str = format!("Delete{}Result", plural_name);
    let subscription_key_arg_name = apply_graphql_case("key", argument_case);
    let subscription_keys_arg_name = apply_graphql_case("keys", argument_case);
    let subscription_keys_arg_description = format!(
        "Complete primary keys to observe, at most Database::subscription_key_limit (default 500); duplicates are ignored, an empty list matches nothing, and it cannot be combined with {subscription_key_arg_name}"
    );
    let key_type = syn::Ident::new(&format!("{}Key", struct_name), struct_name.span());
    let key_input_name = format!("{}KeyInput", struct_name);
    let key_type_definition = if has_composite_primary_key {
        let fields = primary_key_fields
            .iter()
            .map(|field| {
                let field_name = &field.field_name;
                let field_type = &field.field_type;
                let graphql_name = &field.graphql_name;
                quote! { #[graphql(name = #graphql_name)] pub #field_name: #field_type, }
            })
            .collect::<Vec<_>>();
        quote! {
            #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, ::graphql_orm::async_graphql::SimpleObject, ::graphql_orm::async_graphql::InputObject)]
            #[graphql(input_name = #key_input_name)]
            pub struct #key_type {
                #(#fields)*
            }
        }
    } else {
        quote! {
            pub type #key_type = #pk_type_ty;
        }
    };
    let single_query_argument_specs = if has_composite_primary_key {
        primary_key_fields
            .iter()
            .map(|field| {
                let arg_name = apply_graphql_case(&field.graphql_name, argument_case);
                (field.field_name.clone(), field.field_type.clone(), arg_name)
            })
            .collect::<Vec<_>>()
    } else {
        vec![(
            syn::Ident::new("id", proc_macro2::Span::call_site()),
            pk_type_ty.clone(),
            id_arg_name.clone(),
        )]
    };
    let single_query_args = single_query_argument_specs
        .iter()
        .map(|(field_name, field_type, arg_name)| {
            let description = semantic_argument_description(arg_name);
            quote! {
                #[graphql(name = #arg_name, desc = #description)] #field_name: #field_type,
            }
        })
        .collect::<Vec<_>>();
    let single_query_key_init = if has_composite_primary_key {
        let fields = primary_key_fields
            .iter()
            .map(|field| {
                let field_name = &field.field_name;
                quote! { #field_name, }
            })
            .collect::<Vec<_>>();
        quote! {
            let key = #key_type {
                #(#fields)*
            };
        }
    } else {
        quote! {
            let key = id;
        }
    };
    let scope_enforcement = |category: &str| {
        let arguments = match category {
            "list" => vec![
                ScopeTemplateArgumentSpec::complex(where_arg_name.clone()),
                ScopeTemplateArgumentSpec::complex(order_by_arg_name.clone()),
                ScopeTemplateArgumentSpec::complex(page_arg_name.clone()),
            ],
            "aggregate" => vec![
                ScopeTemplateArgumentSpec::complex(where_arg_name.clone()),
                ScopeTemplateArgumentSpec::complex(group_by_arg_name.clone()),
                ScopeTemplateArgumentSpec::complex(metrics_arg_name.clone()),
                ScopeTemplateArgumentSpec::complex(group_limit_arg_name.clone()),
            ],
            "search" => vec![
                ScopeTemplateArgumentSpec::complex(search_arg_name.clone()),
                ScopeTemplateArgumentSpec::complex(where_arg_name.clone()),
                ScopeTemplateArgumentSpec::complex(page_arg_name.clone()),
            ],
            "keyset_list" => vec![
                ScopeTemplateArgumentSpec::complex(where_arg_name.clone()),
                ScopeTemplateArgumentSpec::complex(page_arg_name.clone()),
            ],
            "single_read" => single_query_argument_specs
                .iter()
                .map(|(rust_ident, rust_type, graphql_name)| {
                    ScopeTemplateArgumentSpec::scalar(
                        graphql_name.clone(),
                        rust_ident.clone(),
                        rust_type.clone(),
                    )
                })
                .collect(),
            "create" | "upsert" => {
                vec![ScopeTemplateArgumentSpec::complex(input_arg_name.clone())]
            }
            "update" => vec![
                ScopeTemplateArgumentSpec::scalar(
                    id_arg_name.clone(),
                    syn::Ident::new("id", proc_macro2::Span::call_site()),
                    pk_type_ty.clone(),
                ),
                ScopeTemplateArgumentSpec::complex(input_arg_name.clone()),
            ],
            "update_many" => vec![
                ScopeTemplateArgumentSpec::complex(where_arg_name.clone()),
                ScopeTemplateArgumentSpec::complex(input_arg_name.clone()),
            ],
            "delete" => vec![ScopeTemplateArgumentSpec::scalar(
                id_arg_name.clone(),
                syn::Ident::new("id", proc_macro2::Span::call_site()),
                pk_type_ty.clone(),
            )],
            "delete_many" => {
                vec![ScopeTemplateArgumentSpec::complex(where_arg_name.clone())]
            }
            "subscription" => {
                vec![ScopeTemplateArgumentSpec::complex(filter_arg_name.clone())]
            }
            _ => unreachable!("validated generated operation category"),
        };
        scope_enforcement_tokens(category, operation_authorizations.get(category), &arguments)
    };
    let list_scope_enforcement = scope_enforcement("list")?;
    let aggregate_scope_enforcement = scope_enforcement("aggregate")?;
    let search_scope_enforcement = scope_enforcement("search")?;
    let keyset_list_scope_enforcement = scope_enforcement("keyset_list")?;
    let single_read_scope_enforcement = scope_enforcement("single_read")?;
    let create_scope_enforcement = scope_enforcement("create")?;
    let upsert_scope_enforcement = scope_enforcement("upsert")?;
    let update_scope_enforcement = scope_enforcement("update")?;
    let update_many_scope_enforcement = scope_enforcement("update_many")?;
    let delete_scope_enforcement = scope_enforcement("delete")?;
    let delete_many_scope_enforcement = scope_enforcement("delete_many")?;
    let subscription_scope_enforcement = scope_enforcement("subscription")?;
    let key_bind_from_key_tokens = if has_composite_primary_key {
        primary_key_fields
            .iter()
            .map(|field| {
                let field_name = &field.field_name;
                push_create_input_sql_value_tokens(
                    backend,
                    struct_name,
                    quote! { key.#field_name.clone() },
                    &field.field_type,
                    &field.meta,
                    quote! { ::graphql_orm::sqlx::Error },
                )
            })
            .collect::<Vec<_>>()
    } else {
        vec![push_create_input_sql_value_tokens(
            backend,
            struct_name,
            quote! { key.clone() },
            &pk_type_ty,
            &first_primary_key.meta,
            quote! { ::graphql_orm::sqlx::Error },
        )]
    };
    let key_condition_method = quote! {
        #[doc(hidden)]
        pub fn __gom_key_where_clause() -> String {
            use ::graphql_orm::graphql::orm::{OrmBackend, SqlDialect};
            let dialect = <#backend_marker as OrmBackend>::DIALECT;
            <Self as ::graphql_orm::graphql::orm::DatabaseEntity>::PRIMARY_KEYS
                .iter()
                .enumerate()
                .map(|(index, column)| {
                    format!(
                        "{} = {}",
                        dialect.quote_identifier_path(column),
                        dialect.placeholder(index + 1),
                    )
                })
                .collect::<Vec<_>>()
                .join(" AND ")
        }

        #[doc(hidden)]
        pub fn __gom_key_values(key: &#key_type) -> Vec<::graphql_orm::graphql::orm::SqlValue> {
            let mut bind_values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
            #(#key_bind_from_key_tokens)*
            bind_values
        }
    };
    let subscription_key_binds = primary_key_fields
        .iter()
        .map(|field| {
            let name = &field.field_name;
            push_create_input_sql_value_tokens(
                backend,
                struct_name,
                quote! { entity.#name.clone() },
                &field.field_type,
                &field.meta,
                quote! { ::graphql_orm::sqlx::Error },
            )
        })
        .collect::<Vec<_>>();
    let subscription_key_visibility =
        if crate::entity::has_repository_entity_attribute(&input.attrs) {
            quote! {}
        } else {
            quote! { pub }
        };
    let subscription_key_method = quote! {
        #[doc(hidden)]
        pub const __GOM_CHANGED_ROOT_NAME: &'static str = #subscription_name;

        #[doc(hidden)]
        #subscription_key_visibility fn __gom_authorize_changed_operation(ctx: &::graphql_orm::async_graphql::Context<'_>)
            -> ::graphql_orm::async_graphql::Result<()> {
            let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
            #subscription_scope_enforcement
            ::graphql_orm::graphql::assurance::enforce_operation_assurance(
                ctx, ::graphql_orm::graphql::orm::GraphqlOperationKind::Subscription, #subscription_name,
            )
        }
        #[doc(hidden)]
        #subscription_key_visibility fn __gom_subscription_key_values(entity: &Self) -> Vec<::graphql_orm::graphql::orm::SqlValue> {
            let mut bind_values = Vec::new();
            #(#subscription_key_binds)*
            bind_values
        }
    };
    let subscription_identity_type = if has_composite_primary_key {
        quote! { String }
    } else {
        quote! { #pk_type }
    };
    let subscription_extra_key_field = if has_composite_primary_key {
        quote! { pub key: #key_type, }
    } else {
        quote! {}
    };
    let subscription_key_initializer = if has_composite_primary_key {
        quote! { key: id.clone(), }
    } else {
        quote! {}
    };
    let propagated_event_id = if has_composite_primary_key {
        quote! { ::graphql_orm::graphql::orm::composite_key_id(&Self::__gom_key_values(&id)) }
    } else {
        quote! { id.clone() }
    };
    let subscription_event_key = if has_composite_primary_key {
        quote! { event.key }
    } else {
        quote! { event.id }
    };
    let propagation_entity_key = if has_composite_primary_key {
        let components = primary_key_fields.iter().map(|field| {
            let name = &field.field_name;
            quote! { #name: entity.#name.clone() }
        });
        quote! { #key_type { #(#components),* } }
    } else {
        quote! { entity.#pk_field.clone() }
    };
    // Repository-only observers keep their existing redacted event contract;
    // these entities have no generated GraphQL subscription channel.
    let subscription_witness_emission = |publish: proc_macro2::TokenStream| {
        if crate::entity::has_repository_entity_attribute(&input.attrs) {
            quote! {}
        } else {
            quote! {
                let mut event = event;
                if action != ::graphql_orm::graphql::orm::ChangeAction::Deleted { event.entity = None; }
                if let Some(witness) = ::graphql_orm::graphql::subscriptions::SubscriptionEvent::<#changed_event, #backend_marker>::new(
                    event, Self::__gom_subscription_key_values(entity), action == ::graphql_orm::graphql::orm::ChangeAction::Deleted, Vec::new(),
                ) { #publish }
            }
        }
    };
    let queue_subscription_witness =
        subscription_witness_emission(quote! { hook_ctx.queue_event(witness); });
    let emit_subscription_witness =
        subscription_witness_emission(quote! { db.emit_event(witness); });
    let propagated_witness_emission = if crate::entity::has_repository_entity_attribute(
        &input.attrs,
    ) {
        quote! {}
    } else {
        quote! {
            if let Some(parent_entity) = parent_entity {
                let mut event = event;
                event.entity = None;
                if let Some(witness) = ::graphql_orm::graphql::subscriptions::SubscriptionEvent::<#changed_event, #backend_marker>::new(
                    event, Self::__gom_subscription_key_values(parent_entity), false, provenance,
                ) { hook_ctx.queue_event(witness); }
            }
        }
    };
    // These are generated host-to-host hooks, like SubscriptionEvent::new.
    // Public entity expansions may cross Rust crates. Keep repository-only
    // entity hooks private; visibility grants no subscriber release authority.
    let propagated_subscription_emitter = quote! {
        #[doc(hidden)]
        #subscription_key_visibility fn __gom_propagation_key(entity: &Self) -> #key_type {
            #propagation_entity_key
        }

        #[doc(hidden)]
        #[allow(clippy::too_many_arguments, unused_variables)]
        #subscription_key_visibility fn __gom_queue_propagated_changed_event(
            hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
            action: ::graphql_orm::graphql::orm::ChangeAction,
            id: #key_type,
            parent_entity: Option<&Self>,
            source_entity: &str,
            source_id: &str,
            path: Vec<String>,
            provenance: Vec<::graphql_orm::graphql::subscriptions::SubscriptionEventSource<#backend_marker>>,
        ) {
            let event = #changed_event {
                action,
                change_kind: ::graphql_orm::graphql::orm::ChangeKind::Propagated,
                id: #propagated_event_id,
                #subscription_key_initializer
                source_entity: (action != ::graphql_orm::graphql::orm::ChangeAction::Deleted).then(|| source_entity.to_string()),
                source_id: (action != ::graphql_orm::graphql::orm::ChangeAction::Deleted).then(|| source_id.to_string()),
                path,
                entity: parent_entity.cloned(),
            };
            hook_ctx.queue_event(event.clone());
            #propagated_witness_emission
        }
    };
    // Direct and propagated events must share exactly the same typed target
    // identity and origin encoding, so a cycle cannot re-emit its source.
    let propagation_visited_seed = quote! {
        let origin_target_id = ::graphql_orm::graphql::orm::composite_key_id(
            &Self::__gom_subscription_key_values(entity),
        );
        let origin_dedupe_key = ::graphql_orm::serde_json::to_string(&(
            #entity_name_lit, origin_target_id, #entity_name_lit, &source_id,
        )).map_err(|error| ::graphql_orm::sqlx::Error::Protocol(error.to_string()))?;
        let mut visited = ::std::collections::HashSet::from([origin_dedupe_key]);
    };
    let propagation_blocks: Vec<proc_macro2::TokenStream> = propagated_relations.iter()
        .map(|relation| -> syn::Result<proc_macro2::TokenStream> {
            let relation_name = &relation.graphql_name;
            let target_type = &relation.target_type;
            let target_fields = &relation.target_fields;
            let mut value_tokens = Vec::new();
            let mut optional_checks = Vec::new();
            let mut source_names = Vec::new();
            for (field, ty, metadata) in &relation.source_fields {
                source_names.push(field.to_string());
                if is_option_type(ty) { optional_checks.push(quote! { entity.#field.is_some() }); }
                value_tokens.push(push_create_input_sql_value_tokens(backend, struct_name,
                    quote! { entity.#field.clone() }, ty, metadata, quote! { ::graphql_orm::sqlx::Error }));
            }
            Ok(quote! {
                if true #(&& #optional_checks)* {
                    if visited.len() >= ::graphql_orm::graphql::subscriptions::MAX_SUBSCRIPTION_PROPAGATION_EVENTS { return Ok(()); }
                    let mut bind_values = Vec::new();
                    #(#value_tokens)*
                    let parent_entities = ::graphql_orm::graphql::subscriptions::propagation_targets::<#target_type, #backend_marker>(
                        hook_ctx, &[#(#target_fields),*], &bind_values,
                        ::graphql_orm::graphql::subscriptions::MAX_SUBSCRIPTION_PROPAGATION_EVENTS - visited.len(),
                    ).await?;
                    for parent_entity in parent_entities {
                        let parent_id = <#target_type>::__gom_propagation_key(&parent_entity);
                        let parent_keys = <#target_type>::__gom_subscription_key_values(&parent_entity);
                        let parent_id_string = ::graphql_orm::graphql::orm::composite_key_id(&parent_keys);
                        // Reject cycles within this path, without discarding a
                        // different path with different authorization witnesses.
                        if provenance.iter().any(|source| {
                            let (entity, id, _) = source.propagation_identity();
                            entity == <#target_type as ::graphql_orm::graphql::orm::Entity>::entity_name()
                                && id == parent_id_string
                        }) || (<Self as ::graphql_orm::graphql::orm::Entity>::entity_name()
                            == <#target_type as ::graphql_orm::graphql::orm::Entity>::entity_name()
                            && ::graphql_orm::graphql::orm::composite_key_id(&Self::__gom_subscription_key_values(entity)) == parent_id_string) {
                            continue;
                        }
                        let mut next_path = path.clone();
                        next_path.push(#relation_name.to_string());
                        let mut next_provenance = provenance.to_vec();
                        if action == ::graphql_orm::graphql::orm::ChangeAction::Deleted && provenance.is_empty() {
                            // The FK tuple came from the deleted preimage inside this transaction.
                            // Retain only the surviving parent's identity and authority.
                            next_provenance.push(::graphql_orm::graphql::subscriptions::SubscriptionEventSource::<#backend_marker>::new_delete_target::<#target_type>(
                                parent_keys.clone(), #relation_name, <#target_type>::__gom_authorize_changed_operation,
                            ));
                        } else {
                            next_provenance.push(::graphql_orm::graphql::subscriptions::SubscriptionEventSource::<#backend_marker>::new_relation::<Self, #target_type>(
                                Self::__gom_subscription_key_values(entity), #relation_name, vec![#((#source_names, #target_fields)),*], Self::__gom_authorize_changed_operation,
                            ));
                        }
                        let dedupe_key = ::graphql_orm::serde_json::to_string(&(
                            <#target_type as ::graphql_orm::graphql::orm::Entity>::entity_name(),
                            parent_id_string, source_entity, source_id,
                            next_provenance.iter().map(|source| source.propagation_identity()).collect::<Vec<_>>(),
                        )).map_err(|error| ::graphql_orm::sqlx::Error::Protocol(error.to_string()))?;
                        if visited.insert(dedupe_key) {
                            <#target_type>::__gom_queue_propagated_changed_event(hook_ctx, action, parent_id,
                                Some(&parent_entity), source_entity, source_id, next_path.clone(), next_provenance.clone());
                            <#target_type>::__gom_propagate_changed_event(hook_ctx, action, &parent_entity,
                                source_entity, source_id, next_path, visited, &next_provenance).await?;
                        }
                    }
                }
            })
        }).collect::<syn::Result<Vec<_>>>()?;

    // Generate input fields (excluding primary key for create, all optional for update)
    let mut create_input_fields: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut graphql_create_input_fields: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut create_input_from_graphql_fields: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_input_fields: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut graphql_update_input_fields: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_input_from_graphql_fields: Vec<proc_macro2::TokenStream> = Vec::new();

    // For SQL generation
    let mut insert_columns: Vec<String> = Vec::new();
    let mut insert_value_exprs: Vec<String> = Vec::new();
    let mut insert_default_columns: Vec<String> = Vec::new();
    let mut insert_default_exprs: Vec<String> = Vec::new();
    let mut insert_binds_graphql: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut insert_binds_repo: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_field_checks_graphql: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_field_checks_repo: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut create_policy_checks: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_policy_checks: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_runtime_policy_checks: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_bulk_omission_checks: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut upsert_policy_checks_graphql: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut upsert_policy_checks_repo: Vec<proc_macro2::TokenStream> = Vec::new();
    // Field write decisions of the repository insert and update paths. A
    // `RepositoryEntity` asks them through its `Entity` hook instead.
    let mut repository_create_field_checks: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut repository_update_field_checks: Vec<proc_macro2::TokenStream> = Vec::new();
    // Which key-less optional fields the GraphQL client supplied, recorded
    // before an input transform or hook can change the input. A GraphQL write
    // asks about a key-less field only when the client could and did send it.
    let mut create_client_supplied: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_client_supplied: Vec<proc_macro2::TokenStream> = Vec::new();
    let repository_entity_hook_asks = crate::entity::has_repository_entity_attribute(&input.attrs);
    let mut upsert_update_columns: Vec<String> = Vec::new();
    let mut upsert_change_fields: Vec<String> = Vec::new();
    let mut upsert_change_binds_graphql: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut upsert_change_binds_repo: Vec<proc_macro2::TokenStream> = Vec::new();
    let upsert_target_db_columns = upsert_config
        .as_ref()
        .map(|config| {
            config
                .fields
                .iter()
                .map(|field| field.db_column.clone())
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    // Track string-filterable fields for search_similar
    let mut string_filterable_fields: Vec<(syn::Ident, bool)> = Vec::new(); // (field_name, is_option)

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;
        let meta = parse_field_metadata(field)?;

        // Skip relations and computed fields
        if meta.is_relation || meta.skip_db {
            continue;
        }

        if meta.spatial.is_some() {
            if backend == BackendKind::Mssql {
                return Err(syn::Error::new(
                    field.span(),
                    "spatial fields are currently supported for backend = \"postgres\" and backend = \"sqlite\"",
                ));
            }
            if !is_serde_json_value_or_option(field_type) {
                return Err(syn::Error::new(
                    field.span(),
                    "spatial fields must use serde_json::Value or Option<serde_json::Value>",
                ));
            }
        }

        let rust_name = field_name.to_string();
        let graphql_name =
            graphql_field_name(&meta, &rust_name, graphql_rename_fields, serde_rename_all);
        let db_col = meta.db_column.clone().unwrap_or_else(|| rust_name.clone());
        let db_col_sql = backend_render_identifier_path(backend, &db_col);
        let is_json_like_field = meta.is_json_field || meta.spatial.is_some();
        // One rule on every generated write path: the field write decision is
        // asked for each field written, with its declared key or `None`.
        let write_policy_key = match &meta.write_policy {
            Some(policy_key) => quote! { Some(#policy_key) },
            None => quote! { None },
        };

        // Track string-filterable fields for fuzzy search
        if meta.filter && meta.filterable.as_deref() == Some("string") {
            string_filterable_fields.push((field_name.clone(), is_option_type(field_type)));
        }

        // Skip auto-generated primary key, timestamps, and skip_input fields (e.g. password_hash)
        // But #[input_only] overrides skip_input (allows write-only fields like encrypted credentials)
        let is_timestamp = meta.timestamp_behavior(&rust_name).exclude_input;
        let include_in_create =
            (!meta.is_primary_key || !auto_generated_pk) && !is_timestamp && meta.write;
        let include_generated_default_in_create = (!meta.is_primary_key || !auto_generated_pk)
            && !is_timestamp
            && !meta.write
            && meta.default.is_some();
        if include_in_create {
            let graphql_include_in_create = !meta.skip_input || meta.input_only;
            let graphql_create_field_type = if is_json_like_field {
                if let Some(inner_type) = option_inner_type(field_type) {
                    quote! { Option<::graphql_orm::async_graphql::Json<#inner_type>> }
                } else {
                    quote! { ::graphql_orm::async_graphql::Json<#field_type> }
                }
            } else {
                quote! { #field_type }
            };
            let create_policy_check = {
                if is_option_type(field_type) {
                    quote! {
                        if !db.check_field_write(
                            ctx,
                            #entity_name_lit,
                            #graphql_name,
                            #write_policy_key,
                            None,
                            Some(&input.#field_name as &(dyn ::std::any::Any + Send + Sync)),
                        ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)? {
                            input.#field_name = None;
                        }
                    }
                } else {
                    quote! {
                        db.ensure_writable_field(
                            ctx,
                            #entity_name_lit,
                            #graphql_name,
                            #write_policy_key,
                            None,
                            Some(&input.#field_name as &(dyn ::std::any::Any + Send + Sync)),
                        ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                    }
                }
            };
            // A field with a declared key is asked as before, also when the
            // optional input is absent. A key-less field is asked only when it
            // is part of the GraphQL input type and the client supplied it; a
            // required input is always supplied.
            let keyless = meta.write_policy.is_none();
            let graphql_create_asks = !keyless || graphql_include_in_create;
            let graphql_create_condition = if keyless && is_option_type(field_type) {
                create_client_supplied.push(quote! {
                    if input.#field_name.is_some() {
                        __gom_client_supplied.insert(#graphql_name);
                    }
                });
                quote! { __gom_client_supplied.contains(#graphql_name) }
            } else {
                quote! { true }
            };
            if graphql_create_asks {
                create_policy_checks.push(quote! {
                    if #graphql_create_condition {
                        #create_policy_check
                    }
                });
            }
            // Repository code writes every create field; ORM-managed columns
            // are not the caller's to write and are not asked about.
            if !repository_entity_hook_asks && !meta.managed_version {
                repository_create_field_checks.push(quote! {
                    db.ensure_repository_writable_field(
                        None,
                        #entity_name_lit,
                        #graphql_name,
                        #write_policy_key,
                        None,
                        Some(&input.#field_name as &(dyn ::std::any::Any + Send + Sync)),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                });
            }
            // For create: use the field type directly (required fields stay required)
            create_input_fields.push(quote! {
                pub #field_name: #field_type,
            });
            if graphql_include_in_create {
                graphql_create_input_fields.push(quote! {
                    #[graphql(name = #graphql_name)]
                    pub #field_name: #graphql_create_field_type,
                });
                if is_json_like_field {
                    if option_inner_type(field_type).is_some() {
                        create_input_from_graphql_fields.push(quote! {
                            #field_name: input.#field_name.map(|value| value.0),
                        });
                    } else {
                        create_input_from_graphql_fields.push(quote! {
                            #field_name: input.#field_name.0,
                        });
                    }
                } else {
                    create_input_from_graphql_fields.push(quote! {
                        #field_name: input.#field_name,
                    });
                }
            } else {
                create_input_from_graphql_fields.push(quote! {
                    #field_name: ::std::default::Default::default(),
                });
            }

            if upsert_config.is_some() {
                let upsert_update_candidate =
                    !meta.is_primary_key && !upsert_target_db_columns.contains(&db_col);

                {
                    let graphql_check = if is_option_type(field_type) {
                        if upsert_update_candidate {
                            quote! {
                                let __gom_field_write_allowed = if let Some(current_entity) = current_entity.as_ref() {
                                    db.check_field_write(
                                        ctx,
                                        #entity_name_lit,
                                        #graphql_name,
                                        #write_policy_key,
                                        Some(current_entity as &(dyn ::std::any::Any + Send + Sync)),
                                        Some(&input.#field_name as &(dyn ::std::any::Any + Send + Sync)),
                                    ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?
                                } else {
                                    db.check_field_write(
                                        ctx,
                                        #entity_name_lit,
                                        #graphql_name,
                                        #write_policy_key,
                                        None,
                                        Some(&input.#field_name as &(dyn ::std::any::Any + Send + Sync)),
                                    ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?
                                };
                                if !__gom_field_write_allowed {
                                    input.#field_name = None;
                                }
                            }
                        } else {
                            quote! {
                                if current_entity.is_none()
                                    && !db.check_field_write(
                                        ctx,
                                        #entity_name_lit,
                                        #graphql_name,
                                        #write_policy_key,
                                        None,
                                        Some(&input.#field_name as &(dyn ::std::any::Any + Send + Sync)),
                                    ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?
                                {
                                    input.#field_name = None;
                                }
                            }
                        }
                    } else if upsert_update_candidate {
                        quote! {
                            if let Some(current_entity) = current_entity.as_ref() {
                                db.ensure_writable_field(
                                    ctx,
                                    #entity_name_lit,
                                    #graphql_name,
                                    #write_policy_key,
                                    Some(current_entity as &(dyn ::std::any::Any + Send + Sync)),
                                    Some(&input.#field_name as &(dyn ::std::any::Any + Send + Sync)),
                                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                            } else {
                                db.ensure_writable_field(
                                    ctx,
                                    #entity_name_lit,
                                    #graphql_name,
                                    #write_policy_key,
                                    None,
                                    Some(&input.#field_name as &(dyn ::std::any::Any + Send + Sync)),
                                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                            }
                        }
                    } else {
                        quote! {
                            if current_entity.is_none() {
                                db.ensure_writable_field(
                                    ctx,
                                    #entity_name_lit,
                                    #graphql_name,
                                    #write_policy_key,
                                    None,
                                    Some(&input.#field_name as &(dyn ::std::any::Any + Send + Sync)),
                                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                            }
                        }
                    };
                    let repo_check = if upsert_update_candidate {
                        quote! {
                            if let Some(current_entity) = current_entity.as_ref() {
                                db.ensure_repository_writable_field(
                                    None,
                                    #entity_name_lit,
                                    #graphql_name,
                                    #write_policy_key,
                                    Some(current_entity as &(dyn ::std::any::Any + Send + Sync)),
                                    Some(&input.#field_name as &(dyn ::std::any::Any + Send + Sync)),
                                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                            } else {
                                db.ensure_repository_writable_field(
                                    None,
                                    #entity_name_lit,
                                    #graphql_name,
                                    #write_policy_key,
                                    None,
                                    Some(&input.#field_name as &(dyn ::std::any::Any + Send + Sync)),
                                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                            }
                        }
                    } else {
                        quote! {
                            if current_entity.is_none() {
                                db.ensure_repository_writable_field(
                                    None,
                                    #entity_name_lit,
                                    #graphql_name,
                                    #write_policy_key,
                                    None,
                                    Some(&input.#field_name as &(dyn ::std::any::Any + Send + Sync)),
                                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                            }
                        }
                    };
                    // The conflict update sets every update candidate, also one
                    // the client left out: an omitted optional field is written
                    // as NULL and a field outside the GraphQL input type as its
                    // default. On an existing row the decision is therefore
                    // asked for every update candidate, with the current row
                    // and the value about to be written. The insert branch
                    // keeps the client-supplied rule. ORM-managed version
                    // columns are not the client's to write.
                    let existing_row_asks = upsert_update_candidate && !meta.managed_version;
                    let supplied_condition_is_constant = !(keyless && is_option_type(field_type));
                    if graphql_create_asks && (supplied_condition_is_constant || !existing_row_asks)
                    {
                        upsert_policy_checks_graphql.push(quote! {
                            if #graphql_create_condition {
                                #graphql_check
                            }
                        });
                    } else if graphql_create_asks {
                        upsert_policy_checks_graphql.push(quote! {
                            if #graphql_create_condition || current_entity.is_some() {
                                #graphql_check
                            }
                        });
                    } else if existing_row_asks {
                        upsert_policy_checks_graphql.push(quote! {
                            if current_entity.is_some() {
                                #graphql_check
                            }
                        });
                    }
                    if !meta.managed_version {
                        upsert_policy_checks_repo.push(repo_check);
                    }
                }

                if upsert_update_candidate {
                    upsert_update_columns.push(db_col.clone());
                    upsert_change_fields.push(db_col.clone());
                    upsert_change_binds_graphql.push(push_create_input_sql_value_tokens(
                        backend,
                        struct_name,
                        quote! { input.#field_name.clone() },
                        field_type,
                        &meta,
                        quote! { ::graphql_orm::async_graphql::Error },
                    ));
                    upsert_change_binds_repo.push(push_create_input_sql_value_tokens(
                        backend,
                        struct_name,
                        quote! { input.#field_name.clone() },
                        field_type,
                        &meta,
                        quote! { ::graphql_orm::sqlx::Error },
                    ));
                }
            }

            // Track columns for INSERT
            insert_columns.push(db_col.clone());
            insert_value_exprs.push(
                meta.spatial
                    .as_ref()
                    .map(|spatial| {
                        if backend == BackendKind::Postgres {
                            format!("ST_SetSRID(ST_GeomFromGeoJSON(?::jsonb), {})", spatial.srid)
                        } else {
                            "?".to_string()
                        }
                    })
                    .unwrap_or_else(|| "?".to_string()),
            );

            // Generate bind value push based on field type
            // We push to bind_values vector to avoid lifetime issues with ::graphql_orm::sqlx::query
            if meta.decimal.is_some() {
                insert_binds_graphql.push(push_create_input_sql_value_tokens(
                    backend,
                    struct_name,
                    quote! { input.#field_name.clone() },
                    field_type,
                    &meta,
                    quote! { ::graphql_orm::async_graphql::Error },
                ));
                insert_binds_repo.push(push_create_input_sql_value_tokens(
                    backend,
                    struct_name,
                    quote! { input.#field_name.clone() },
                    field_type,
                    &meta,
                    quote! { ::graphql_orm::sqlx::Error },
                ));
            } else if meta.spatial.is_some() {
                insert_binds_graphql.push(push_create_input_sql_value_tokens(
                    backend,
                    struct_name,
                    quote! { input.#field_name.clone() },
                    field_type,
                    &meta,
                    quote! { ::graphql_orm::async_graphql::Error },
                ));
                insert_binds_repo.push(push_create_input_sql_value_tokens(
                    backend,
                    struct_name,
                    quote! { input.#field_name.clone() },
                    field_type,
                    &meta,
                    quote! { ::graphql_orm::sqlx::Error },
                ));
            } else if meta.is_boolean_field || is_bool_type(field_type) {
                if is_option_type(field_type) {
                    let bind_tokens = quote! {
                        match input.#field_name {
                            Some(b) => bind_values.push(#struct_name::__gom_bool_sql_value(b)),
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::BoolNull),
                        }
                    };
                    insert_binds_graphql.push(bind_tokens.clone());
                    insert_binds_repo.push(bind_tokens);
                } else {
                    let bind_tokens = quote! {
                        bind_values.push(#struct_name::__gom_bool_sql_value(input.#field_name));
                    };
                    insert_binds_graphql.push(bind_tokens.clone());
                    insert_binds_repo.push(bind_tokens);
                }
            } else if meta.is_json_field
                || (is_vec_type(field_type) && !is_byte_vec_type(field_type))
            {
                if is_option_type(field_type) {
                    insert_binds_graphql.push(quote! {
                        match &input.#field_name {
                            Some(value) => bind_values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::async_graphql::Error>(value)?),
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::JsonNull),
                        }
                    });
                    insert_binds_repo.push(quote! {
                        match &input.#field_name {
                            Some(value) => bind_values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::sqlx::Error>(value)?),
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::JsonNull),
                        }
                    });
                } else {
                    insert_binds_graphql.push(quote! {
                        bind_values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::async_graphql::Error>(&input.#field_name)?);
                    });
                    insert_binds_repo.push(quote! {
                        bind_values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::sqlx::Error>(&input.#field_name)?);
                    });
                }
            } else if is_uuid_type(field_type) {
                if is_option_type(field_type) {
                    let bind_tokens = quote! {
                        match input.#field_name {
                            Some(v) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(v)),
                            None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::UuidNull),
                        }
                    };
                    insert_binds_graphql.push(bind_tokens.clone());
                    insert_binds_repo.push(bind_tokens);
                } else {
                    let bind_tokens = quote! {
                        bind_values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(input.#field_name));
                    };
                    insert_binds_graphql.push(bind_tokens.clone());
                    insert_binds_repo.push(bind_tokens);
                }
            } else if option_inner_type(field_type)
                .and_then(type_path_last_ident)
                .is_some_and(|ident| {
                    matches!(
                        ident.to_string().as_str(),
                        "i8" | "i16"
                            | "i32"
                            | "i64"
                            | "isize"
                            | "u8"
                            | "u16"
                            | "u32"
                            | "u64"
                            | "usize"
                    )
                })
            {
                let bind_tokens = quote! {
                    match input.#field_name {
                        Some(v) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Int(v as i64)),
                        None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::IntNull),
                    }
                };
                insert_binds_graphql.push(bind_tokens.clone());
                insert_binds_repo.push(bind_tokens);
            } else if option_inner_type(field_type)
                .and_then(type_path_last_ident)
                .is_some_and(|ident| matches!(ident.to_string().as_str(), "f32" | "f64"))
            {
                let bind_tokens = quote! {
                    match input.#field_name {
                        Some(v) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Float(v.into())),
                        None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::FloatNull),
                    }
                };
                insert_binds_graphql.push(bind_tokens.clone());
                insert_binds_repo.push(bind_tokens);
            } else if type_path_last_ident(field_type).is_some_and(|ident| {
                matches!(
                    ident.to_string().as_str(),
                    "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize"
                )
            }) {
                let bind_tokens = quote! {
                    bind_values.push(::graphql_orm::graphql::orm::SqlValue::Int(input.#field_name as i64));
                };
                insert_binds_graphql.push(bind_tokens.clone());
                insert_binds_repo.push(bind_tokens);
            } else if type_path_last_ident(field_type)
                .is_some_and(|ident| matches!(ident.to_string().as_str(), "f32" | "f64"))
            {
                let bind_tokens = quote! {
                    bind_values.push(::graphql_orm::graphql::orm::SqlValue::Float(input.#field_name.into()));
                };
                insert_binds_graphql.push(bind_tokens.clone());
                insert_binds_repo.push(bind_tokens);
            } else if is_byte_vec_type(field_type) {
                let bind_tokens = quote! {
                    bind_values.push(::graphql_orm::graphql::orm::SqlValue::Bytes(input.#field_name.clone()));
                };
                insert_binds_graphql.push(bind_tokens.clone());
                insert_binds_repo.push(bind_tokens);
            } else if is_option_type(field_type)
                && option_inner_type(field_type).is_some_and(is_byte_vec_type)
            {
                let bind_tokens = quote! {
                    match &input.#field_name {
                        Some(v) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::Bytes(v.clone())),
                        None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::BytesNull),
                    }
                };
                insert_binds_graphql.push(bind_tokens.clone());
                insert_binds_repo.push(bind_tokens);
            } else if is_option_type(field_type) {
                let value_expr = quote! { v.to_string() };

                let bind_tokens = quote! {
                    match &input.#field_name {
                        Some(v) => bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr)),
                        None => bind_values.push(::graphql_orm::graphql::orm::SqlValue::StringNull),
                    }
                };
                insert_binds_graphql.push(bind_tokens.clone());
                insert_binds_repo.push(bind_tokens);
            } else {
                let value_expr = quote! { input.#field_name.to_string() };

                let bind_tokens = quote! {
                    bind_values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr));
                };
                insert_binds_graphql.push(bind_tokens.clone());
                insert_binds_repo.push(bind_tokens);
            }
        } else if include_generated_default_in_create {
            insert_default_columns.push(db_col.clone());
            insert_default_exprs.push(
                meta.default
                    .clone()
                    .expect("generated create default must exist"),
            );
        }

        // For update: wrap in Option to make all fields optional (skip PK, timestamps, skip_input)
        // But #[input_only] overrides skip_input (allows write-only fields like encrypted credentials)
        let is_timestamp = meta.timestamp_behavior(&rust_name).exclude_input;
        if !meta.is_primary_key && !is_timestamp && meta.write {
            // All update fields are wrapped in Option (even if already optional)
            // This allows distinguishing between "not provided" and "set to null"
            let is_already_optional = is_option_type(field_type);
            let update_type = quote! { Option<#field_type> };
            let graphql_update_type = if let Some(inner_type) = option_inner_type(field_type) {
                if is_json_like_field {
                    quote! { ::graphql_orm::async_graphql::MaybeUndefined<::graphql_orm::async_graphql::Json<#inner_type>> }
                } else {
                    quote! { ::graphql_orm::async_graphql::MaybeUndefined<#inner_type> }
                }
            } else if is_json_like_field {
                quote! { Option<::graphql_orm::async_graphql::Json<#field_type>> }
            } else {
                quote! { #update_type }
            };
            let graphql_include_in_update = !meta.skip_input || meta.input_only;
            update_input_fields.push(quote! {
                pub #field_name: #update_type,
            });
            if graphql_include_in_update {
                graphql_update_input_fields.push(quote! {
                    #[graphql(name = #graphql_name)]
                    pub #field_name: #graphql_update_type,
                });
                if is_json_like_field && is_already_optional {
                    update_input_from_graphql_fields.push(quote! {
                        #field_name: match input.#field_name {
                            ::graphql_orm::async_graphql::MaybeUndefined::Value(value) => Some(Some(value.0)),
                            ::graphql_orm::async_graphql::MaybeUndefined::Null => Some(None),
                            ::graphql_orm::async_graphql::MaybeUndefined::Undefined => None,
                        },
                    });
                } else if is_json_like_field {
                    update_input_from_graphql_fields.push(quote! {
                        #field_name: input.#field_name.map(|value| value.0),
                    });
                } else if is_already_optional {
                    update_input_from_graphql_fields.push(quote! {
                        #field_name: input.#field_name.into(),
                    });
                } else {
                    update_input_from_graphql_fields.push(quote! {
                        #field_name: input.#field_name,
                    });
                }
            } else {
                update_input_from_graphql_fields.push(quote! {
                    #field_name: ::std::default::Default::default(),
                });
            }

            // Generate update field check
            let update_policy_check = {
                quote! {
                    let __gom_field_write_allowed = if let Some(val) = &input.#field_name {
                        db.check_field_write(
                            ctx,
                            #entity_name_lit,
                            #graphql_name,
                            #write_policy_key,
                            Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                            Some(val as &(dyn ::std::any::Any + Send + Sync)),
                        ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?
                    } else {
                        true
                    };
                    if !__gom_field_write_allowed {
                        input.#field_name = None;
                    }
                }
            };
            let keyless = meta.write_policy.is_none();
            let graphql_update_asks = !keyless || graphql_include_in_update;
            let graphql_update_condition = if keyless {
                update_client_supplied.push(quote! {
                    if __gom_client_input.#field_name.is_some() {
                        __gom_client_supplied.insert(#graphql_name);
                    }
                });
                quote! { __gom_client_supplied.contains(#graphql_name) }
            } else {
                quote! { true }
            };
            if graphql_update_asks {
                update_policy_checks.push(quote! {
                    if #graphql_update_condition {
                        #update_policy_check
                    }
                });
            }
            if !repository_entity_hook_asks && !meta.managed_version {
                repository_update_field_checks.push(quote! {
                    if let Some(value) = &input.#field_name {
                        db.ensure_repository_writable_field(
                            None,
                            #entity_name_lit,
                            #graphql_name,
                            #write_policy_key,
                            __gom_existing,
                            Some(value as &(dyn ::std::any::Any + Send + Sync)),
                        ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                    }
                });
            }
            if graphql_update_asks {
                update_runtime_policy_checks.push(quote! {
                    let __gom_field_write_allowed = if !(#graphql_update_condition) {
                        true
                    } else if let Some(val) = &input.#field_name {
                        db.check_field_write(
                            ctx,
                            #entity_name_lit,
                            #graphql_name,
                            #write_policy_key,
                            Some(policy_entity as &(dyn ::std::any::Any + Send + Sync)),
                            Some(val as &(dyn ::std::any::Any + Send + Sync)),
                        ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?
                    } else {
                        true
                    };
                    if !__gom_field_write_allowed {
                        __gom_omitted_fields.insert(#graphql_name);
                        input.#field_name = None;
                    }
                });
                update_bulk_omission_checks.push(quote! {
                    if __gom_omitted_fields.contains(#graphql_name) {
                        input.#field_name = None;
                    }
                });
            }

            if let Some(spatial) = &meta.spatial {
                let spatial_value_expr = if backend == BackendKind::Postgres {
                    format!("ST_SetSRID(ST_GeomFromGeoJSON(?::jsonb), {})", spatial.srid)
                } else {
                    "?".to_string()
                };
                let geometry_type = spatial_geometry_type_tokens(
                    &spatial.geometry_type,
                    proc_macro2::Span::call_site(),
                )?;
                let srid = spatial.srid;
                if is_already_optional {
                    update_field_checks_graphql.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = {}", #db_col_sql, #spatial_value_expr));
                            match val {
                                Some(value) => values.push(
                                    #struct_name::__gom_spatial_sql_value::<::graphql_orm::async_graphql::Error>(
                                        value,
                                        ::graphql_orm::graphql::orm::SpatialColumnDef::geometry(#geometry_type, #srid),
                                    )?
                                ),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::JsonNull),
                            }
                        }
                    });
                    update_field_checks_repo.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = {}", #db_col_sql, #spatial_value_expr));
                            match val {
                                Some(value) => values.push(
                                    #struct_name::__gom_spatial_sql_value::<::graphql_orm::sqlx::Error>(
                                        value,
                                        ::graphql_orm::graphql::orm::SpatialColumnDef::geometry(#geometry_type, #srid),
                                    )?
                                ),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::JsonNull),
                            }
                        }
                    });
                } else {
                    update_field_checks_graphql.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = {}", #db_col_sql, #spatial_value_expr));
                            values.push(
                                #struct_name::__gom_spatial_sql_value::<::graphql_orm::async_graphql::Error>(
                                    val,
                                    ::graphql_orm::graphql::orm::SpatialColumnDef::geometry(#geometry_type, #srid),
                                )?
                            );
                        }
                    });
                    update_field_checks_repo.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = {}", #db_col_sql, #spatial_value_expr));
                            values.push(
                                #struct_name::__gom_spatial_sql_value::<::graphql_orm::sqlx::Error>(
                                    val,
                                    ::graphql_orm::graphql::orm::SpatialColumnDef::geometry(#geometry_type, #srid),
                                )?
                            );
                        }
                    });
                }
            } else if meta.is_boolean_field || is_bool_type(field_type) {
                if is_already_optional {
                    // Option<Option<bool>> case
                    let update_tokens = quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col_sql));
                            match val {
                                Some(b) => values.push(#struct_name::__gom_bool_sql_value(*b)),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::BoolNull),
                            }
                        }
                    };
                    update_field_checks_graphql.push(update_tokens.clone());
                    update_field_checks_repo.push(update_tokens);
                } else {
                    // Option<bool> case
                    let update_tokens = quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col_sql));
                            values.push(#struct_name::__gom_bool_sql_value(*val));
                        }
                    };
                    update_field_checks_graphql.push(update_tokens.clone());
                    update_field_checks_repo.push(update_tokens);
                }
            } else if meta.is_json_field
                || (is_vec_type(field_type) && !is_byte_vec_type(field_type))
            {
                if is_already_optional {
                    update_field_checks_graphql.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col_sql));
                            match val {
                                Some(value) => values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::async_graphql::Error>(value)?),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::JsonNull),
                            }
                        }
                    });
                    update_field_checks_repo.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col_sql));
                            match val {
                                Some(value) => values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::sqlx::Error>(value)?),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::JsonNull),
                            }
                        }
                    });
                } else {
                    update_field_checks_graphql.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col_sql));
                            values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::async_graphql::Error>(val)?);
                        }
                    });
                    update_field_checks_repo.push(quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col_sql));
                            values.push(::graphql_orm::graphql::orm::json_sql_value::<_, ::graphql_orm::sqlx::Error>(val)?);
                        }
                    });
                }
            } else if is_uuid_type(field_type) {
                if is_already_optional {
                    let update_tokens = quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col_sql));
                            match val {
                                Some(v) => values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(*v)),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::UuidNull),
                            }
                        }
                    };
                    update_field_checks_graphql.push(update_tokens.clone());
                    update_field_checks_repo.push(update_tokens);
                } else {
                    let update_tokens = quote! {
                        if let Some(ref val) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col_sql));
                            values.push(::graphql_orm::graphql::orm::SqlValue::Uuid(*val));
                        }
                    };
                    update_field_checks_graphql.push(update_tokens.clone());
                    update_field_checks_repo.push(update_tokens);
                }
            } else if meta.decimal.is_some() {
                let definition = decimal_definition_tokens(&meta);
                let graphql_tokens = if is_already_optional {
                    quote! {
                        if let Some(ref value) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col_sql));
                            match value {
                                Some(value) => values.push(
                                    ::graphql_orm::graphql::orm::decimal_sql_value::<::graphql_orm::async_graphql::Error>(*value, #definition)?
                                ),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::DecimalNull(#definition)),
                            }
                        }
                    }
                } else {
                    quote! {
                        if let Some(value) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col_sql));
                            values.push(
                                ::graphql_orm::graphql::orm::decimal_sql_value::<::graphql_orm::async_graphql::Error>(value, #definition)?
                            );
                        }
                    }
                };
                let repository_tokens = if is_already_optional {
                    quote! {
                        if let Some(ref value) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col_sql));
                            match value {
                                Some(value) => values.push(
                                    ::graphql_orm::graphql::orm::decimal_sql_value::<::graphql_orm::sqlx::Error>(*value, #definition)?
                                ),
                                None => values.push(::graphql_orm::graphql::orm::SqlValue::DecimalNull(#definition)),
                            }
                        }
                    }
                } else {
                    quote! {
                        if let Some(value) = input.#field_name {
                            changed_fields.push(#db_col);
                            set_clauses.push(format!("{} = ?", #db_col_sql));
                            values.push(
                                ::graphql_orm::graphql::orm::decimal_sql_value::<::graphql_orm::sqlx::Error>(value, #definition)?
                            );
                        }
                    }
                };
                update_field_checks_graphql.push(graphql_tokens);
                update_field_checks_repo.push(repository_tokens);
            } else if is_already_optional
                && option_inner_type(field_type)
                    .and_then(type_path_last_ident)
                    .is_some_and(|ident| {
                        matches!(
                            ident.to_string().as_str(),
                            "i8" | "i16"
                                | "i32"
                                | "i64"
                                | "isize"
                                | "u8"
                                | "u16"
                                | "u32"
                                | "u64"
                                | "usize"
                        )
                    })
            {
                let update_tokens = quote! {
                    if let Some(ref val) = input.#field_name {
                        changed_fields.push(#db_col);
                        set_clauses.push(format!("{} = ?", #db_col_sql));
                        match val {
                            Some(v) => values.push(::graphql_orm::graphql::orm::SqlValue::Int((*v) as i64)),
                            None => values.push(::graphql_orm::graphql::orm::SqlValue::IntNull),
                        }
                    }
                };
                update_field_checks_graphql.push(update_tokens.clone());
                update_field_checks_repo.push(update_tokens);
            } else if is_already_optional
                && option_inner_type(field_type)
                    .and_then(type_path_last_ident)
                    .is_some_and(|ident| matches!(ident.to_string().as_str(), "f32" | "f64"))
            {
                let update_tokens = quote! {
                    if let Some(ref val) = input.#field_name {
                        changed_fields.push(#db_col);
                        set_clauses.push(format!("{} = ?", #db_col_sql));
                        match val {
                            Some(v) => values.push(::graphql_orm::graphql::orm::SqlValue::Float((*v).into())),
                            None => values.push(::graphql_orm::graphql::orm::SqlValue::FloatNull),
                        }
                    }
                };
                update_field_checks_graphql.push(update_tokens.clone());
                update_field_checks_repo.push(update_tokens);
            } else if is_already_optional
                && option_inner_type(field_type).is_some_and(is_byte_vec_type)
            {
                let update_tokens = quote! {
                    if let Some(ref val) = input.#field_name {
                        changed_fields.push(#db_col);
                        set_clauses.push(format!("{} = ?", #db_col_sql));
                        match val {
                            Some(v) => values.push(::graphql_orm::graphql::orm::SqlValue::Bytes(v.clone())),
                            None => values.push(::graphql_orm::graphql::orm::SqlValue::BytesNull),
                        }
                    }
                };
                update_field_checks_graphql.push(update_tokens.clone());
                update_field_checks_repo.push(update_tokens);
            } else if is_already_optional {
                // Field type is already Option<T>, update type is Option<Option<T>>
                let value_expr = quote! { v.to_string() };

                let update_tokens = quote! {
                    if let Some(ref val) = input.#field_name {
                        changed_fields.push(#db_col);
                        set_clauses.push(format!("{} = ?", #db_col_sql));
                        match val {
                            Some(v) => values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr)),
                            None => values.push(::graphql_orm::graphql::orm::SqlValue::StringNull),
                        }
                    }
                };
                update_field_checks_graphql.push(update_tokens.clone());
                update_field_checks_repo.push(update_tokens);
            } else if type_path_last_ident(field_type).is_some_and(|ident| {
                matches!(
                    ident.to_string().as_str(),
                    "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize"
                )
            }) {
                let update_tokens = quote! {
                    if let Some(ref val) = input.#field_name {
                        changed_fields.push(#db_col);
                        set_clauses.push(format!("{} = ?", #db_col_sql));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Int((*val) as i64));
                    }
                };
                update_field_checks_graphql.push(update_tokens.clone());
                update_field_checks_repo.push(update_tokens);
            } else if type_path_last_ident(field_type)
                .is_some_and(|ident| matches!(ident.to_string().as_str(), "f32" | "f64"))
            {
                let update_tokens = quote! {
                    if let Some(ref val) = input.#field_name {
                        changed_fields.push(#db_col);
                        set_clauses.push(format!("{} = ?", #db_col_sql));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Float((*val).into()));
                    }
                };
                update_field_checks_graphql.push(update_tokens.clone());
                update_field_checks_repo.push(update_tokens);
            } else if is_byte_vec_type(field_type) {
                let update_tokens = quote! {
                    if let Some(ref val) = input.#field_name {
                        changed_fields.push(#db_col);
                        set_clauses.push(format!("{} = ?", #db_col_sql));
                        values.push(::graphql_orm::graphql::orm::SqlValue::Bytes(val.clone()));
                    }
                };
                update_field_checks_graphql.push(update_tokens.clone());
                update_field_checks_repo.push(update_tokens);
            } else {
                // Field type is T, update type is Option<T>
                let value_expr = quote! { val.to_string() };

                let update_tokens = quote! {
                    if let Some(ref val) = input.#field_name {
                        changed_fields.push(#db_col);
                        set_clauses.push(format!("{} = ?", #db_col_sql));
                        values.push(::graphql_orm::graphql::orm::SqlValue::String(#value_expr));
                    }
                };
                update_field_checks_graphql.push(update_tokens.clone());
                update_field_checks_repo.push(update_tokens);
            }
        }
    }

    // The per-field UPDATE assignments are pure and synchronous. Every generated
    // update path needs the same list, so it is emitted once per entity and
    // called, instead of being spliced into each async body (#677). Field write
    // policy checks are not part of it: they stay inline at each call site.
    ensure_pure_update_assignments(
        "update_field_checks_repo",
        &update_field_checks_repo,
        struct_name.span(),
    )?;
    ensure_pure_update_assignments(
        "update_field_checks_graphql",
        &update_field_checks_graphql,
        struct_name.span(),
    )?;
    // The GraphQL resolver shares the repository helper only when the two lists
    // are token-identical and cannot leave the helper early.
    let update_assignments_shared_with_graphql = update_field_checks_graphql.len()
        == update_field_checks_repo.len()
        && update_field_checks_graphql
            .iter()
            .zip(&update_field_checks_repo)
            .all(|(graphql, repo)| graphql.to_string() == repo.to_string())
        && !update_field_checks_repo.iter().any(|tokens| {
            contains_question_mark(tokens.clone())
                || first_identifier_in(tokens.clone(), &["return"], &[]).is_some()
        });
    let update_assignments_graphql_helper = if update_assignments_shared_with_graphql {
        quote! {}
    } else {
        quote! {
            #[doc(hidden)]
            #[allow(dead_code, unused_imports, unused_variables, clippy::ptr_arg)]
            fn __gom_push_update_assignments_graphql(
                input: &#update_input,
                set_clauses: &mut ::std::vec::Vec<::std::string::String>,
                changed_fields: &mut ::std::vec::Vec<&'static str>,
                values: &mut ::std::vec::Vec<::graphql_orm::graphql::orm::SqlValue>,
            ) -> ::graphql_orm::async_graphql::Result<()> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};
                #(#update_field_checks_graphql)*
                Ok(())
            }
        }
    };
    let update_assignment_helpers = quote! {
        impl #struct_name {
            #[doc(hidden)]
            #[allow(dead_code, unused_imports, unused_variables, clippy::ptr_arg)]
            fn __gom_push_update_assignments(
                input: &#update_input,
                set_clauses: &mut ::std::vec::Vec<::std::string::String>,
                changed_fields: &mut ::std::vec::Vec<&'static str>,
                values: &mut ::std::vec::Vec<::graphql_orm::graphql::orm::SqlValue>,
            ) -> ::graphql_orm::Result<()> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};
                #(#update_field_checks_repo)*
                Ok(())
            }

            #update_assignments_graphql_helper

            /// Asks the repository field write decision for every field a
            /// repository update supplies. Emitted once per entity and called
            /// by every repository update path, before the assignments.
            #[doc(hidden)]
            #[allow(dead_code, unused_variables)]
            async fn __gom_authorize_repository_update_field_writes(
                db: &::graphql_orm::db::Database<#backend_marker>,
                __gom_existing: Option<&(dyn ::std::any::Any + Send + Sync)>,
                input: &#update_input,
            ) -> ::graphql_orm::Result<()> {
                #(#repository_update_field_checks)*
                Ok(())
            }
        }
    };
    let update_assignments_repo_call = quote! {
        #struct_name::__gom_push_update_assignments(
            &input,
            &mut set_clauses,
            &mut changed_fields,
            &mut values,
        )?;
    };
    let update_assignments_graphql_call = if update_assignments_shared_with_graphql {
        // The shared list has no fallible conversion, so this cannot fail today.
        // If it ever does, the error is sanitized like every other database
        // error of the resolver, not rendered through `Display`.
        quote! {
            #struct_name::__gom_push_update_assignments(
                &input,
                &mut set_clauses,
                &mut changed_fields,
                &mut values,
            )
            .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
        }
    } else {
        quote! {
            #struct_name::__gom_push_update_assignments_graphql(
                &input,
                &mut set_clauses,
                &mut changed_fields,
                &mut values,
            )?;
        }
    };

    let has_updated_at_column = fields.iter().any(|field| {
        field.ident.as_ref().is_some_and(|ident| {
            parse_field_metadata(field).is_ok_and(|meta| {
                meta.timestamp_behavior(&ident.to_string())
                    .update_updated_at
            })
        })
    });

    if upsert_config.is_some() {
        if upsert_update_columns.is_empty() && !has_updated_at_column {
            return Err(syn::Error::new(
                struct_name.span(),
                "graphql_entity upsert requires at least one updatable field or an updated_at column",
            ));
        }
    }

    // Build INSERT SQL template
    let insert_sql = if auto_generated_pk {
        let mut columns = vec![pk_column.to_string()];
        columns.extend(insert_default_columns.iter().cloned());
        columns.extend(insert_columns.iter().cloned());
        let mut placeholders = vec!["?".to_string()];
        placeholders.extend(insert_default_exprs.iter().cloned());
        placeholders.extend(insert_value_exprs.iter().cloned());
        format!(
            "INSERT INTO {} ({}) VALUES ({})",
            sql_table_name,
            columns
                .iter()
                .map(|column| backend_render_identifier_path(backend, column))
                .collect::<Vec<_>>()
                .join(", "),
            placeholders.join(", ")
        )
    } else {
        let mut columns = insert_default_columns.clone();
        columns.extend(insert_columns.iter().cloned());
        let mut placeholders = insert_default_exprs.clone();
        placeholders.extend(insert_value_exprs.iter().cloned());
        format!(
            "INSERT INTO {} ({}) VALUES ({})",
            sql_table_name,
            columns
                .iter()
                .map(|column| backend_render_identifier_path(backend, column))
                .collect::<Vec<_>>()
                .join(", "),
            placeholders.join(", ")
        )
    };
    let create_mutation_fields = if auto_generated_pk {
        let mut fields = vec![pk_column.to_string()];
        fields.extend(insert_columns.iter().cloned());
        fields
    } else {
        insert_columns.clone()
    };
    let create_mutation_field_literals: Vec<syn::LitStr> = create_mutation_fields
        .iter()
        .map(|field| syn::LitStr::new(field, struct_name.span()))
        .collect();
    let created_pk_init = if auto_generated_pk {
        if pk_is_uuid {
            quote! {
                let created_pk = ::graphql_orm::uuid::Uuid::new_v4();
            }
        } else {
            quote! {
                let created_pk = ::graphql_orm::uuid::Uuid::new_v4().to_string();
            }
        }
    } else {
        quote! {
            let created_pk = input.#pk_field.clone();
        }
    };
    let prepend_pk_bind = if auto_generated_pk {
        quote! {
            bind_values.push(#created_pk_value);
        }
    } else {
        quote! {}
    };
    let upsert_change_field_literals: Vec<syn::LitStr> = upsert_change_fields
        .iter()
        .map(|field| syn::LitStr::new(field, struct_name.span()))
        .collect();
    let upsert_update_column_literals: Vec<syn::LitStr> = upsert_update_columns
        .iter()
        .map(|field| {
            syn::LitStr::new(
                &backend_render_identifier_path(backend, field),
                struct_name.span(),
            )
        })
        .collect();
    let (upsert_insert_columns, upsert_insert_values): (Vec<String>, Vec<String>) =
        if auto_generated_pk {
            let mut columns = vec![pk_column.to_string()];
            columns.extend(insert_default_columns.iter().cloned());
            columns.extend(insert_columns.iter().cloned());
            let mut values = vec!["?".to_string()];
            values.extend(insert_default_exprs.iter().cloned());
            values.extend(insert_value_exprs.iter().cloned());
            (columns, values)
        } else {
            let mut columns = insert_default_columns.clone();
            columns.extend(insert_columns.iter().cloned());
            let mut values = insert_default_exprs.clone();
            values.extend(insert_value_exprs.iter().cloned());
            (columns, values)
        };
    let upsert_insert_column_literals: Vec<syn::LitStr> = upsert_insert_columns
        .iter()
        .map(|field| {
            syn::LitStr::new(
                &backend_render_identifier_path(backend, field),
                struct_name.span(),
            )
        })
        .collect();
    let upsert_insert_value_literals: Vec<syn::LitStr> = upsert_insert_values
        .iter()
        .map(|field| syn::LitStr::new(field, struct_name.span()))
        .collect();
    let upsert_conflict_column_literals: Vec<syn::LitStr> = upsert_config
        .as_ref()
        .map(|config| {
            config
                .fields
                .iter()
                .map(|field| syn::LitStr::new(&field.db_column, struct_name.span()))
                .collect()
        })
        .unwrap_or_default();

    // Column list for SQL (unused now but kept for reference)
    let column_names: Vec<String> = fields
        .iter()
        .filter_map(|f| {
            let meta = parse_field_metadata(f).ok()?;
            if meta.is_relation || meta.skip_db {
                return None;
            }
            let name = f.ident.as_ref()?.to_string();
            Some(meta.db_column.unwrap_or(name))
        })
        .collect();
    let _columns_str = column_names.join(", ");

    // Generate additional type names
    let edge_type = syn::Ident::new(&format!("{}Edge", struct_name), struct_name.span());
    let connection_type =
        syn::Ident::new(&format!("{}Connection", struct_name), struct_name.span());
    let search_edge_type =
        syn::Ident::new(&format!("{}SearchEdge", struct_name), struct_name.span());
    let search_connection_type = syn::Ident::new(
        &format!("{}SearchConnection", struct_name),
        struct_name.span(),
    );
    let edge_type_str = format!("{}Edge", struct_name);
    let connection_type_str = format!("{}Connection", struct_name);
    let search_edge_type_str = format!("{}SearchEdge", struct_name);
    let search_connection_type_str = format!("{}SearchConnection", struct_name);
    let create_input_str = format!("Create{}Input", struct_name);
    let update_input_str = format!("Update{}Input", struct_name);
    let result_type_str = format!("{}Result", struct_name);
    let changed_event_str = format!("{}ChangedEvent", struct_name);
    let has_search = fields
        .iter()
        .filter_map(|f| parse_field_metadata(f).ok())
        .any(|m| m.search.is_some() || !m.search_json.is_empty() || m.search_relation.is_some());
    if operation_authorizations.contains_key("search") && !has_search {
        return Err(syn::Error::new_spanned(
            struct_name,
            "operation authorization category `search` requires a generated search operation; mark at least one supported field as searchable",
        ));
    }
    if operation_authorizations.contains_key("keyset_list") && keyset_parts.is_none() {
        return Err(syn::Error::new_spanned(
            struct_name,
            "operation authorization category `keyset_list` requires a generated keyset operation; configure graphql_entity keyset = \"field asc|desc, ...\"",
        ));
    }
    let generates_standard_write_surface = backend_writable
        && !schema_policy_read_only
        && !has_composite_primary_key
        && !entity_meta.append_only;
    // A GraphQL input object must declare at least one field. When every
    // create (or update) column is a generated key, a managed timestamp,
    // `skip_input` or `write = false`, the public input would be empty, so the
    // roots that accept it are not generated. The Rust repository inputs keep
    // every repository-writable column, so repository and transaction writes
    // are unaffected. Input-free roots (delete, delete_many) do not depend on
    // writable fields.
    let public_create_input_available = !graphql_create_input_fields.is_empty();
    let public_update_input_available = !graphql_update_input_fields.is_empty();
    let generates_public_create = (entity_meta.append_only || generates_standard_write_surface)
        && public_create_input_available;
    let generates_public_upsert =
        generates_standard_write_surface && graphql_upsert_enabled && public_create_input_available;
    let generates_public_update = generates_standard_write_surface && public_update_input_available;
    let zero_input_reason = |category: &str| -> Option<&'static str> {
        match category {
            "create" | "upsert"
                if (entity_meta.append_only || generates_standard_write_surface)
                    && !public_create_input_available =>
            {
                Some(
                    "the entity has no publicly writable create field (each create column is a generated key, a managed timestamp, skip_input or write = false), so no CreateInput or create/upsert mutation is generated",
                )
            }
            "update" | "update_many"
                if generates_standard_write_surface && !public_update_input_available =>
            {
                Some(
                    "the entity has no publicly writable update field (each non-key column is a managed timestamp, skip_input or write = false), so no UpdateInput or update/update_many mutation is generated",
                )
            }
            _ => None,
        }
    };
    let generated_category_required = |category: &str, generated: bool| -> syn::Result<()> {
        if operation_authorizations.contains_key(category) && !generated {
            let message = match zero_input_reason(category) {
                Some(reason) => format!(
                    "operation authorization category `{category}` requires the entity to generate that GraphQL operation, but {reason}"
                ),
                None => format!(
                    "operation authorization category `{category}` requires the entity to generate that GraphQL operation"
                ),
            };
            return Err(syn::Error::new_spanned(struct_name, message));
        }
        Ok(())
    };
    generated_category_required("create", generates_public_create)?;
    generated_category_required("upsert", generates_public_upsert)?;
    generated_category_required("update", generates_public_update)?;
    generated_category_required("update_many", generates_public_update)?;
    for category in ["create", "upsert", "update", "update_many"] {
        if entity_meta.ai_mutations.contains_key(category)
            && let Some(reason) = zero_input_reason(category)
        {
            return Err(syn::Error::new_spanned(
                struct_name,
                format!(
                    "AI mutation policy `{category}` names an operation that is not generated: {reason}"
                ),
            ));
        }
    }
    generated_category_required("delete", generates_standard_write_surface)?;
    generated_category_required(
        "delete_many",
        generates_standard_write_surface && !entity_meta.dependent_deletes,
    )?;
    if entity_meta.dependent_deletes && entity_meta.ai_mutations.contains_key("delete_many") {
        return Err(syn::Error::new_spanned(
            struct_name,
            "dependent_deletes disables delete_many; remove its AI mutation policy",
        ));
    }
    if let Some(shapes) = &entity_meta.mutation_shapes {
        for shape in shapes {
            let available = match shape.as_str() {
                "create" => generates_public_create,
                "upsert" => generates_public_upsert,
                "update" | "update_many" => generates_public_update,
                _ => generates_standard_write_surface,
            };
            if !available {
                let reason = if let Some(reason) = zero_input_reason(shape) {
                    reason
                } else if entity_meta.append_only {
                    "append_only entities generate only create"
                } else if shape == "upsert" && generates_standard_write_surface {
                    "configure graphql_entity upsert = \"field, ...\" to generate it"
                } else {
                    "this entity does not generate standard GraphQL mutations"
                };
                return Err(syn::Error::new_spanned(
                    struct_name,
                    format!("mutation_shapes selects unavailable {shape} mutation: {reason}"),
                ));
            }
        }
        // A policy for an operation that is not in the schema can never apply,
        // so it is almost certainly a stale declaration.
        for category in operation_authorizations
            .keys()
            .map(String::as_str)
            .chain(entity_meta.ai_mutations.keys().map(String::as_str))
        {
            let is_mutation = matches!(
                category,
                "create" | "upsert" | "update" | "update_many" | "delete" | "delete_many"
            );
            if is_mutation && !shapes.iter().any(|selected| selected == category) {
                return Err(syn::Error::new_spanned(
                    struct_name,
                    format!(
                        "`{category}` is declared for operation authorization or AI mutation policy but mutation_shapes does not select it"
                    ),
                ));
            }
        }
    }
    // Every supported entity profile emits a real Changed subscription,
    // independently of whether it can generate standard GraphQL mutations.
    let resolver_schema_signature = generated_resolver_schema_signature(
        struct_name,
        fields,
        &entity_meta,
        backend,
        graphql_rename_fields,
        serde_rename_all,
        auto_generated_pk,
    )?;
    // Target authority belongs to the relationship resolver. Pool-only eager
    // preloads cannot evaluate current request policies and may fail a parent
    // before its nullable child can report a sanitized traversal error.
    let relation_preload_list = quote! {};
    let relation_preload_search_list = quote! {};
    let relation_preload_single = quote! {};

    let find_by_id_method = if has_composite_primary_key {
        quote! {}
    } else {
        quote! {
            #[allow(clippy::ptr_arg)]
            pub async fn find_by_id(
                db: &::graphql_orm::db::Database<#backend_marker>,
                id: &#pk_type_ty,
            ) -> ::graphql_orm::Result<Option<Self>> {
                Self::find_by_key(db, id).await
            }
        }
    };
    let get_by_id_method = if has_composite_primary_key {
        quote! {}
    } else {
        quote! {
            pub async fn get(pool: &#pool_type, id: &#pk_type_ty) -> ::graphql_orm::Result<Option<Self>> {
                Self::get_by_key(pool, id).await
            }
        }
    };
    let read_repository_key_methods = quote! {
        #find_by_id_method

        pub async fn find_by_key(
            db: &::graphql_orm::db::Database<#backend_marker>,
            key: &#key_type,
        ) -> ::graphql_orm::Result<Option<Self>> {
            db.ensure_entity_access(
                None,
                #entity_name_lit,
                <Self as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
            ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

            // The Repository-surface predicate narrows the SQL, as for
            // repository list reads; the row still passes `can_read_row`.
            let entity = ::graphql_orm::graphql::orm::with_repository_read_visibility(
                db,
                ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                    .where_values(&Self::__gom_key_where_clause(), Self::__gom_key_values(key)),
            )
            .await?
            .fetch_one(db)
            .await?;

            if let Some(ref loaded) = entity {
                if !db.can_read_row(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                    loaded as &(dyn ::std::any::Any + Send + Sync),
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)? {
                    return Ok(None);
                }
                for field in <Self as ::graphql_orm::graphql::orm::Entity>::repository_field_policies() {
                    db.ensure_repository_readable_field(
                        None,
                        #entity_name_lit,
                        field.api_name,
                        field.read_policy,
                        Some(loaded as &(dyn ::std::any::Any + Send + Sync)),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                }
            }

            Ok(entity)
        }

        #get_by_id_method

        pub async fn get_by_key(pool: &#pool_type, key: &#key_type) -> ::graphql_orm::Result<Option<Self>> {
            ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                .where_values(&Self::__gom_key_where_clause(), Self::__gom_key_values(key))
                .fetch_one(pool)
                .await
        }
    };

    // An entity without generated writes still contributes a mutation object,
    // so it carries one hidden placeholder field. The field is described in the
    // operation catalogue like any root field: its authorization requirement is
    // unsatisfiable and its resolver refuses every request.
    let emits_write_placeholder = (has_composite_primary_key || schema_policy_read_only)
        && backend != BackendKind::Mssql
        && !entity_meta.append_only;
    // Named per entity, so several placeholders merge into one mutation root
    // without sharing a coordinate. No leading underscore: some routers drop
    // underscore root fields from the contract they bind descriptors to, and
    // would then refuse this field's descriptor as describing an absent field.
    // An append-only entity without a public create field has no generated
    // write either; its mutation object carries the same placeholder.
    let emits_append_only_write_placeholder = entity_meta.append_only && !generates_public_create;
    let write_placeholder_field = format!("gom{struct_name}WritesUnavailable");
    let write_placeholder_description = "Unavailable placeholder: this entity serves no generated writes, and every request is refused";
    let composite_write_stubs = if emits_write_placeholder {
        quote! {
            #[derive(Default)]
            pub struct #mutations_struct;

            #[::graphql_orm::async_graphql::Object]
            impl #mutations_struct {
                #[doc = #write_placeholder_description]
                #[graphql(name = #write_placeholder_field, visible = false)]
                async fn __gom_composite_key_writes_unavailable(
                    &self,
                ) -> ::graphql_orm::async_graphql::Result<bool> {
                    Err(::graphql_orm::graphql::errors::OrmPublicError::forbidden().into_graphql_error())
                }
            }

        }
    } else {
        quote! {}
    };

    // Generate match arms for searchable fields (used in search_similar)
    let searchable_field_arms: Vec<proc_macro2::TokenStream> = string_filterable_fields
        .iter()
        .map(|(field_name, is_option)| {
            let field_str = field_name.to_string();
            if *is_option {
                quote! {
                    #field_str => entity.#field_name.as_deref(),
                }
            } else {
                quote! {
                    #field_str => Some(entity.#field_name.as_str()),
                }
            }
        })
        .collect();

    let searchable_field_match = if searchable_field_arms.is_empty() {
        quote! { None }
    } else {
        quote! {
            match field {
                #(#searchable_field_arms)*
                _ => None,
            }
        }
    };

    let upsert_result_definition = if upsert_config.is_some() {
        quote! {
            #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
            #[graphql(name = #upsert_result_type_str, rename_fields = #field_case_rule)]
            pub struct #upsert_result_type {
                pub success: bool,
                pub error: Option<String>,
                pub action: Option<::graphql_orm::graphql::orm::ChangeAction>,
                #[graphql(name = #entity_result_field_name)]
                pub entity: Option<#struct_name>,
            }

            impl #upsert_result_type {
                pub fn ok(action: ::graphql_orm::graphql::orm::ChangeAction, entity: #struct_name) -> Self {
                    Self { success: true, error: None, action: Some(action), entity: Some(entity) }
                }

                pub fn err(msg: impl Into<String>) -> Self {
                    Self { success: false, error: Some(msg.into()), action: None, entity: None }
                }
            }
        }
    } else {
        quote! {}
    };

    let upsert_fetch_conditions = upsert_config
        .as_ref()
        .map(|config| {
            config
                .fields
                .iter()
                .enumerate()
                .map(|(index, field)| {
                    let db_column = &field.db_column;
                    let bind_index = index + 1;
                    quote! {
                        format!(
                            "{} = {}",
                            <#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT
                                .quote_identifier_path(#db_column),
                            Self::__gom_placeholder(#bind_index),
                        )
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let upsert_fetch_bind_tokens = upsert_config
        .as_ref()
        .map(|config| {
            config
                .fields
                .iter()
                .map(|field| {
                    let field_name = &field.field_name;
                    push_create_input_sql_value_tokens(
                        backend,
                        struct_name,
                        quote! { input.#field_name.clone() },
                        &field.field_type,
                        &field.meta,
                        quote! { ::graphql_orm::sqlx::Error },
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let upsert_read_lock_hint = if backend == BackendKind::Mssql {
        quote! { " WITH (UPDLOCK, HOLDLOCK)" }
    } else {
        quote! { "" }
    };
    let write_read_lock_suffix = if backend == BackendKind::Postgres {
        quote! { " FOR UPDATE" }
    } else {
        quote! { "" }
    };
    let upsert_sql_expression = if upsert_config.is_some() {
        quote! {
            ::graphql_orm::graphql::orm::build_upsert_sql_with_managed_version(
                #table_name,
                &[#(#upsert_insert_column_literals),*],
                &[#(#upsert_insert_value_literals),*],
                &[#(#upsert_conflict_column_literals),*],
                &[#(#upsert_update_column_literals),*],
                #has_updated_at_column,
                #managed_version_column_option,
            )
        }
    } else {
        quote! { String::new() }
    };
    let composite_upsert_policy_guard = if composite_repository_mutations {
        quote! { Self::__gom_require_repository_write_policy(&db)?; }
    } else {
        quote! {}
    };
    let upsert_helper_methods = if upsert_config.is_some() {
        quote! {
            #[doc(hidden)]
            pub(crate) async fn __gom_fetch_by_upsert_on(
                hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                input: &#create_input,
            ) -> ::graphql_orm::Result<Option<Self>> {
                use ::graphql_orm::graphql::orm::{FromSqlRow, SqlDialect};

                let where_clause = vec![#(#upsert_fetch_conditions),*].join(" AND ");
                let sql = Self::__gom_rebind_sql(
                    &format!(
                        "SELECT {} FROM {}{} WHERE {}{}",
                        <Self as ::graphql_orm::graphql::orm::DatabaseEntity>::column_names().join(", "),
                        #sql_table_name,
                        #upsert_read_lock_hint,
                        where_clause,
                        #write_read_lock_suffix
                    ),
                    1,
                );
                let mut bind_values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                #(#upsert_fetch_bind_tokens)*
                let rows = hook_ctx.fetch_rows(&sql, &bind_values).await?;
                rows.first()
                    .map(<Self as ::graphql_orm::graphql::orm::FromSqlRow<#backend_marker>>::from_row)
                    .transpose()
            }

            #[doc(hidden)]
            async fn __gom_upsert_with_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                input: #create_input,
            ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::UpsertOutcome<Self>> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<::graphql_orm::graphql::orm::UpsertOutcome<Self>> = async {
                hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                hook_ctx.require_state_machine("upsert")?;
                let db = hook_ctx.database().clone();
                #composite_upsert_policy_guard
                let current_entity = Self::__gom_fetch_by_upsert_on(hook_ctx, &input).await?;
                if let Some(current_entity) = current_entity.as_ref() {
                    db.ensure_writable_row(
                        None,
                        #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                        current_entity as &(dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                }
                #(#upsert_policy_checks_repo)*
                <Self as ::graphql_orm::graphql::orm::Entity>::authorize_repository_create_fields(
                    &db,
                    &input as &(dyn ::std::any::Any + Send + Sync),
                ).await?;

                #created_pk_init
                let action = if current_entity.is_some() {
                    ::graphql_orm::graphql::orm::ChangeAction::Updated
                } else {
                    ::graphql_orm::graphql::orm::ChangeAction::Created
                };
                let before_state = current_entity
                    .as_ref()
                    .map(Self::__gom_capture_entity_state)
                    .transpose()?;
                let event_id = current_entity
                    .as_ref()
                    .map(|entity| #pk_id_for_entity)
                    .unwrap_or_else(|| #created_pk_id_string);
                let lookup_input = input.clone();
                let change_input = input.clone();
                let sql = Self::__gom_rebind_sql(&#upsert_sql_expression, 1);
                let mut bind_values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                #prepend_pk_bind
                #(#insert_binds_repo)*
                let mutation_changes = if action == ::graphql_orm::graphql::orm::ChangeAction::Updated {
                    let input = change_input;
                    let mut bind_values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                    #(#upsert_change_binds_repo)*
                    ::graphql_orm::graphql::orm::mutation_changes(&[#(#upsert_change_field_literals),*], &bind_values)
                } else {
                    ::graphql_orm::graphql::orm::mutation_changes(&[#(#create_mutation_field_literals),*], &bind_values)
                };

                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: event_id,
                        changes: mutation_changes.clone(),
                        before_state: before_state.clone(),
                        after_state: None,
                    },
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

                if let Some(current_entity) = current_entity.as_ref() {
                    Self::__gom_require_managed_version_headroom(current_entity)?;
                }
                hook_ctx.execute(&sql, &bind_values).await?;
                let entity = Self::__gom_fetch_by_upsert_on(hook_ctx, &lookup_input).await?
                    .ok_or(::graphql_orm::sqlx::Error::RowNotFound)?;
                #repository_post_image_check
                Self::__gom_require_managed_version_advanced(current_entity.as_ref(), &entity)?;
                let after_state = Some(Self::__gom_capture_entity_state(&entity)?);

                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #pk_id_for_entity,
                        changes: mutation_changes.clone(),
                        before_state,
                        after_state,
                    },
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

                Self::__gom_queue_changed_event(hook_ctx, action, Some(&entity)).await?;

                Ok(::graphql_orm::graphql::orm::UpsertOutcome { action, entity })
            }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }
        }
    } else {
        quote! {}
    };
    let upsert_graphql_method = if generates_public_upsert {
        quote! {
            #[doc = #upsert_operation_description]
            #[graphql(name = #upsert_mutation_name, #upsert_requires_scopes #authenticated_directive)]
            async fn upsert(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #input_arg_name, desc = #input_arg_description)] input: #graphql_create_input,
            ) -> ::graphql_orm::async_graphql::Result<#upsert_result_type> {
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #upsert_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation,
                )?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                let pool = db.pool();
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                let mut input: #create_input = input.into();
                #[allow(unused_mut)]
                let mut __gom_client_supplied: ::std::collections::HashSet<&'static str> =
                    ::std::collections::HashSet::new();
                #(#create_client_supplied)*
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin_with_auth(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                    auth_context.as_ref(),
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                {
                    let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::graphql_in_transaction(
                        ctx,
                        #entity_name_lit,
                        &mut hook_ctx,
                    );
                    db.run_before_upsert_with_context(
                        &mut write_ctx,
                        &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                }

                let current_entity = #struct_name::__gom_fetch_by_upsert_on(&mut hook_ctx, &input)
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                if let Some(current_entity) = current_entity.as_ref() {
                    db.ensure_writable_row(
                        Some(ctx),
                        #entity_name_lit,
                        <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                        current_entity as &(dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                }
                #(#upsert_policy_checks_graphql)*

                #created_pk_init
                let action = if current_entity.is_some() {
                    ::graphql_orm::graphql::orm::ChangeAction::Updated
                } else {
                    ::graphql_orm::graphql::orm::ChangeAction::Created
                };
                let before_state = current_entity
                    .as_ref()
                    .map(#struct_name::__gom_capture_entity_state)
                    .transpose()
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                let event_id = current_entity
                    .as_ref()
                    .map(|entity| #pk_id_for_entity)
                    .unwrap_or_else(|| #created_pk_id_string);
                let lookup_input = input.clone();
                let change_input = input.clone();
                let sql = #struct_name::__gom_rebind_sql(&#upsert_sql_expression, 1);
                let mut bind_values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                #prepend_pk_bind
                #(#insert_binds_graphql)*
                let mutation_changes = if action == ::graphql_orm::graphql::orm::ChangeAction::Updated {
                    let input = change_input;
                    let mut bind_values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                    #(#upsert_change_binds_graphql)*
                    ::graphql_orm::graphql::orm::mutation_changes(&[#(#upsert_change_field_literals),*], &bind_values)
                } else {
                    ::graphql_orm::graphql::orm::mutation_changes(&[#(#create_mutation_field_literals),*], &bind_values)
                };

                hook_ctx.run_mutation_hook(
                    Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: event_id,
                        changes: mutation_changes.clone(),
                        before_state: before_state.clone(),
                        after_state: None,
                    },
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                if let Some(current_entity) = current_entity.as_ref() {
                    #struct_name::__gom_require_managed_version_headroom(current_entity)
                        .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                }
                hook_ctx.execute(&sql, &bind_values)
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                let entity = #struct_name::__gom_fetch_by_upsert_on(&mut hook_ctx, &lookup_input)
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?
                    .ok_or_else(|| ::graphql_orm::async_graphql::Error::new("Entity not found after upsert"))?;
                #graphql_post_image_check
                #struct_name::__gom_require_managed_version_advanced(current_entity.as_ref(), &entity)
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                let after_state = Some(
                    #struct_name::__gom_capture_entity_state(&entity)
                        .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?
                );

                hook_ctx.run_mutation_hook(
                    Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #pk_id_for_entity,
                        changes: mutation_changes.clone(),
                        before_state,
                        after_state,
                    },
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                #struct_name::__gom_queue_changed_event(&mut hook_ctx, action, Some(&entity))
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                hook_ctx
                    .commit_and_emit()
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                match action {
                    ::graphql_orm::graphql::orm::ChangeAction::Created => {
                        #notify_on_created
                    }
                    ::graphql_orm::graphql::orm::ChangeAction::Updated => {
                        #notify_on_updated
                    }
                    ::graphql_orm::graphql::orm::ChangeAction::Deleted => {}
                }

                let mut result = #upsert_result_type::ok(action, entity);
                #mutation_output_release
                Ok(result)
            }
        }
    } else {
        quote! {}
    };
    let upsert_repository_method = if upsert_config.is_some() {
        quote! {
            pub async fn upsert(
                db: &::graphql_orm::db::Database<#backend_marker>,
                input: #create_input,
            ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::UpsertOutcome<Self>> {
                db.ensure_entity_access(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                ).await?;
                let mut input = input;
                {
                    let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::repository_in_transaction(
                        #entity_name_lit,
                        &mut hook_ctx,
                    );
                    db.run_before_upsert_with_context(
                        &mut write_ctx,
                        &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                }
                let outcome = Self::__gom_upsert_with_mutation_context(&mut hook_ctx, input).await?;
                hook_ctx.commit_and_emit().await?;
                Ok(outcome)
            }

            /// Upsert multiple entity records transactionally using the configured upsert key.
            pub async fn upsert_many<I>(
                db: &::graphql_orm::db::Database<#backend_marker>,
                inputs: I,
            ) -> ::graphql_orm::Result<Vec<::graphql_orm::graphql::orm::UpsertOutcome<Self>>>
            where
                I: IntoIterator<Item = #create_input>,
            {
                db.ensure_entity_access(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

                let inputs: Vec<#create_input> = inputs.into_iter().collect();
                if inputs.is_empty() {
                    return Ok(Vec::new());
                }

                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                ).await?;
                let mut outcomes = Vec::with_capacity(inputs.len());
                for mut input in inputs {
                    {
                        let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::repository_in_transaction(
                            #entity_name_lit,
                            &mut hook_ctx,
                        );
                        db.run_before_upsert_with_context(
                            &mut write_ctx,
                            &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                        ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    }
                    outcomes.push(Self::__gom_upsert_with_mutation_context(&mut hook_ctx, input).await?);
                }
                hook_ctx.commit_and_emit().await?;
                Ok(outcomes)
            }
        }
    } else {
        quote! {}
    };
    let upsert_trait_impl = if upsert_config.is_some() {
        quote! {
            impl ::graphql_orm::graphql::orm::MutationContextUpsert<#backend_marker> for #struct_name {
                type UpsertInput = #create_input;

                fn upsert_in_mutation_context<'a>(
                    hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    input: Self::UpsertInput,
                ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<::graphql_orm::graphql::orm::UpsertOutcome<Self>>> {
                    Box::pin(async move {
                        hook_ctx.require_state_machine("upsert")?;
                        let db = hook_ctx.database().clone();
                        let mut input = input;
                        let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::internal(
                            #entity_name_lit,
                            hook_ctx,
                        );
                        db.run_before_upsert_with_context(
                            &mut write_ctx,
                            &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                        ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                        Self::__gom_upsert_with_mutation_context(hook_ctx, input).await
                    })
                }
            }
        }
    } else {
        quote! {}
    };

    let cas_begin_savepoint = if backend == BackendKind::Mssql {
        quote! { hook_ctx.execute("SAVE TRANSACTION graphql_orm_cas", &[]).await?; }
    } else {
        quote! { hook_ctx.execute("SAVEPOINT graphql_orm_cas", &[]).await?; }
    };
    let cas_rollback_savepoint = if backend == BackendKind::Mssql {
        quote! { hook_ctx.execute("ROLLBACK TRANSACTION graphql_orm_cas", &[]).await?; }
    } else {
        quote! {
            hook_ctx.execute("ROLLBACK TO SAVEPOINT graphql_orm_cas", &[]).await?;
            hook_ctx.execute("RELEASE SAVEPOINT graphql_orm_cas", &[]).await?;
        }
    };
    // Error-path rollback: the original error is returned, not a rollback failure.
    let cas_abort_savepoint = if backend == BackendKind::Mssql {
        quote! { let _ = hook_ctx.execute("ROLLBACK TRANSACTION graphql_orm_cas", &[]).await; }
    } else {
        quote! {
            let _ = hook_ctx.execute("ROLLBACK TO SAVEPOINT graphql_orm_cas", &[]).await;
            let _ = hook_ctx.execute("RELEASE SAVEPOINT graphql_orm_cas", &[]).await;
        }
    };
    let cas_release_savepoint = if backend == BackendKind::Mssql {
        quote! {}
    } else {
        quote! { hook_ctx.execute("RELEASE SAVEPOINT graphql_orm_cas", &[]).await?; }
    };
    let cas_update_sql = if backend == BackendKind::Mssql {
        quote! {
            {
                // SQL Server rejects `OUTPUT` without `INTO` on a table with an
                // enabled trigger. Read the row back in the same batch instead:
                // the update holds its exclusive lock until the transaction
                // ends, and `@@ROWCOUNT` excludes statements run by triggers.
                // The read-back also requires the version this swap writes, so
                // an `INSTEAD OF` trigger that suppresses or rewrites the update
                // yields a conflict rather than an unchanged row.
                let output_columns = <Self as ::graphql_orm::graphql::orm::DatabaseSchema>::columns()
                    .iter()
                    .map(|column| {
                        let quoted = dialect.quote_identifier_path(column.name);
                        if column.decimal.is_some_and(|definition| definition.is_mssql_money()) {
                            format!("CAST({quoted} AS DECIMAL(19,4)) AS {quoted}")
                        } else {
                            quoted
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let quoted_table = dialect.quote_identifier_path(#table_name);
                let update = format!(
                    "UPDATE {quoted_table} SET {} WHERE {}",
                    set_clauses.join(", "),
                    predicates.join(" AND "),
                );
                match expected_version.checked_add(1) {
                    Some(written_version) => {
                        // Placeholders are numbered in order, so the read-back
                        // binds the key and the written version after the update's.
                        values.push(#pk_bind_value_ref);
                        values.push(SqlValue::Int(written_version));
                        format!(
                            "{update}; IF @@ROWCOUNT = 1 SELECT {output_columns} FROM {quoted_table} \
                             WHERE {} = ? AND {quoted_version_column} = ?",
                            dialect.quote_identifier_path(Self::PRIMARY_KEY),
                        )
                    }
                    // `version < i64::MAX` already makes the update match nothing.
                    None => update,
                }
            }
        }
    } else {
        quote! {
            {
                format!(
                    "UPDATE {} SET {} WHERE {} RETURNING {}",
                    dialect.quote_identifier_path(#table_name),
                    set_clauses.join(", "),
                    predicates.join(" AND "),
                    <Self as DatabaseEntity>::column_names()
                        .iter()
                        .map(|column| dialect.quote_identifier_path(column))
                        .collect::<Vec<_>>()
                        .join(", "),
                )
            }
        }
    };

    let (cas_helper_methods, cas_trait_impl) = if let Some((_version_ident, _, version_column)) =
        &version_field
    {
        let version_column = version_column.clone();
        (
            quote! {
                #[doc(hidden)]
                async fn __gom_compare_and_swap_with_mutation_context<'a>(
                    hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    id: &#pk_type_ty,
                    expected_version: i64,
                    expected: #where_input,
                    mut input: #update_input,
                ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::ConditionalUpdateOutcome<Self>> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<::graphql_orm::graphql::orm::ConditionalUpdateOutcome<Self>> = async {
                    hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, EntityQuery, FromSqlRow, OrmBackend, SqlDialect, SqlValue};

                    #cas_begin_savepoint
                    // Any failure after the savepoint opens undoes the swap and closes it.
                    let __gom_cas: ::graphql_orm::Result<::graphql_orm::graphql::orm::ConditionalUpdateOutcome<Self>> = async {

                    let current_entity = Self::__gom_fetch_by_id_on(hook_ctx, id).await?;
                    let db = hook_ctx.database().clone();
                    if let Some(ref current) = current_entity {
                        db.ensure_writable_row(
                            None,
                            #entity_name_lit,
                            <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                            ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                            current as &(dyn ::std::any::Any + Send + Sync),
                        ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    }
                    let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::internal(
                        #entity_name_lit,
                        hook_ctx,
                    );
                    db.run_before_update_with_context(
                        &mut write_ctx,
                        current_entity.as_ref().map(|entity| entity as &(dyn ::std::any::Any + Send + Sync)),
                        &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    <Self as ::graphql_orm::graphql::orm::Entity>::authorize_repository_update_fields(
                        &db,
                        current_entity.as_ref().map(|entity| entity as &(dyn ::std::any::Any + Send + Sync)),
                        &input as &(dyn ::std::any::Any + Send + Sync),
                    ).await?;
                    Self::__gom_authorize_repository_update_field_writes(
                        &db,
                        current_entity.as_ref().map(|entity| entity as &(dyn ::std::any::Any + Send + Sync)),
                        &input,
                    ).await?;

                    let dialect = <#backend_marker as OrmBackend>::DIALECT;
                    let quoted_version_column = dialect.quote_identifier_path(#version_column);
                    let mut set_clauses = vec![format!("{quoted_version_column} = {quoted_version_column} + 1")];
                    let mut changed_fields = vec![#version_column];
                    let mut values: Vec<SqlValue> = Vec::new();
                    #update_assignments_repo_call
                    for (clause, column) in set_clauses.iter_mut().zip(changed_fields.iter()) {
                        let rhs = clause.split_once('=').map(|(_, rhs)| rhs.trim()).unwrap_or("?");
                        *clause = format!("{} = {rhs}", dialect.quote_identifier_path(column));
                    }
                    if #has_updated_at_column {
                        set_clauses.push(format!("updated_at = {}", Self::__gom_current_updated_at_expr()));
                    }
                    let before_state = current_entity
                        .as_ref()
                        .map(Self::__gom_capture_entity_state)
                        .transpose()?;
                    let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                    let expected_clause = if expected.is_empty() {
                        None
                    } else {
                        let query = EntityQuery::<Self, #backend_marker>::new().filter(&expected);
                        let (delete_sql, expected_values) = query.try_build_delete_sql()?;
                        let clause = delete_sql.split_once(" WHERE ")
                            .map(|(_, clause)| clause.to_string())
                            .ok_or_else(|| Self::__gom_runtime_error("expected predicates produced empty SQL"))?;
                        Some((clause, expected_values))
                    };
                    let pk_placeholder = Self::__gom_placeholder(values.len() + 1);
                    values.push(#pk_bind_value_ref);
                    let version_placeholder = Self::__gom_placeholder(values.len() + 1);
                    values.push(SqlValue::Int(expected_version));
                    let mut predicates = vec![
                        format!("{} = {}", dialect.quote_identifier_path(Self::PRIMARY_KEY), pk_placeholder),
                        format!("{} = {}", quoted_version_column, version_placeholder),
                        format!("{} < {}", quoted_version_column, i64::MAX),
                    ];
                    if let Some((clause, expected_values)) = expected_clause {
                        predicates.push(Self::__gom_rebind_sql(&clause, values.len() + 1));
                        values.extend(expected_values);
                    }
                    let sql = Self::__gom_rebind_sql(&(#cas_update_sql), 1);
                    let rows = hook_ctx.fetch_rows(&sql, &values,).await?;
                    if !rows.is_empty() {
                        hook_ctx.__note_write();
                    }
                    if rows.len() > 1 {
                        return Err(Self::__gom_runtime_error("conditional update affected more than one row"));
                    }
                    let Some(row) = rows.first() else {
                        #cas_rollback_savepoint
                        return if Self::__gom_fetch_by_id_on(hook_ctx, id).await?.is_some() {
                            Ok(::graphql_orm::graphql::orm::ConditionalUpdateOutcome::Conflict)
                        } else {
                            Ok(::graphql_orm::graphql::orm::ConditionalUpdateOutcome::NotFound)
                        };
                    };
                    let entity = <Self as FromSqlRow<#backend_marker>>::from_row(row)?;
                    if let Err(error) = hook_ctx.__ensure_written_row(
                        None,
                        #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                        &entity as &(dyn ::std::any::Any + Send + Sync),
                    ).await {
                        // The savepoint is undone below; the context also stays uncommittable.
                        return Err(::graphql_orm::graphql::errors::sqlx_error_from_graphql(error));
                    }
                    hook_ctx.run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: #pk_id_for_entity,
                            changes: mutation_changes.clone(),
                            before_state: before_state.clone(),
                            after_state: None,
                        },
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    hook_ctx.run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: #pk_id_for_entity,
                            changes: mutation_changes,
                            before_state,
                            after_state: Some(Self::__gom_capture_entity_state(&entity)?),
                        },
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    Self::__gom_queue_changed_event(
                        hook_ctx,
                        ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        Some(&entity),
                    ).await?;
                    #cas_release_savepoint
                    Ok(::graphql_orm::graphql::orm::ConditionalUpdateOutcome::Updated(entity))
                }.await;
                    if __gom_cas.is_err() {
                        #cas_abort_savepoint
                    }
                    __gom_cas
                    }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }

                /// Atomically update this versioned entity when all typed expectations match.
                #[allow(clippy::ptr_arg)]
                pub async fn compare_and_swap(
                    db: &::graphql_orm::db::Database<#backend_marker>,
                    id: &#pk_type_ty,
                    expected_version: i64,
                    expected: #where_input,
                    input: #update_input,
                ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::ConditionalUpdateOutcome<Self>> {
                    db.ensure_entity_access(
                        None,
                        #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::Default,
                ).await?;
                    let outcome = Self::__gom_compare_and_swap_with_mutation_context(
                        &mut hook_ctx, id, expected_version, expected, input,
                    ).await?;
                    hook_ctx.commit_and_emit().await?;
                    Ok(outcome)
                }
            },
            quote! {
                impl ::graphql_orm::graphql::orm::MutationContextCompareAndSwap<#backend_marker> for #struct_name {
                    type Id = #pk_type_ty;
                    type Expected = #where_input;
                    type UpdateInput = #update_input;

                    fn compare_and_swap_in_mutation_context<'a>(
                        hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                        id: &'a Self::Id,
                        expected_version: i64,
                        expected: Self::Expected,
                        input: Self::UpdateInput,
                    ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<::graphql_orm::graphql::orm::ConditionalUpdateOutcome<Self>>> {
                        Box::pin(async move {
                            Self::__gom_compare_and_swap_with_mutation_context(
                                hook_ctx, id, expected_version, expected, input,
                            ).await
                        })
                    }
                }
            },
        )
    } else {
        (quote! {}, quote! {})
    };

    let scan_connection_type = quote::format_ident!("{}AuthorizedScanPage", struct_name);
    let scan_query_name = apply_graphql_case(&format!("{}Scan", plural_name), resolver_case);
    let keyset_order_input = quote::format_ident!("{}KeysetOrderByInput", struct_name);
    let keyset_order_field = quote::format_ident!("{}OrderField", struct_name);
    let runtime_keyset_available = primary_key_fields.iter().all(|field| {
        field.meta.graphql_readable()
            && !field.meta.graphql_skip
            && field.meta.transform_read.is_none()
            && runtime_keyset_value(&field.field_type).is_some()
    });
    let keyset_generated = if let Some(parts) = &keyset_parts {
        let mut runtime_fields = Vec::new();
        let mut runtime_metadata = Vec::new();
        let mut runtime_values = Vec::new();
        let mut runtime_orders = Vec::new();
        let mut runtime_ordered_arms = Vec::new();
        let mut primary_columns = Vec::new();
        for field in fields {
            let meta = parse_field_metadata(field)?;
            let ident = field.ident.as_ref().expect("named field");
            let is_primary_key = primary_key_fields
                .iter()
                .any(|key| &key.field_name == ident);
            if meta.is_relation
                || meta.skip_db
                || !meta.graphql_readable()
                || meta.graphql_skip
                || meta.transform_read.is_some()
                || !(is_primary_key || (meta.sortable && meta.order))
            {
                continue;
            }
            let Some(value_tokens) = runtime_keyset_value(&field.ty) else {
                continue;
            };
            let rust_name = ident.to_string();
            let graphql_name =
                graphql_field_name(&meta, &rust_name, graphql_rename_fields, serde_rename_all);
            let column = backend_render_identifier_path(
                backend,
                meta.db_column.as_deref().unwrap_or(&rust_name),
            );
            let policy = match meta.read_policy.as_deref() {
                Some(policy) => quote! { Some(#policy) },
                None => quote! { None },
            };
            runtime_fields.push(quote! {
                #[graphql(name = #graphql_name)]
                pub #ident: Option<::graphql_orm::graphql::orm::OrderDirection>,
            });
            runtime_metadata.push(quote! { ::graphql_orm::graphql::orm::KeysetField {
                column: #column, api_name: #graphql_name, read_policy: #policy,
            } });
            runtime_values
                .push(quote! { #column => { let value = &self.#ident; Some(#value_tokens) } });
            runtime_orders.push(quote! {
                if let Some(direction) = item.#ident {
                    selected += 1;
                    columns.push(match direction {
                        ::graphql_orm::graphql::orm::OrderDirection::Asc => ::graphql_orm::graphql::orm::KeysetOrderColumn::asc(#column),
                        ::graphql_orm::graphql::orm::OrderDirection::Desc => ::graphql_orm::graphql::orm::KeysetOrderColumn::desc(#column),
                    });
                }
            });
            // The typed ordered form addresses the fields that are also
            // generated order-input fields.
            if meta.sortable && meta.order {
                let variant = crate::entity::order_field_variant(
                    &crate::entity::rust_ident_from_graphql_name(&graphql_name, ident.span()),
                );
                runtime_ordered_arms.push(quote! {
                    #keyset_order_field::#variant => {
                        let mut order = Self::default();
                        order.#ident = Some(direction);
                        order
                    }
                });
            }
            if is_primary_key {
                primary_columns.push(column);
            }
        }
        let mut order_tokens = Vec::new();
        let mut cursor_tokens = Vec::new();
        let mut cursor_field_metadata = Vec::new();
        let mut fingerprint_parts = Vec::new();
        for (field, ty, column, ascending, nulls_first) in parts {
            let quoted_column = backend_render_identifier_path(backend, column);
            let source = fields
                .iter()
                .find(|candidate| candidate.ident.as_ref() == Some(field))
                .expect("keyset field was resolved during parsing");
            let meta = parse_field_metadata(source)?;
            let api_name = graphql_field_name(
                &meta,
                &field.to_string(),
                graphql_rename_fields,
                serde_rename_all,
            );
            let graphql_readable =
                meta.graphql_readable() && !meta.graphql_skip && meta.transform_read.is_none();
            let policy = match meta.read_policy.as_deref() {
                Some(policy) => quote! { Some(#policy) },
                None => quote! { None },
            };
            cursor_field_metadata.push(quote! { ::graphql_orm::graphql::orm::KeysetCursorField {
                column: #quoted_column, api_name: #api_name, read_policy: #policy, graphql_readable: #graphql_readable,
            } });
            let direction = if *ascending {
                quote! { ::graphql_orm::graphql::orm::KeysetOrderColumn::asc(#quoted_column) }
            } else {
                quote! { ::graphql_orm::graphql::orm::KeysetOrderColumn::desc(#quoted_column) }
            };
            let order = if *nulls_first {
                quote! { #direction.nulls_first() }
            } else {
                direction
            };
            order_tokens.push(order);
            fingerprint_parts.push(format!(
                "{}:{}:{}",
                column,
                if *ascending { "asc" } else { "desc" },
                if *nulls_first {
                    "nulls_first"
                } else {
                    "nulls_last"
                }
            ));
            let (inner, optional) = option_inner_type(ty)
                .map(|inner| (inner, true))
                .unwrap_or((ty, false));
            let type_name = quote! { #inner }.to_string().replace(' ', "");
            let value = if is_string_type(inner) {
                quote! { ::graphql_orm::graphql::pagination::KeysetValue::String(value.clone()) }
            } else if is_byte_vec_type(inner) {
                quote! { ::graphql_orm::graphql::pagination::KeysetValue::Bytes(value.clone()) }
            } else if is_uuid_type(inner) {
                quote! { ::graphql_orm::graphql::pagination::KeysetValue::Uuid(value.to_string()) }
            } else if is_bool_type(inner) {
                quote! { ::graphql_orm::graphql::pagination::KeysetValue::Bool(*value) }
            } else if matches!(type_name.as_str(), "f32" | "f64") {
                quote! { ::graphql_orm::graphql::pagination::KeysetValue::Float(*value as f64) }
            } else if matches!(
                type_name.as_str(),
                "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32"
            ) {
                quote! { ::graphql_orm::graphql::pagination::KeysetValue::Int(*value as i64) }
            } else {
                return Err(syn::Error::new_spanned(
                    ty,
                    "keyset fields support String, bytes, UUID, bool, integer, and float scalar types",
                ));
            };
            cursor_tokens.push(if optional {
                quote! {
                    match entity.#field.as_ref() {
                        Some(value) => #value,
                        None => ::graphql_orm::graphql::pagination::KeysetValue::Null,
                    }
                }
            } else {
                quote! {{ let value = &entity.#field; #value }}
            });
        }
        let fingerprint = format!("v1:{}", fingerprint_parts.join(","));
        let runtime_order_definitions = if runtime_keyset_available {
            quote! {
                    /// Scalar-only runtime keyset order. Each list entry must select
                    /// exactly one field; NULL values sort last in either direction.
                    #[derive(::graphql_orm::async_graphql::InputObject, Default, Clone, Debug)]
                    pub struct #keyset_order_input { #(#runtime_fields)* }
                    impl #keyset_order_input {
                        /// Build an ordered keyset sort from typed `(field, direction)`
                        /// pairs: the first pair is the primary sort key, and the cursor's
                        /// key order follows the list. The complete primary key is still
                        /// appended as the tiebreak. A field that cannot be a keyset key
                        /// is refused with `INVALID_INPUT`.
                        #[allow(unreachable_patterns)]
                        pub fn ordered(
                            fields: impl IntoIterator<Item = (#keyset_order_field, ::graphql_orm::graphql::orm::OrderDirection)>,
                        ) -> ::std::result::Result<Vec<Self>, ::graphql_orm::graphql::errors::OrmPublicError> {
                            fields
                                .into_iter()
                                .map(|(field, direction)| {
                                    Ok(match field {
                                        #(#runtime_ordered_arms)*
                                        _ => {
                                            return Err(::graphql_orm::graphql::errors::OrmPublicError::with_message(
                                                ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                                                "this field cannot be a keyset sort key",
                                            ));
                                        }
                                    })
                                })
                                .collect()
                        }

                        fn __gom_columns(items: Vec<Self>) -> ::graphql_orm::async_graphql::Result<Vec<::graphql_orm::graphql::orm::KeysetOrderColumn>> {
                            let mut columns = Vec::new();
                            for item in items {
                                let mut selected = 0;
                                #(#runtime_orders)*
                                if selected != 1 {
                                    return Err(::graphql_orm::graphql::errors::OrmPublicError::new(
                                        ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput
                                    ).into_graphql_error());
                                }
                            }
                            Ok(columns)
                        }
                    }

            }
        } else {
            quote! {}
        };
        let runtime_order_trait_items = if runtime_keyset_available {
            quote! {
                        const KEYSET_FIELDS: &'static [::graphql_orm::graphql::orm::KeysetField] = &[#(#runtime_metadata),*];
                        const KEYSET_PRIMARY_COLUMNS: &'static [&'static str] = &[#(#primary_columns),*];
                        fn keyset_value_for_column(&self, column: &str) -> Option<::graphql_orm::graphql::pagination::KeysetValue> {
                            match column { #(#runtime_values),*, _ => None }
                        }

            }
        } else {
            quote! {}
        };
        let runtime_order_repository_methods = if runtime_keyset_available {
            quote! {
                    /// Fetch a bounded page with optional scalar ordering and a complete primary-key suffix.
                    pub async fn keyset_page_ordered(
                        db: &::graphql_orm::db::Database<#backend_marker>,
                        filter: #where_input,
                        order_by: Option<Vec<#keyset_order_input>>,
                        page: ::graphql_orm::graphql::pagination::KeysetPageInput,
                    ) -> ::graphql_orm::async_graphql::Result<::graphql_orm::graphql::pagination::Connection<Self>> {
                        let order = order_by.map(#keyset_order_input::__gom_columns).transpose()?;
                        ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                            .filter_with_entity_matching(&filter)
                            .fetch_authorized_keyset_ordered(db, None, None, page, order).await
                    }

                    /// Fetch a forward or backward page with optional scalar ordering.
                    pub async fn keyset_connection_page_ordered(
                        db: &::graphql_orm::db::Database<#backend_marker>,
                        filter: #where_input,
                        order_by: Option<Vec<#keyset_order_input>>,
                        page: ::graphql_orm::graphql::pagination::KeysetConnectionInput,
                    ) -> ::graphql_orm::async_graphql::Result<::graphql_orm::graphql::pagination::Connection<Self>> {
                        let order = order_by.map(#keyset_order_input::__gom_columns).transpose()?;
                        ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                            .filter_with_entity_matching(&filter)
                            .fetch_authorized_keyset_connection_with_order(db, None, None, page, order).await
                    }

            }
        } else {
            quote! {}
        };
        let definitions = quote! {
                #[derive(::graphql_orm::async_graphql::SimpleObject, Clone, Debug)]
                #[graphql(rename_fields = #field_case_rule)]
                pub struct #scan_connection_type {
                    pub nodes: Vec<#struct_name>,
                    pub page_info: ::graphql_orm::graphql::orm::ScanPageInfo,
                }

                #runtime_order_definitions

                impl ::graphql_orm::graphql::orm::AuthorizedKeysetEntity for #struct_name {
                    #runtime_order_trait_items
                    const KEYSET_CURSOR_FIELDS: &'static [::graphql_orm::graphql::orm::KeysetCursorField] = &[#(#cursor_field_metadata),*];
                    const KEYSET_ORDER: &'static [::graphql_orm::graphql::orm::KeysetOrderColumn] = &[#(#order_tokens),*];
                    const KEYSET_FINGERPRINT: &'static str = #fingerprint;
                    fn keyset_values(&self) -> Vec<::graphql_orm::graphql::pagination::KeysetValue> {
                        let entity = self;
                        vec![#(#cursor_tokens),*]
                    }
                }

        };
        (
            quote! {
                /// Scan bounded batches with request-independent repository policies.
                /// Install AuthorizedScanConfig on the database to opt in.
                pub async fn authorized_scan(
                    db: &::graphql_orm::db::Database<#backend_marker>,
                    filter: #where_input,
                    page: ::graphql_orm::graphql::orm::AuthorizedScanInput,
                ) -> ::graphql_orm::async_graphql::Result<::graphql_orm::graphql::orm::AuthorizedScanPage<Self>> {
                    ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                        .filter_with_entity_matching(&filter)
                        .fetch_authorized_scan(db, None, None, page).await
                }

                #runtime_order_repository_methods

                /// Fetch a bounded, stable keyset page using the entity's configured order.
                pub async fn keyset_page(
                    db: &::graphql_orm::db::Database<#backend_marker>,
                    filter: #where_input,
                    page: ::graphql_orm::graphql::pagination::KeysetPageInput,
                ) -> Result<::graphql_orm::graphql::pagination::Connection<Self>, ::graphql_orm::graphql::errors::OrmPublicError> {
                    ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                        .filter_with_entity_matching(&filter)
                        .fetch_authorized_keyset(db, None, None, page).await
                        .map_err(|error| ::graphql_orm::graphql::errors::OrmPublicError::from(::graphql_orm::graphql::errors::sqlx_error_from_graphql(error)))
                }

                /// Fetch a bounded forward or backward keyset connection using
                /// the entity's configured canonical order.
                pub async fn keyset_connection_page(
                    db: &::graphql_orm::db::Database<#backend_marker>,
                    filter: #where_input,
                    page: ::graphql_orm::graphql::pagination::KeysetConnectionInput,
                ) -> Result<::graphql_orm::graphql::pagination::Connection<Self>, ::graphql_orm::graphql::errors::OrmPublicError> {
                    ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                        .filter_with_entity_matching(&filter)
                        .fetch_authorized_keyset_connection(db, None, None, page).await
                        .map_err(|error| ::graphql_orm::graphql::errors::OrmPublicError::from(::graphql_orm::graphql::errors::sqlx_error_from_graphql(error)))
                }
            },
            quote! {
                impl ::graphql_orm::graphql::orm::MutationContextKeysetPage<#backend_marker> for #struct_name {
                    type Filter = #where_input;

                    fn keyset_page_in_mutation_context<'a>(
                        hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                        filter: Self::Filter,
                        page: ::graphql_orm::graphql::pagination::KeysetPageInput,
                    ) -> ::graphql_orm::futures::future::BoxFuture<'a, Result<::graphql_orm::graphql::pagination::Connection<Self>, ::graphql_orm::graphql::errors::OrmPublicError>> {
                        Box::pin(async move {
                            use ::graphql_orm::graphql::orm::DatabaseFilter;
                            const ORDER: &[::graphql_orm::graphql::orm::KeysetOrderColumn] = &[
                                #(#order_tokens),*
                            ];
                            const FINGERPRINT: &str = #fingerprint;
                            let bound_fingerprint = ::graphql_orm::graphql::orm::bind_keyset_fingerprint(
                                Some(<Self as ::graphql_orm::graphql::orm::Entity>::entity_name()),
                                FINGERPRINT,
                                &filter.keyset_filter_identity().into_iter().collect::<Vec<_>>(),
                            );
                            if filter.requires_in_memory_filtering(<#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT) {
                                return Err(::graphql_orm::graphql::errors::OrmPublicError::new(
                                    ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                                ));
                            }
                            let config = hook_ctx.database().pagination_config();
                            let resolved = config.resolve_page(
                                Some(&::graphql_orm::graphql::orm::PageInput { limit: page.limit, offset: Some(0) }),
                                true,
                            );
                            let limit = resolved.limit.unwrap_or(::graphql_orm::graphql::orm::PaginationConfig::DEFAULT_LIMIT).max(0);
                            let mut query = ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new().filter(&filter);
                            let total_count = if page.include_total_count {
                                Some(query.clone().count_in_transaction(hook_ctx).await.map_err(::graphql_orm::graphql::errors::OrmPublicError::from)?)
                            } else {
                                None
                            };
                            if let Some(cursor) = page.after.as_deref() {
                                let values = ::graphql_orm::graphql::pagination::decode_keyset_cursor(cursor, &bound_fingerprint, ORDER.len())?;
                                let (predicate, binds) = ::graphql_orm::graphql::orm::render_keyset_after(
                                    <#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT,
                                    ORDER,
                                    &values,
                                    query.values.len() + 1,
                                )?;
                                query = query.where_values(&predicate, binds);
                            }
                            query.order_clauses = ORDER.iter().map(|column| column.order_sql()).collect();
                            query = query.paginate(&::graphql_orm::graphql::orm::PageInput {
                                limit: Some(limit.saturating_add(1)),
                                offset: Some(0),
                            });
                            let mut entities = query.fetch_all_in_transaction(hook_ctx).await.map_err(::graphql_orm::graphql::errors::OrmPublicError::from)?;
                            let has_next_page = entities.len() > limit as usize;
                            if has_next_page { entities.truncate(limit as usize); }
                            let edges = entities
                                .into_iter()
                                .map(|entity| {
                                    let values = vec![#(#cursor_tokens),*];
                                    ::graphql_orm::graphql::pagination::Edge {
                                        cursor: ::graphql_orm::graphql::pagination::encode_keyset_cursor(&bound_fingerprint, values),
                                        node: entity,
                                    }
                                })
                                .collect::<Vec<_>>();
                            Ok(::graphql_orm::graphql::pagination::Connection {
                                page_info: ::graphql_orm::graphql::pagination::PageInfo {
                                    has_next_page,
                                    has_previous_page: page.after.is_some(),
                                    start_cursor: edges.first().map(|edge| edge.cursor.clone()),
                                    end_cursor: edges.last().map(|edge| edge.cursor.clone()),
                                    total_count,
                                },
                                edges,
                            })
                        })
                    }
                }

                impl ::graphql_orm::graphql::orm::MutationContextKeysetConnectionPage<#backend_marker> for #struct_name {
                    type Filter = #where_input;

                    fn keyset_connection_page_in_mutation_context<'a>(
                        hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                        filter: Self::Filter,
                        page: ::graphql_orm::graphql::pagination::KeysetConnectionInput,
                    ) -> ::graphql_orm::futures::future::BoxFuture<'a, Result<::graphql_orm::graphql::pagination::Connection<Self>, ::graphql_orm::graphql::errors::OrmPublicError>> {
                        Box::pin(async move {
                            use ::graphql_orm::graphql::orm::DatabaseFilter;
                            const ORDER: &[::graphql_orm::graphql::orm::KeysetOrderColumn] = &[
                                #(#order_tokens),*
                            ];
                            const FINGERPRINT: &str = #fingerprint;
                            let bound_fingerprint = ::graphql_orm::graphql::orm::bind_keyset_fingerprint(
                                Some(<Self as ::graphql_orm::graphql::orm::Entity>::entity_name()),
                                FINGERPRINT,
                                &filter.keyset_filter_identity().into_iter().collect::<Vec<_>>(),
                            );
                            if filter.requires_in_memory_filtering(<#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT) {
                                return Err(::graphql_orm::graphql::errors::OrmPublicError::new(
                                    ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                                ));
                            }
                            let validated = page.validate(
                                ::graphql_orm::graphql::orm::PaginationConfig::DEFAULT_LIMIT,
                                i64::MAX,
                            )?;
                            let config = hook_ctx.database().pagination_config();
                            let resolved = config.resolve_page(
                                Some(&::graphql_orm::graphql::orm::PageInput {
                                    limit: Some(validated.limit),
                                    offset: Some(0),
                                }),
                                true,
                            );
                            let limit = resolved
                                .limit
                                .unwrap_or(::graphql_orm::graphql::orm::PaginationConfig::DEFAULT_LIMIT)
                                .max(1);
                            let backward = validated.direction
                                == ::graphql_orm::graphql::pagination::KeysetWindowDirection::Backward;
                            let had_cursor = validated.cursor.is_some();
                            let mut query = ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new().filter(&filter);
                            let total_count = if validated.include_total_count {
                                Some(query.clone().count_in_transaction(hook_ctx).await.map_err(::graphql_orm::graphql::errors::OrmPublicError::from)?)
                            } else {
                                None
                            };
                            if let Some(cursor) = validated.cursor.as_deref() {
                                let values = ::graphql_orm::graphql::pagination::decode_keyset_cursor(cursor, &bound_fingerprint, ORDER.len())?;
                                let (predicate, binds) = if backward {
                                    ::graphql_orm::graphql::orm::render_keyset_before(
                                        <#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT,
                                        ORDER,
                                        &values,
                                        query.values.len() + 1,
                                    )?
                                } else {
                                    ::graphql_orm::graphql::orm::render_keyset_after(
                                        <#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::DIALECT,
                                        ORDER,
                                        &values,
                                        query.values.len() + 1,
                                    )?
                                };
                                query = query.where_values(&predicate, binds);
                            }
                            query.order_clauses = if backward {
                                ORDER.iter().map(|column| column.reversed().order_sql()).collect()
                            } else {
                                ORDER.iter().map(|column| column.order_sql()).collect()
                            };
                            query = query.paginate(&::graphql_orm::graphql::orm::PageInput {
                                limit: Some(limit.saturating_add(1)),
                                offset: Some(0),
                            });
                            let mut entities = query.fetch_all_in_transaction(hook_ctx).await.map_err(::graphql_orm::graphql::errors::OrmPublicError::from)?;
                            let has_more = entities.len() > limit as usize;
                            if has_more {
                                entities.truncate(limit as usize);
                            }
                            if backward {
                                entities.reverse();
                            }
                            let edges = entities
                                .into_iter()
                                .map(|entity| {
                                    let values = vec![#(#cursor_tokens),*];
                                    ::graphql_orm::graphql::pagination::Edge {
                                        cursor: ::graphql_orm::graphql::pagination::encode_keyset_cursor(&bound_fingerprint, values),
                                        node: entity,
                                    }
                                })
                                .collect::<Vec<_>>();
                            Ok(::graphql_orm::graphql::pagination::Connection {
                                page_info: ::graphql_orm::graphql::pagination::PageInfo {
                                    has_next_page: if backward { had_cursor } else { has_more },
                                    has_previous_page: if backward { has_more } else { had_cursor },
                                    start_cursor: edges.first().map(|edge| edge.cursor.clone()),
                                    end_cursor: edges.last().map(|edge| edge.cursor.clone()),
                                    total_count,
                                },
                                edges,
                            })
                        })
                    }
                }
            },
            definitions,
        )
    } else {
        (quote! {}, quote! {}, quote! {})
    };
    let (keyset_repository_methods, keyset_trait_impl, authorized_keyset_definitions) =
        keyset_generated;
    let runtime_keyset_argument = if runtime_keyset_available {
        quote! {
            #[graphql(name = #order_by_arg_name, desc = #order_by_arg_description)] order_by: Option<Vec<#keyset_order_input>>,
        }
    } else {
        quote! {}
    };
    let runtime_keyset_prepare = if runtime_keyset_available {
        quote! {
            let order = order_by.map(#keyset_order_input::__gom_columns).transpose()?;
        }
    } else {
        quote! {}
    };
    let runtime_keyset_fetch = if runtime_keyset_available {
        quote! {
            .fetch_authorized_keyset_ordered(db, Some(ctx), auth, page, order).await?
        }
    } else {
        quote! {
            .fetch_authorized_keyset(db, Some(ctx), auth, page).await?
        }
    };
    // Filtering reveals field information even when the response selects only
    // unrelated fields. Authorize original recursive predicate intent before
    // any list/count/scan SQL, using the same public identities as field reads.
    let filter_read_authorization = quote! {
        let mut where_input = where_input;
        if let Some(filter) = where_input.as_mut() {
            filter.__gom_prepare_read(ctx, 0, &mut 128).await?;
        }

        if let Some(filter) = where_input.as_ref() {
            ::graphql_orm::graphql::orm::authorize_read_fields::<#struct_name, #backend_marker>(
                db, ctx, ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter),
            ).await?;
        }
    };
    // Ordering reveals field values, and relation count ordering reveals
    // related-row counts. Authorize each scalar term with the field's read
    // policy, and each count against the target's access, operation scopes
    // and SQL visibility, before any SQL.
    let order_read_authorization = quote! {
        let mut order_by = order_by;
        if let Some(orders) = order_by.as_mut() {
            for order in orders.iter_mut() {
                order.__gom_prepare_read(ctx).await?;
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
    let keyset_graphql_method = if keyset_parts.is_some() {
        quote! {
            #[doc = #keyset_operation_description]
            #[graphql(#keyset_query_skip name = #keyset_query_name, #keyset_list_requires_scopes #authenticated_directive)]
            async fn keyset(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: Option<#where_input>,
                #runtime_keyset_argument
                #[graphql(name = #page_arg_name, desc = #page_arg_description)] page: ::graphql_orm::graphql::pagination::KeysetPageInput,
            ) -> ::graphql_orm::async_graphql::Result<#connection_type> {
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #keyset_list_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                )?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                let auth = ctx.data_opt::<::graphql_orm::graphql::orm::DbAuthContext>();
                #filter_read_authorization
                let filter = where_input.unwrap_or_default();
                #runtime_keyset_prepare
                let connection = ::graphql_orm::graphql::orm::EntityQuery::<#struct_name, #backend_marker>::new()
                    .filter_with_entity_matching(&filter)
                    #runtime_keyset_fetch;
                Ok(#connection_type::from_generic(connection))
            }

            #[doc = #scan_operation_description]
            #[graphql(#scan_query_skip name = #scan_query_name, #keyset_list_requires_scopes #authenticated_directive)]
            async fn authorized_scan(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: Option<#where_input>,
                #[graphql(name = #page_arg_name, desc = #page_arg_description)] page: ::graphql_orm::graphql::orm::AuthorizedScanInput,
            ) -> ::graphql_orm::async_graphql::Result<#scan_connection_type> {
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #keyset_list_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx, ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                )?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                let auth = ctx.data_opt::<::graphql_orm::graphql::orm::DbAuthContext>();
                #filter_read_authorization
                let result = ::graphql_orm::graphql::orm::EntityQuery::<#struct_name, #backend_marker>::new()
                    .filter_with_entity_matching(&where_input.unwrap_or_default())
                    .fetch_authorized_scan(db, Some(ctx), auth, page).await?;
                Ok(#scan_connection_type { nodes: result.nodes, page_info: result.page_info })
            }
        }
    } else {
        quote! {}
    };

    let search_connection_definitions = if has_search {
        quote! {
            #[derive(::graphql_orm::async_graphql::SimpleObject, Debug, Clone)]
            #[graphql(name = #search_edge_type_str, rename_fields = #field_case_rule)]
            pub struct #search_edge_type {
                pub node: #struct_name,
                pub cursor: String,
                pub score: f64,
            }

            #[derive(::graphql_orm::async_graphql::SimpleObject, Debug, Clone)]
            #[graphql(name = #search_connection_type_str, rename_fields = #field_case_rule)]
            pub struct #search_connection_type {
                pub edges: Vec<#search_edge_type>,
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
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: Option<#where_input>,
                #[graphql(name = #page_arg_name, desc = #page_arg_description)] page: Option<::graphql_orm::graphql::orm::PageInput>,
            ) -> ::graphql_orm::async_graphql::Result<#search_connection_type> {
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #search_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                )?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                let pool = db.pool();
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                #filter_read_authorization

                let mut query = #struct_name::search(pool, search)
                    .pagination_config(db.pagination_config());
                if let Some(ref filter) = where_input {
                    query = query.filter(filter.clone());
                }
                query = query.default_order();

                let mut generic_conn = query
                    .fetch_authorized_connection(db, ctx, auth_context.as_ref(), page)
                    .await?;

                #relation_preload_search_list

                Ok(#search_connection_type::from_generic(generic_conn))
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
    let rebuild_rls_guard = if backend == BackendKind::Postgres {
        quote! { hook_ctx.execute("SET LOCAL row_security = off", &[]).await?; }
    } else {
        quote! {}
    };
    let rebuild_document_read = if backend == BackendKind::Postgres {
        quote! {
            let mut read_tx = db.pool().begin().await?;
            ::graphql_orm::sqlx::query("SET LOCAL row_security = off")
                .execute(&mut *read_tx).await?;
            let entity = query.fetch_one_on(&mut *read_tx).await?;
            read_tx.commit().await?;
            entity
        }
    } else {
        quote! { query.fetch_one(db).await? }
    };
    let search_rebuild_methods = if has_search
        && backend != BackendKind::Mssql
        && !schema_policy_read_only
        && !has_composite_primary_key
    {
        quote! {
            pub async fn rebuild_search_index(
                db: &::graphql_orm::db::Database<#backend_marker>,
            ) -> ::graphql_orm::Result<()> {
                let Some(index) = <Self as ::graphql_orm::graphql::orm::DatabaseSearchSchema>::search_index() else {
                    return Ok(());
                };
                if !index.enabled {
                    return Ok(());
                }
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::Default,
                ).await?;
                #rebuild_rls_guard
                let rows = ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                    .fetch_all_in_transaction(&mut hook_ctx)
                    .await?;
                for entity in &rows {
                    let document = <Self as ::graphql_orm::graphql::orm::SearchableEntity>::search_document(entity);
                    ::graphql_orm::graphql::orm::upsert_search_document_on::<#backend_marker>(
                        &mut hook_ctx,
                        index,
                        &document,
                    ).await?;
                }
                hook_ctx.commit_and_emit().await?;
                Ok(())
            }

            pub async fn rebuild_search_document(
                db: &::graphql_orm::db::Database<#backend_marker>,
                id: &#pk_type_ty,
            ) -> ::graphql_orm::Result<()> {
                let Some(index) = <Self as ::graphql_orm::graphql::orm::DatabaseSearchSchema>::search_index() else {
                    return Ok(());
                };
                if !index.enabled {
                    return Ok(());
                }
                db.ensure_entity_access(None, #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                let query = ::graphql_orm::graphql::orm::with_repository_read_visibility(
                    db,
                    ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                        .where_values(&Self::__gom_key_where_clause(), Self::__gom_key_values(id)),
                ).await?;
                // The short guarded read finishes before host callbacks, which may
                // query through a one-connection pool. They authorize the exact
                // fetched row used for the document; the later index write is separate.
                let entity = { #rebuild_document_read };
                let Some(entity) = entity else {
                    return Ok(());
                };
                if !db.can_read_row(None, #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                    &entity as &(dyn ::std::any::Any + Send + Sync),
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)? {
                    return Ok(());
                }
                for field in <Self as ::graphql_orm::graphql::orm::Entity>::repository_field_policies() {
                    db.ensure_repository_readable_field(None, #entity_name_lit,
                        field.api_name, field.read_policy,
                        Some(&entity as &(dyn ::std::any::Any + Send + Sync)),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                }
                let document = <Self as ::graphql_orm::graphql::orm::SearchableEntity>::search_document(&entity);
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::Default,
                ).await?;
                #rebuild_rls_guard
                ::graphql_orm::graphql::orm::upsert_search_document_on::<#backend_marker>(
                    &mut hook_ctx,
                    index,
                    &document,
                ).await?;
                hook_ctx.commit_and_emit().await?;
                Ok(())
            }
        }
    } else {
        quote! {}
    };
    let search_write_maintenance = if has_search && backend != BackendKind::Mssql {
        quote! {
            if let Some(index) = <Self as ::graphql_orm::graphql::orm::DatabaseSearchSchema>::search_index() {
                if index.enabled {
                    match action {
                        ::graphql_orm::graphql::orm::ChangeAction::Deleted => {
                            let key = <Self as ::graphql_orm::graphql::orm::SearchableEntity>::search_key(entity);
                            ::graphql_orm::graphql::orm::delete_search_document_on::<#backend_marker>(
                                hook_ctx,
                                index,
                                &key,
                            ).await?;
                        }
                        ::graphql_orm::graphql::orm::ChangeAction::Created
                        | ::graphql_orm::graphql::orm::ChangeAction::Updated => {
                            let document = <Self as ::graphql_orm::graphql::orm::SearchableEntity>::search_document(entity);
                            ::graphql_orm::graphql::orm::upsert_search_document_on::<#backend_marker>(
                                hook_ctx,
                                index,
                                &document,
                            ).await?;
                        }
                    }
                }
            }
        }
    } else {
        quote! {}
    };

    let retention_event_id_values = primary_key_fields
        .iter()
        .map(|field| {
            let field_name = &field.field_name;
            push_create_input_sql_value_tokens(
                backend,
                struct_name,
                quote! { self.#field_name.clone() },
                &field.field_type,
                &field.meta,
                quote! { ::graphql_orm::sqlx::Error },
            )
        })
        .collect::<Vec<_>>();
    let retention_event_id = if has_composite_primary_key {
        quote! {{
            let mut bind_values = Vec::new();
            #(#retention_event_id_values)*
            ::graphql_orm::graphql::orm::composite_key_id(&bind_values)
        }}
    } else if pk_is_bytes {
        quote! { ::graphql_orm::graphql::orm::binary_key_id(&self.#pk_field) }
    } else {
        quote! { self.#pk_field.to_string() }
    };
    let retention_surface = if entity_meta.retention_policy.is_some() {
        quote! {
            impl ::graphql_orm::graphql::orm::RetentionPurge<#backend_marker> for #struct_name {
                type Filter = #where_input;

                fn retention_event_id(&self) -> String {
                    #retention_event_id
                }

                fn retention_queue_changed_event<'a>(
                    hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    entity: &'a Self,
                ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<()>> {
                    Box::pin(async move {
                        Self::__gom_queue_changed_event(
                            hook_ctx,
                            ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                            entity,
                        ).await
                    })
                }
            }
        }
    } else {
        quote! {}
    };

    // A committed create does not grant permission to disclose the resulting row.
    // Keep ordinary and append-only GraphQL creates on the same release contract.
    let create_output_authorization = quote! {
        let entity = db.__release_graphql_mutation_output(
            ctx,
            #entity_result_field_name,
            Some(entity),
            &#struct_name::__gom_key_where_clause(),
            #struct_name::__gom_subscription_key_values,
        ).await?;
    };
    // Ordinary and append-only GraphQL creates use one resolver pipeline. Only
    // key/event representation varies; the append-only object emits no other writes.
    let created_key_init = if has_composite_primary_key {
        let components = primary_key_fields.iter().map(|key| {
            let field = &key.field_name;
            quote! { #field: input.#field.clone() }
        });
        quote! { let created_pk = #key_type { #(#components),* }; }
    } else {
        created_pk_init.clone()
    };
    let created_key_id = if has_composite_primary_key {
        quote! { ::graphql_orm::graphql::orm::composite_key_id(&#struct_name::__gom_key_values(&created_pk)) }
    } else {
        created_pk_id_string.clone()
    };
    let create_graphql_entity_id = if has_composite_primary_key {
        quote! { ::graphql_orm::graphql::orm::composite_key_id(
            &#struct_name::__gom_key_values(&(#propagation_entity_key))
        ) }
    } else {
        pk_id_for_entity.clone()
    };
    let create_graphql_event_entity = if entity_meta.append_only {
        quote! { &entity }
    } else {
        quote! { Some(&entity) }
    };
    let create_output_binding = if has_notify_handler {
        // Retain the committed row for notification after the output checks.
        quote! { let entity = entity.clone(); }
    } else {
        quote! {}
    };
    // Only a create input with a public member is a valid GraphQL input. Upsert
    // shares it and is generated only where create is.
    let graphql_create_input_definition = if generates_public_create {
        quote! {
            #[derive(::graphql_orm::async_graphql::InputObject, Clone, Debug)]
            #[graphql(name = #create_input_str, rename_fields = #field_case_rule)]
            struct #graphql_create_input {
                #(#graphql_create_input_fields)*
            }

            impl From<#graphql_create_input> for #create_input {
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
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #create_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation,
                )?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                let pool = db.pool();
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                let mut input: #create_input = input.into();
                #[allow(unused_mut)]
                let mut __gom_client_supplied: ::std::collections::HashSet<&'static str> =
                    ::std::collections::HashSet::new();
                #(#create_client_supplied)*
                db.run_before_create(
                    Some(ctx),
                    #entity_name_lit,
                    &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                #created_key_init
                let sql = #struct_name::__gom_rebind_sql(#insert_sql, 1);

                // Collect all values first
                let mut bind_values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
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
                    Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #created_key_id,
                        changes: mutation_changes.clone(),
                        before_state: None,
                        after_state: None,
                    },
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                hook_ctx.execute(&sql, &bind_values)
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                let entity = #struct_name::__gom_fetch_by_id_on(&mut hook_ctx, &created_pk)
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?
                    .ok_or_else(|| ::graphql_orm::async_graphql::Error::new("Entity not found after creation"))?;
                #graphql_post_image_check
                let after_state = Some(
                    ::graphql_orm::graphql::orm::entity_state(&entity)
                        .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?
                );

                hook_ctx.run_mutation_hook(
                    Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #create_graphql_entity_id,
                        changes: mutation_changes.clone(),
                        before_state: None,
                        after_state,
                    },
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                #struct_name::__gom_queue_changed_event(
                    &mut hook_ctx,
                    ::graphql_orm::graphql::orm::ChangeAction::Created,
                    #create_graphql_event_entity,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                hook_ctx.commit_and_emit().await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                let result = {
                    #create_output_binding
                    #create_output_authorization
                    #result_type { success: true, error: None, entity }
                };
                #notify_on_created
                Ok(result)
            }

    };
    let create_graphql_method = if generates_public_create {
        create_graphql_method
    } else {
        quote! {}
    };

    let append_only_surface = if entity_meta.append_only {
        // An object type needs one field. Without a public create field the
        // root keeps an introspection-hidden placeholder, like the composite
        // and read-only stubs, so schema_roots! can still merge it.
        // It is the same described, unsatisfiable placeholder as the
        // write-less roots above, so routers and the catalogue see it.
        let append_only_mutation_placeholder = if emits_append_only_write_placeholder {
            quote! {
                #[doc = #write_placeholder_description]
                #[graphql(name = #write_placeholder_field, visible = false)]
                async fn __gom_public_create_unavailable(
                    &self,
                ) -> ::graphql_orm::async_graphql::Result<bool> {
                    Err(::graphql_orm::graphql::errors::OrmPublicError::forbidden().into_graphql_error())
                }
            }
        } else {
            quote! {}
        };
        let append_event_source_id = if has_composite_primary_key {
            quote! { ::graphql_orm::graphql::orm::composite_key_id(&Self::__gom_key_values(&id)) }
        } else {
            pk_id_for_entity.clone()
        };
        let append_release_key_field = if has_composite_primary_key {
            quote! { key: event.key.clone(), }
        } else {
            quote! {}
        };
        quote! {
            #[derive(Clone, Debug)]
            pub struct #create_input { #(#create_input_fields)* }

            #graphql_create_input_definition

            #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
            #[graphql(name = #result_type_str, rename_fields = #field_case_rule)]
            pub struct #result_type {
                pub success: bool,
                pub error: Option<String>,
                #[graphql(name = #entity_result_field_name)]
                pub entity: Option<#struct_name>,
            }

            #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject, serde::Serialize, serde::Deserialize)]
            #[graphql(name = #changed_event_str, rename_fields = #field_case_rule)]
            pub struct #changed_event {
                pub action: ::graphql_orm::graphql::orm::ChangeAction,
                #[graphql(name = #change_kind_field_name)]
                pub change_kind: ::graphql_orm::graphql::orm::ChangeKind,
                pub id: #subscription_identity_type,
                #subscription_extra_key_field
                #[graphql(name = #source_entity_field_name)]
                pub source_entity: Option<String>,
                #[graphql(name = #source_id_field_name)]
                pub source_id: Option<String>,
                pub path: Vec<String>,
                #[graphql(name = #entity_result_field_name)]
                pub entity: Option<#struct_name>,
            }

            #[derive(Default)]
            pub struct #mutations_struct;

            #[::graphql_orm::async_graphql::Object]
            impl #mutations_struct {
                    #create_graphql_method
                    #append_only_mutation_placeholder
            }

            #[derive(Default)]
            pub struct #subscriptions_struct;

            #[::graphql_orm::async_graphql::Subscription]
            impl #subscriptions_struct {
                #[doc = #subscription_operation_description]
                #[graphql(name = #subscription_name, #subscription_requires_scopes #authenticated_directive)]
                async fn on_changed(
                    &self,
                    ctx: &::graphql_orm::async_graphql::Context<'_>,
                    #[graphql(name = #filter_arg_name, desc = #filter_arg_description)] _filter: Option<::graphql_orm::graphql::orm::SubscriptionFilterInput>,
                    #[graphql(name = #subscription_key_arg_name, desc = "Exact complete primary key to observe")] key: Option<#key_type>,
                #[graphql(name = #subscription_keys_arg_name, desc = #subscription_keys_arg_description)] keys: Option<Vec<#key_type>>,
                ) -> ::graphql_orm::async_graphql::Result<impl ::graphql_orm::futures::Stream<Item = ::graphql_orm::async_graphql::Result<::graphql_orm::graphql::subscriptions::ChangedPayload<#changed_event>>>> {
                    use ::graphql_orm::futures::StreamExt;
                    ::graphql_orm::graphql::subscriptions::admit_generated_operation(ctx)?;
                    let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                    #subscription_scope_enforcement
                    ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                        ctx,
                        ::graphql_orm::graphql::orm::GraphqlOperationKind::Subscription,
                    )?;
                    let db = ctx.data::<::graphql_orm::db::Database<#backend_marker>>().map_err(|_| {
                        ::graphql_orm::async_graphql::Error::new(
                            "graphql-orm Database runtime not registered; build the schema with schema_builder(database) or add Database to schema data",
                        )
                    })?;
                    db.ensure_entity_access(
                        Some(ctx),
                        #entity_name_lit,
                        <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                        ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlSubscription,
                    ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                    if _filter.as_ref().is_some_and(|filter| filter.dummy.is_some()) {
                        return Err(::graphql_orm::graphql::errors::OrmPublicError::new(::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput).into_graphql_error());
                    }
                    ::graphql_orm::graphql::subscriptions::validate_subscription_selection::<#struct_name, #backend_marker>(ctx, #entity_result_field_name)?;
                    let keys = ::graphql_orm::graphql::subscriptions::admit_subscription_keys(
                        db,
                        key.is_some(),
                        keys.map(|keys| keys.iter().map(#struct_name::__gom_key_values).collect()),
                    )?;
                    let actions = _filter.and_then(|filter| filter.actions);
                    let rx = db.ensure_event_sender::<::graphql_orm::graphql::subscriptions::SubscriptionEvent<#changed_event, #backend_marker>>().subscribe();
                    let db = db.clone();
                    let interval = db.subscription_authority_recheck_interval();
                    let timer_db = db.clone();
                    let events = ::graphql_orm::graphql::subscriptions::continuity_events(rx).filter_map(move |result| {
                        let db = db.clone();
                        let actions = actions.clone();
                        let key = key.clone();
                        let keys = keys.clone();
                        async move {
                            let witness = match result {
                                Ok(witness) => witness,
                                // Lost continuity or a closed event source: one error, then the stream ends.
                                Err(error) => return Some(Err(error)),
                            };
                            // Host policies may buffer private diagnostics before denying an event.
                            // Cover every filter and release-proof drop, not only payload preparation.
                            let errors_before = ctx.query_env.errors.lock().unwrap().len();
                            let delivered: Option<::graphql_orm::graphql::subscriptions::ChangedPayload<#changed_event>> = async move {
                                let event = witness.event.as_ref();
                                if key.as_ref().is_some_and(|key| ::graphql_orm::graphql::loaders::RelationKey::from_sql_values(&#struct_name::__gom_key_values(key)) != ::graphql_orm::graphql::loaders::RelationKey::from_sql_values(&witness.keys)) { return None; }
                                if keys.as_ref().is_some_and(|keys| !keys.contains(&::graphql_orm::graphql::loaders::RelationKey::from_sql_values(&witness.keys))) { return None; }
                                if ::graphql_orm::graphql::loaders::RelationKey::from_sql_values(&witness.keys)
                                    != ::graphql_orm::graphql::loaders::RelationKey::from_sql_values(&#struct_name::__gom_key_values(&#subscription_event_key)) { return None; }
                                if actions.as_ref().is_some_and(|actions| !actions.contains(&event.action)) { return None; }
                                let authorize_current = || async {
                                    let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                                    #subscription_scope_enforcement
                                    ::graphql_orm::graphql::assurance::enforce_resolver_assurance(ctx, ::graphql_orm::graphql::orm::GraphqlOperationKind::Subscription)?;
                                    witness.authorize_provenance(&db, ctx, event.source_entity.as_deref(), event.source_id.as_deref(), &event.path, event.change_kind == ::graphql_orm::graphql::orm::ChangeKind::Propagated).await?;
                                    if witness.trusted_delete
                                        && event.entity.as_ref().is_some_and(|entity|
                                            ::graphql_orm::graphql::loaders::RelationKey::from_sql_values(&#struct_name::__gom_subscription_key_values(entity))
                                            != ::graphql_orm::graphql::loaders::RelationKey::from_sql_values(&witness.keys)
                                        ) { return Err(::graphql_orm::graphql::errors::OrmPublicError::forbidden().into_graphql_error()); }
                                    let preimage = if witness.trusted_delete && event.action == ::graphql_orm::graphql::orm::ChangeAction::Deleted { event.entity.as_ref() } else { None };
                                    let row = ::graphql_orm::graphql::subscriptions::authorize_subscription_row::<#struct_name, #backend_marker>(&db, ctx, &witness.keys, preimage).await?
                                        .ok_or_else(|| ::graphql_orm::graphql::errors::OrmPublicError::forbidden().into_graphql_error())?;
                                    if key.is_some() || keys.is_some() {
                                        ::graphql_orm::graphql::subscriptions::authorize_subscription_keys::<#struct_name, #backend_marker>(&db, ctx, &row).await?;
                                    }
                                    ::graphql_orm::graphql::subscriptions::authorize_subscription_fields(&db, ctx, &row, #entity_result_field_name).await?;
                                    // Query and field-policy awaits must not outlive current authority.
                                    db.ensure_entity_access(Some(ctx), #entity_name_lit,
                                        <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                                        ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                                        ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlSubscription).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                                    let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                                    #subscription_scope_enforcement
                                    ::graphql_orm::graphql::assurance::enforce_resolver_assurance(ctx, ::graphql_orm::graphql::orm::GraphqlOperationKind::Subscription)?;
                                    witness.authorize_provenance(&db, ctx, event.source_entity.as_deref(), event.source_id.as_deref(), &event.path, event.change_kind == ::graphql_orm::graphql::orm::ChangeKind::Propagated).await?;
                                    #struct_name::__gom_authorize_changed_operation(ctx)?;
                                    Ok::<#struct_name, ::graphql_orm::async_graphql::Error>(row)
                                };
                                match authorize_current().await {
                                    Ok(row) => {
                                        let prepared = ::graphql_orm::graphql::subscriptions::ChangedPayload::prepare(#changed_event {
                                            action: event.action,
                                            change_kind: event.change_kind,
                                            id: event.id.clone(),
                                            #append_release_key_field
                                            source_entity: event.source_entity.clone(),
                                            source_id: event.source_id.clone(),
                                            path: event.path.clone(),
                                            entity: Some(row),
                                        }, ctx).await?;
                                        // Successful serialization can itself await or revoke authority.
                                        // Recheck the complete current release contract before yielding.
                                        match authorize_current().await {
                                            Ok(_) => Some(prepared),
                                            Err(error) => { let _ = ::graphql_orm::graphql::loaders::relation_authorization_error(error); None }
                                        }
                                    },
                                    Err(error) => { let _ = ::graphql_orm::graphql::loaders::relation_authorization_error(error); None }
                                }
                            }.await;
                            if delivered.is_none() {
                                ctx.query_env.errors.lock().unwrap().truncate(errors_before);
                            }
                            delivered.map(Ok)
                        }
                    });
                    Ok(::graphql_orm::graphql::subscriptions::watch_subscription_authority(
                        events, interval, move || {
                            let db = timer_db.clone();
                            async move {
                                let result = async {
                                #struct_name::__gom_authorize_changed_operation(ctx)?;
                                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                                    ctx, ::graphql_orm::graphql::orm::GraphqlOperationKind::Subscription)?;
                                if !db.can_access_entity(Some(ctx), #entity_name_lit,
                                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                                    ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlSubscription).await? {
                                    return Err(::graphql_orm::graphql::errors::OrmPublicError::forbidden().into_graphql_error());
                                }
                                // Current principal/scopes/assurance must survive the policy await.
                                #struct_name::__gom_authorize_changed_operation(ctx)?;
                                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                                    ctx, ::graphql_orm::graphql::orm::GraphqlOperationKind::Subscription)
                                }.await;
                                if result.is_err() {
                                    // A timer may preempt a pending event preparation.
                                    // Never attach its private diagnostics to the terminal frame.
                                    ctx.query_env.errors.lock().unwrap().clear();
                                }
                                result
                            }
                        }
                    ))
                }
            }

            impl #struct_name {
                #[doc(hidden)]
                fn __gom_runtime_error(message: impl Into<String>) -> ::graphql_orm::sqlx::Error {
                    ::graphql_orm::sqlx::Error::Protocol(message.into())
                }

                #subscription_key_method
                #propagated_subscription_emitter

                #[doc(hidden)]
                #[allow(clippy::too_many_arguments)]
                #subscription_key_visibility fn __gom_propagate_changed_event<'a>(
                    hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    action: ::graphql_orm::graphql::orm::ChangeAction,
                    entity: &'a Self,
                    source_entity: &'a str,
                    source_id: &'a str,
                    path: Vec<String>,
                    visited: &'a mut ::std::collections::HashSet<String>,
                    provenance: &'a [::graphql_orm::graphql::subscriptions::SubscriptionEventSource<#backend_marker>],
                ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<()>> {
                    Box::pin(async move {
                        if provenance.len() >= ::graphql_orm::graphql::subscriptions::MAX_SUBSCRIPTION_PROVENANCE_HOPS
                            || visited.len() >= ::graphql_orm::graphql::subscriptions::MAX_SUBSCRIPTION_PROPAGATION_EVENTS { return Ok(()); }
                        #(#propagation_blocks)* Ok(())
                    })
                }

                #[doc(hidden)]
                async fn __gom_queue_changed_event(
                    hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    action: ::graphql_orm::graphql::orm::ChangeAction,
                    entity: &Self,
                ) -> ::graphql_orm::Result<()> {
                    #search_write_maintenance
                    let id = #propagation_entity_key;
                    let source_id = #append_event_source_id;
                    let event = #changed_event {
                        action,
                        change_kind: ::graphql_orm::graphql::orm::ChangeKind::Direct,
                        id: #propagated_event_id,
                        #subscription_key_initializer
                        source_entity: None, source_id: None, path: Vec::new(), entity: Some(entity.clone()),
                    };
                    hook_ctx.queue_event(event.clone());
                    #queue_subscription_witness
                    #propagation_visited_seed
                    Self::__gom_propagate_changed_event(
                        hook_ctx, action, entity, #entity_name_lit, &source_id, Vec::new(), &mut visited, &[],
                    ).await
                }

                #[doc(hidden)]
                async fn __gom_insert_with_mutation_context<'a>(
                    hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    mut input: #create_input,
                ) -> ::graphql_orm::Result<Self> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<Self> = async {
                    hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    use ::graphql_orm::graphql::orm::{DatabaseEntity, SqlValue};
                    let db = hook_ctx.database().clone();
                    let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::internal(#entity_name_lit, hook_ctx);
                    db.run_before_create_with_context(&mut write_ctx, &mut input as &mut (dyn ::std::any::Any + Send + Sync))
                        .await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    <Self as ::graphql_orm::graphql::orm::Entity>::authorize_repository_create_fields(
                        &db,
                        &input as &(dyn ::std::any::Any + Send + Sync),
                    ).await?;
                    #(#repository_create_field_checks)*
                    #created_key_init
                    let sql = Self::__gom_rebind_sql(#insert_sql, 1);
                    let mut bind_values: Vec<SqlValue> = Vec::new();
                    #prepend_pk_bind
                    #(#insert_binds_repo)*
                    hook_ctx.run_mutation_hook(None, &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #created_key_id,
                        changes: ::graphql_orm::graphql::orm::mutation_changes(&[#(#create_mutation_field_literals),*], &bind_values),
                        before_state: None,
                        after_state: None,
                    }).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    hook_ctx.execute(&sql, &bind_values).await?;
                    let entity = Self::__gom_fetch_by_id_on(hook_ctx, &created_pk).await?
                        .ok_or(::graphql_orm::sqlx::Error::RowNotFound)?;
                    #repository_post_image_check
                    hook_ctx.run_mutation_hook(None, &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #created_key_id,
                        changes: ::graphql_orm::graphql::orm::mutation_changes(&[#(#create_mutation_field_literals),*], &bind_values),
                        before_state: None,
                        after_state: Some(::graphql_orm::graphql::orm::entity_state(&entity)?),
                    }).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    Self::__gom_queue_changed_event(
                        hook_ctx,
                        ::graphql_orm::graphql::orm::ChangeAction::Created,
                        &entity,
                    ).await?;
                    Ok(entity)
                }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }

                #[doc(hidden)]
                #[allow(clippy::ptr_arg)]
                async fn __gom_fetch_by_id_on(
                    hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    id: &#key_type,
                ) -> ::graphql_orm::Result<Option<Self>> {
                    use ::graphql_orm::graphql::orm::{DatabaseEntity, FromSqlRow};
                    let sql = Self::__gom_rebind_sql(&format!(
                        "SELECT {} FROM {}{} WHERE {}{}",
                        Self::column_names().join(", "),
                        #sql_table_name,
                        #upsert_read_lock_hint,
                        Self::__gom_key_where_clause(),
                        #write_read_lock_suffix,
                    ), 1);
                    let rows = hook_ctx.fetch_rows(&sql, &Self::__gom_key_values(id)).await?;
                    rows.first().map(<Self as FromSqlRow<#backend_marker>>::from_row).transpose()
                }

                pub async fn insert(db: &::graphql_orm::db::Database<#backend_marker>, input: #create_input) -> ::graphql_orm::Result<Self> {
                    let mut context = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::Default,
                ).await?;
                    let entity = context.insert::<Self>(input).await?;
                    context.commit_and_emit().await?;
                    Ok(entity)
                }

            }

            impl ::graphql_orm::graphql::orm::MutationContextInsert<#backend_marker> for #struct_name {
                type CreateInput = #create_input;
                fn insert_in_mutation_context<'a>(
                    hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    input: Self::CreateInput,
                ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<Self>> {
                    Box::pin(async move { Self::__gom_insert_with_mutation_context(hook_ctx, input).await })
                }
            }

            #keyset_trait_impl
        }
    } else {
        quote! {}
    };

    let composite_mutation_surface = if composite_repository_mutations {
        let key_from_entity = primary_key_fields
            .iter()
            .map(|key| {
                let field = &key.field_name;
                quote! { #field: entity.#field.clone(), }
            })
            .collect::<Vec<_>>();
        let key_from_input = primary_key_fields
            .iter()
            .map(|key| {
                let field = &key.field_name;
                quote! { #field: input.#field.clone(), }
            })
            .collect::<Vec<_>>();
        let insert_if_absent_conflicts = if let Some(config) = &upsert_config {
            config
                .fields
                .iter()
                .map(|field| field.db_column.clone())
                .collect::<Vec<_>>()
        } else {
            primary_key_fields
                .iter()
                .map(|key| {
                    key.meta
                        .db_column
                        .clone()
                        .unwrap_or_else(|| key.rust_name.clone())
                })
                .collect::<Vec<_>>()
        };
        let insert_if_absent_conflict_literals = insert_if_absent_conflicts
            .iter()
            .map(|column| syn::LitStr::new(column, struct_name.span()))
            .collect::<Vec<_>>();
        let fetch_existing_after_conflict = if upsert_config.is_some() {
            quote! { Self::__gom_fetch_by_upsert_on(hook_ctx, &lookup_input).await? }
        } else {
            quote! { Self::__gom_fetch_by_key_on(hook_ctx, &key).await? }
        };

        quote! {
            #[derive(Clone, Debug)]
            pub struct #create_input { #(#create_input_fields)* }

            #[derive(Clone, Debug, Default)]
            pub struct #update_input { #(#update_input_fields)* }

            #update_assignment_helpers

            impl #struct_name {
                #[doc(hidden)]
                fn __gom_runtime_error(message: impl Into<String>) -> ::graphql_orm::sqlx::Error {
                    ::graphql_orm::sqlx::Error::Protocol(message.into())
                }

                #[doc(hidden)]
                fn __gom_require_repository_write_policy(
                    db: &::graphql_orm::db::Database<#backend_marker>,
                ) -> ::graphql_orm::Result<()> {
                    if db.entity_policy().is_none() {
                        return Err(Self::__gom_runtime_error(
                            "composite repository mutations require an explicit entity policy provider",
                        ));
                    }
                    Ok(())
                }

                #[doc(hidden)]
                fn __gom_entity_key(entity: &Self) -> #key_type {
                    #key_type { #(#key_from_entity)* }
                }

                #[doc(hidden)]
                fn __gom_key_where_clause_at(start: usize) -> String {
                    use ::graphql_orm::graphql::orm::{OrmBackend, SqlDialect};
                    let dialect = <#backend_marker as OrmBackend>::DIALECT;
                    <Self as ::graphql_orm::graphql::orm::DatabaseEntity>::PRIMARY_KEYS
                        .iter()
                        .enumerate()
                        .map(|(index, column)| format!(
                            "{} = {}",
                            dialect.quote_identifier_path(column),
                            dialect.placeholder(start + index),
                        ))
                        .collect::<Vec<_>>()
                        .join(" AND ")
                }

                #[doc(hidden)]
                fn __gom_event_id(key: &#key_type) -> String {
                    ::graphql_orm::graphql::orm::composite_key_id(&Self::__gom_key_values(key))
                }

                #[doc(hidden)]
                async fn __gom_fetch_by_key_on(
                    hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    key: &#key_type,
                ) -> ::graphql_orm::Result<Option<Self>> {
                    use ::graphql_orm::graphql::orm::{DatabaseEntity, FromSqlRow, OrmBackend, SqlDialect};
                    let dialect = <#backend_marker as OrmBackend>::DIALECT;
                    let columns = Self::column_names().iter()
                        .map(|column| {
                            if #is_mssql {
                                (*column).to_string()
                            } else {
                                dialect.quote_identifier_path(column)
                            }
                        })
                        .collect::<Vec<_>>().join(", ");
                    let sql = format!(
                        "SELECT {columns} FROM {}{} WHERE {}{}",
                        dialect.quote_identifier_path(#table_name),
                        #upsert_read_lock_hint,
                        Self::__gom_key_where_clause_at(1),
                        #write_read_lock_suffix,
                    );
                    let rows = hook_ctx.fetch_rows(&sql, &Self::__gom_key_values(key)).await?;
                    if rows.len() > 1 {
                        return Err(Self::__gom_runtime_error("complete composite key matched more than one row"));
                    }
                    rows.first().map(<Self as FromSqlRow<#backend_marker>>::from_row).transpose()
                }

                #[doc(hidden)]
                async fn __gom_queue_changed_event(
                    hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    action: ::graphql_orm::graphql::orm::ChangeAction,
                    entity: Option<&Self>,
                ) -> ::graphql_orm::Result<()> {
                    if let Some(entity) = entity {
                        #search_write_maintenance
                        let id = Self::__gom_entity_key(entity);
                        let source_id = ::graphql_orm::graphql::orm::composite_key_id(&Self::__gom_key_values(&id));
                        let event = #changed_event {
                            action, change_kind: ::graphql_orm::graphql::orm::ChangeKind::Direct,
                            id: source_id.clone(), key: id,
                            source_entity: None, source_id: None, path: Vec::new(),
                            entity: Some(entity.clone()),
                        };
                        hook_ctx.queue_event(event.clone());
                        #queue_subscription_witness
                        #propagation_visited_seed
                        Self::__gom_propagate_changed_event(hook_ctx, action, entity, #entity_name_lit,
                            &source_id, Vec::new(), &mut visited, &[]).await?;
                    }
                    Ok(())
                }

                #[doc(hidden)]
                fn __gom_capture_entity_state(entity: &Self) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::EntityState> {
                    ::graphql_orm::graphql::orm::entity_state(entity)
                }

                #[doc(hidden)]
                async fn __gom_insert_with_mutation_context(
                    hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    mut input: #create_input,
                ) -> ::graphql_orm::Result<Self> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<Self> = async {
                    hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    use ::graphql_orm::graphql::orm::SqlValue;
                    let db = hook_ctx.database().clone();
                    Self::__gom_require_repository_write_policy(&db)?;
                    let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::internal(#entity_name_lit, hook_ctx);
                    db.run_before_create_with_context(&mut write_ctx, &mut input as &mut (dyn ::std::any::Any + Send + Sync))
                        .await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    <Self as ::graphql_orm::graphql::orm::Entity>::authorize_repository_create_fields(
                        &db,
                        &input as &(dyn ::std::any::Any + Send + Sync),
                    ).await?;
                    #(#repository_create_field_checks)*
                    let key = #key_type { #(#key_from_input)* };
                    let sql = Self::__gom_rebind_sql(&::graphql_orm::graphql::orm::build_insert_sql(
                        #table_name,
                        &[#(#upsert_insert_column_literals),*],
                        &[#(#upsert_insert_value_literals),*],
                    ), 1);
                    let mut bind_values: Vec<SqlValue> = Vec::new();
                    #(#insert_binds_repo)*
                    let changes = ::graphql_orm::graphql::orm::mutation_changes(&[#(#create_mutation_field_literals),*], &bind_values);
                    hook_ctx.run_mutation_hook(None, &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: #entity_name_lit, table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: Self::__gom_event_id(&key), changes: changes.clone(),
                        before_state: None, after_state: None,
                    }).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    let result = hook_ctx.execute(&sql, &bind_values,).await?;
                    if <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&result) != 1 {
                        return Err(Self::__gom_runtime_error("composite-key insert did not affect exactly one row"));
                    }
                    let entity = Self::__gom_fetch_by_key_on(hook_ctx, &key).await?
                        .ok_or(::graphql_orm::sqlx::Error::RowNotFound)?;
                    hook_ctx.__ensure_written_row(None, #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                        &entity as &(dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    hook_ctx.run_mutation_hook(None, &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: #entity_name_lit, table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: Self::__gom_event_id(&key), changes,
                        before_state: None, after_state: Some(Self::__gom_capture_entity_state(&entity)?),
                    }).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    Self::__gom_queue_changed_event(hook_ctx, ::graphql_orm::graphql::orm::ChangeAction::Created, Some(&entity)).await?;
                    Ok(entity)
                }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }

                #[doc(hidden)]
                async fn __gom_insert_if_absent_with_mutation_context(
                    hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    mut input: #create_input,
                ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::InsertIfAbsentOutcome<Self>> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<::graphql_orm::graphql::orm::InsertIfAbsentOutcome<Self>> = async {
                    hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    use ::graphql_orm::graphql::orm::SqlValue;
                    let db = hook_ctx.database().clone();
                    Self::__gom_require_repository_write_policy(&db)?;
                    let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::internal(#entity_name_lit, hook_ctx);
                    db.run_before_create_with_context(&mut write_ctx, &mut input as &mut (dyn ::std::any::Any + Send + Sync))
                        .await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    <Self as ::graphql_orm::graphql::orm::Entity>::authorize_repository_create_fields(
                        &db,
                        &input as &(dyn ::std::any::Any + Send + Sync),
                    ).await?;
                    #(#repository_create_field_checks)*
                    let key = #key_type { #(#key_from_input)* };
                    let lookup_input = input.clone();
                    let sql = Self::__gom_rebind_sql(&::graphql_orm::graphql::orm::build_insert_if_absent_sql(
                        #table_name,
                        &[#(#upsert_insert_column_literals),*],
                        &[#(#upsert_insert_value_literals),*],
                        &[#(#insert_if_absent_conflict_literals),*],
                    ), 1);
                    let mut bind_values: Vec<SqlValue> = Vec::new();
                    #(#insert_binds_repo)*
                    let changes = ::graphql_orm::graphql::orm::mutation_changes(&[#(#create_mutation_field_literals),*], &bind_values);
                    hook_ctx.run_mutation_hook(None, &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: #entity_name_lit, table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: Self::__gom_event_id(&key), changes: changes.clone(),
                        before_state: None, after_state: None,
                    }).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    let result = hook_ctx.execute(&sql, &bind_values,).await?;
                    match <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&result) {
                        0 => {
                            let existing = #fetch_existing_after_conflict
                                .ok_or(::graphql_orm::sqlx::Error::RowNotFound)?;
                            db.ensure_writable_row(None, #entity_name_lit,
                                <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                                ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                                &existing as &(dyn ::std::any::Any + Send + Sync),
                            ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                            return Ok(::graphql_orm::graphql::orm::InsertIfAbsentOutcome::AlreadyPresent(existing));
                        }
                        1 => {}
                        _ => return Err(Self::__gom_runtime_error("insert-if-absent affected more than one row")),
                    }
                    let entity = Self::__gom_fetch_by_key_on(hook_ctx, &key).await?
                        .ok_or(::graphql_orm::sqlx::Error::RowNotFound)?;
                    hook_ctx.__ensure_written_row(None, #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                        &entity as &(dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    hook_ctx.run_mutation_hook(None, &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                        entity_name: #entity_name_lit, table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: Self::__gom_event_id(&key), changes,
                        before_state: None, after_state: Some(Self::__gom_capture_entity_state(&entity)?),
                    }).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    Self::__gom_queue_changed_event(hook_ctx, ::graphql_orm::graphql::orm::ChangeAction::Created, Some(&entity)).await?;
                    Ok(::graphql_orm::graphql::orm::InsertIfAbsentOutcome::Inserted(entity))
                }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }

                #[doc(hidden)]
                async fn __gom_update_by_key_with_mutation_context(
                    hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    key: &#key_type,
                    mut input: #update_input,
                ) -> ::graphql_orm::Result<Option<Self>> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<Option<Self>> = async {
                    hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    use ::graphql_orm::graphql::orm::{OrmBackend, SqlDialect};
                    let db = hook_ctx.database().clone();
                    Self::__gom_require_repository_write_policy(&db)?;
                    let Some(current_entity) = Self::__gom_fetch_by_key_on(hook_ctx, key).await? else {
                        return Ok(None);
                    };
                    db.ensure_writable_row(None, #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                        &current_entity as &(dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::internal(#entity_name_lit, hook_ctx);
                    db.run_before_update_with_context(
                        &mut write_ctx,
                        Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                        &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    <Self as ::graphql_orm::graphql::orm::Entity>::authorize_repository_update_fields(
                        &db,
                        Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                        &input as &(dyn ::std::any::Any + Send + Sync),
                    ).await?;
                    Self::__gom_authorize_repository_update_field_writes(
                        &db,
                        Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                        &input,
                    ).await?;
                    let mut set_clauses: Vec<String> = Vec::new();
                    let mut changed_fields: Vec<&str> = Vec::new();
                    let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                    #update_assignments_repo_call
                    let dialect = <#backend_marker as OrmBackend>::DIALECT;
                    for (clause, column) in set_clauses.iter_mut().zip(changed_fields.iter()) {
                        let rhs = clause.split_once('=').map(|(_, rhs)| rhs.trim()).unwrap_or("?");
                        *clause = format!("{} = {rhs}", dialect.quote_identifier_path(column));
                    }
                    if #has_updated_at_column {
                        set_clauses.push(format!(
                            "{} = {}",
                            dialect.quote_identifier("updated_at"),
                            Self::__gom_current_updated_at_expr(),
                        ));
                    }
                    if let Some(assignment) = Self::__gom_managed_version_assignment() {
                        set_clauses.push(assignment);
                    }
                    if set_clauses.is_empty() {
                        return Err(Self::__gom_runtime_error("No fields to update"));
                    }
                    Self::__gom_require_managed_version_headroom(&current_entity)?;
                    let before_state = Self::__gom_capture_entity_state(&current_entity)?;
                    let changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                    hook_ctx.run_mutation_hook(None, &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit, table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: Self::__gom_event_id(key), changes: changes.clone(),
                        before_state: Some(before_state.clone()), after_state: None,
                    }).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    let key_start = values.len() + 1;
                    let sql = Self::__gom_rebind_sql(&format!(
                        "UPDATE {} SET {} WHERE {}{}",
                        dialect.quote_identifier_path(#table_name),
                        set_clauses.join(", "),
                        Self::__gom_key_where_clause_at(key_start),
                        Self::__gom_managed_version_guard(),
                    ), 1);
                    values.extend(Self::__gom_key_values(key));
                    let result = hook_ctx.execute(&sql, &values,).await?;
                    match <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&result) {
                        0 => return Ok(None),
                        1 => {}
                        _ => return Err(Self::__gom_runtime_error("complete composite key updated more than one row")),
                    }
                    let entity = Self::__gom_fetch_by_key_on(hook_ctx, key).await?
                        .ok_or(::graphql_orm::sqlx::Error::RowNotFound)?;
                    #repository_post_image_check
                    hook_ctx.run_mutation_hook(None, &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit, table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: Self::__gom_event_id(key), changes,
                        before_state: Some(before_state), after_state: Some(Self::__gom_capture_entity_state(&entity)?),
                    }).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    Self::__gom_queue_changed_event(hook_ctx, ::graphql_orm::graphql::orm::ChangeAction::Updated, Some(&entity)).await?;
                    Ok(Some(entity))
                }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }

                #[doc(hidden)]
                async fn __gom_delete_by_key_with_mutation_context(
                    hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    key: &#key_type,
                ) -> ::graphql_orm::Result<bool> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<bool> = async {
                    hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    use ::graphql_orm::graphql::orm::{OrmBackend, SqlDialect};
                    let db = hook_ctx.database().clone();
                    Self::__gom_require_repository_write_policy(&db)?;
                    let Some(entity) = Self::__gom_fetch_by_key_on(hook_ctx, key).await? else {
                        return Ok(false);
                    };
                    db.ensure_writable_row(None, #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                        &entity as &(dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    let before_state = Self::__gom_capture_entity_state(&entity)?;
                    hook_ctx.run_mutation_hook(None, &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        entity_name: #entity_name_lit, table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: Self::__gom_event_id(key), changes: Vec::new(),
                        before_state: Some(before_state.clone()), after_state: None,
                    }).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    let dialect = <#backend_marker as OrmBackend>::DIALECT;
                    let sql = format!(
                        "DELETE FROM {} WHERE {}",
                        dialect.quote_identifier_path(#table_name),
                        Self::__gom_key_where_clause_at(1),
                    );
                    let result = hook_ctx.execute(&sql, &Self::__gom_key_values(key),
                    ).await?;
                    match <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&result) {
                        0 => return Ok(false),
                        1 => {}
                        _ => return Err(Self::__gom_runtime_error("complete composite key deleted more than one row")),
                    }
                    hook_ctx.run_mutation_hook(None, &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        entity_name: #entity_name_lit, table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: Self::__gom_event_id(key), changes: Vec::new(),
                        before_state: Some(before_state), after_state: None,
                    }).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    Self::__gom_queue_changed_event(hook_ctx, ::graphql_orm::graphql::orm::ChangeAction::Deleted, Some(&entity)).await?;
                    Ok(true)
                }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }

                #[doc(hidden)]
                async fn __gom_update_where_bounded_with_mutation_context(
                    hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    where_input: #where_input,
                    input: #update_input,
                    limit: ::graphql_orm::graphql::orm::MutationLimit,
                ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome> = async {
                    hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    where_input.__gom_require_scalar_write_filter().map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                    use ::graphql_orm::graphql::orm::{DatabaseFilter, EntityQuery};
                    let db = hook_ctx.database().clone();
                    Self::__gom_require_repository_write_policy(&db)?;
                    if where_input.is_empty() {
                        return Err(Self::__gom_runtime_error("bounded update requires a non-empty typed filter"));
                    }
                    let query = EntityQuery::<Self, #backend_marker>::new()
                        .filter_with_entity_matching(&where_input);
                    let rows = hook_ctx
                        .__fetch_bounded_mutation_sentinel(query, limit)
                        .await?;
                    let maximum = usize::try_from(limit.get())
                        .map_err(|_| Self::__gom_runtime_error("bounded update limit conversion overflow"))?;
                    if rows.len() > maximum {
                        return Ok(::graphql_orm::graphql::orm::BoundedMutationOutcome::LimitExceeded {
                            maximum: limit.get(),
                        });
                    }
                    for row in &rows {
                        db.ensure_writable_row(None, #entity_name_lit,
                            <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                            ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                            row as &(dyn ::std::any::Any + Send + Sync),
                        ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    }
                    let selected = u32::try_from(rows.len())
                        .map_err(|_| Self::__gom_runtime_error("bounded update selected-row count overflow"))?;
                    let keys = rows.iter().map(Self::__gom_entity_key).collect::<Vec<_>>();
                    let mut affected = 0u32;
                    for key in keys {
                        if Self::__gom_update_by_key_with_mutation_context(hook_ctx, &key, input.clone()).await?.is_some() {
                            affected = affected.checked_add(1)
                                .ok_or_else(|| Self::__gom_runtime_error("bounded update affected-row count overflow"))?;
                        }
                    }
                    if affected != selected {
                        return Err(Self::__gom_runtime_error("bounded update cardinality changed"));
                    }
                    Ok(::graphql_orm::graphql::orm::BoundedMutationOutcome::Applied { affected })
                }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }

                #[doc(hidden)]
                async fn __gom_delete_where_bounded_with_mutation_context(
                    hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    where_input: #where_input,
                    limit: ::graphql_orm::graphql::orm::MutationLimit,
                ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome> = async {
                    hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    where_input.__gom_require_scalar_write_filter().map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                    use ::graphql_orm::graphql::orm::{DatabaseFilter, EntityQuery};
                    let db = hook_ctx.database().clone();
                    Self::__gom_require_repository_write_policy(&db)?;
                    if where_input.is_empty() {
                        return Err(Self::__gom_runtime_error("bounded delete requires a non-empty typed filter"));
                    }
                    let query = EntityQuery::<Self, #backend_marker>::new()
                        .filter_with_entity_matching(&where_input);
                    let rows = hook_ctx
                        .__fetch_bounded_mutation_sentinel(query, limit)
                        .await?;
                    let maximum = usize::try_from(limit.get())
                        .map_err(|_| Self::__gom_runtime_error("bounded delete limit conversion overflow"))?;
                    if rows.len() > maximum {
                        return Ok(::graphql_orm::graphql::orm::BoundedMutationOutcome::LimitExceeded {
                            maximum: limit.get(),
                        });
                    }
                    for row in &rows {
                        db.ensure_writable_row(None, #entity_name_lit,
                            <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                            ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                            row as &(dyn ::std::any::Any + Send + Sync),
                        ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    }
                    let selected = u32::try_from(rows.len())
                        .map_err(|_| Self::__gom_runtime_error("bounded delete selected-row count overflow"))?;
                    let keys = rows.iter().map(Self::__gom_entity_key).collect::<Vec<_>>();
                    let mut affected = 0u32;
                    for key in keys {
                        if Self::__gom_delete_by_key_with_mutation_context(hook_ctx, &key).await? {
                            affected = affected.checked_add(1)
                                .ok_or_else(|| Self::__gom_runtime_error("bounded delete affected-row count overflow"))?;
                        }
                    }
                    if affected != selected {
                        return Err(Self::__gom_runtime_error("bounded delete cardinality changed"));
                    }
                    Ok(::graphql_orm::graphql::orm::BoundedMutationOutcome::Applied { affected })
                }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }

                #[doc(hidden)]
                async fn __gom_update_if_with_mutation_context(
                    hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    key: &#key_type,
                    expected: #where_input,
                    mut input: #update_input,
                ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::PredicateUpdateOutcome<Self>> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<::graphql_orm::graphql::orm::PredicateUpdateOutcome<Self>> = async {
                    hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    use ::graphql_orm::graphql::orm::{DatabaseFilter, EntityQuery, OrmBackend, SqlDialect};
                    let db = hook_ctx.database().clone();
                    Self::__gom_require_repository_write_policy(&db)?;
                    if expected.is_empty() {
                        return Err(Self::__gom_runtime_error("conditional update requires a non-empty typed expected predicate"));
                    }
                    if expected.requires_in_memory_filtering(<#backend_marker as OrmBackend>::DIALECT) {
                        return Err(Self::__gom_runtime_error("conditional update predicate must be fully database-renderable"));
                    }
                    let Some(current_entity) = Self::__gom_fetch_by_key_on(hook_ctx, key).await? else {
                        return Ok(::graphql_orm::graphql::orm::PredicateUpdateOutcome::NotFound);
                    };
                    db.ensure_writable_row(None, #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                        &current_entity as &(dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::internal(#entity_name_lit, hook_ctx);
                    db.run_before_update_with_context(
                        &mut write_ctx,
                        Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                        &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    <Self as ::graphql_orm::graphql::orm::Entity>::authorize_repository_update_fields(
                        &db,
                        Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                        &input as &(dyn ::std::any::Any + Send + Sync),
                    ).await?;
                    Self::__gom_authorize_repository_update_field_writes(
                        &db,
                        Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                        &input,
                    ).await?;
                    let mut set_clauses: Vec<String> = Vec::new();
                    let mut changed_fields: Vec<&str> = Vec::new();
                    let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                    #update_assignments_repo_call
                    let dialect = <#backend_marker as OrmBackend>::DIALECT;
                    for (clause, column) in set_clauses.iter_mut().zip(changed_fields.iter()) {
                        let rhs = clause.split_once('=').map(|(_, rhs)| rhs.trim()).unwrap_or("?");
                        *clause = format!("{} = {rhs}", dialect.quote_identifier_path(column));
                    }
                    if #has_updated_at_column {
                        set_clauses.push(format!(
                            "{} = {}",
                            dialect.quote_identifier("updated_at"),
                            Self::__gom_current_updated_at_expr(),
                        ));
                    }
                    if let Some(assignment) = Self::__gom_managed_version_assignment() {
                        set_clauses.push(assignment);
                    }
                    if set_clauses.is_empty() {
                        return Err(Self::__gom_runtime_error("No fields to update"));
                    }
                    let expected_query = EntityQuery::<Self, #backend_marker>::new().filter(&expected);
                    let (delete_sql, expected_values) = expected_query.try_build_delete_sql()?;
                    let expected_clause = delete_sql.split_once(" WHERE ")
                        .map(|(_, clause)| clause)
                        .ok_or_else(|| Self::__gom_runtime_error("conditional predicate rendered empty SQL"))?;
                    Self::__gom_require_managed_version_headroom(&current_entity)?;
                    let before_state = Self::__gom_capture_entity_state(&current_entity)?;
                    let changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                    hook_ctx.run_mutation_hook(None, &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit, table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: Self::__gom_event_id(key), changes: changes.clone(),
                        before_state: Some(before_state.clone()), after_state: None,
                    }).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    let key_start = values.len() + 1;
                    let expected_start = key_start + <Self as ::graphql_orm::graphql::orm::DatabaseEntity>::PRIMARY_KEYS.len();
                    let expected_clause = Self::__gom_rebind_sql(expected_clause, expected_start);
                    let sql = Self::__gom_rebind_sql(&format!(
                        "UPDATE {} SET {} WHERE {}{} AND ({expected_clause})",
                        dialect.quote_identifier_path(#table_name),
                        set_clauses.join(", "),
                        Self::__gom_key_where_clause_at(key_start),
                        Self::__gom_managed_version_guard(),
                    ), 1);
                    values.extend(Self::__gom_key_values(key));
                    values.extend(expected_values);
                    let result = hook_ctx.execute(&sql, &values,).await?;
                    match <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&result) {
                        0 => {
                            return if Self::__gom_fetch_by_key_on(hook_ctx, key).await?.is_some() {
                                Ok(::graphql_orm::graphql::orm::PredicateUpdateOutcome::PredicateConflict)
                            } else {
                                Ok(::graphql_orm::graphql::orm::PredicateUpdateOutcome::NotFound)
                            };
                        }
                        1 => {}
                        _ => return Err(Self::__gom_runtime_error("conditional complete-key update affected more than one row")),
                    }
                    let entity = Self::__gom_fetch_by_key_on(hook_ctx, key).await?
                        .ok_or(::graphql_orm::sqlx::Error::RowNotFound)?;
                    #repository_post_image_check
                    hook_ctx.run_mutation_hook(None, &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit, table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: Self::__gom_event_id(key), changes,
                        before_state: Some(before_state), after_state: Some(Self::__gom_capture_entity_state(&entity)?),
                    }).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    Self::__gom_queue_changed_event(hook_ctx, ::graphql_orm::graphql::orm::ChangeAction::Updated, Some(&entity)).await?;
                    Ok(::graphql_orm::graphql::orm::PredicateUpdateOutcome::Updated(entity))
                }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }

                #upsert_helper_methods

                pub async fn insert(db: &::graphql_orm::db::Database<#backend_marker>, input: #create_input) -> ::graphql_orm::Result<Self> {
                    let mut context = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::Default,
                ).await?;
                    let entity = context.insert::<Self>(input).await?;
                    context.commit_and_emit().await?;
                    Ok(entity)
                }

                pub async fn insert_if_absent(
                    db: &::graphql_orm::db::Database<#backend_marker>, input: #create_input,
                ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::InsertIfAbsentOutcome<Self>> {
                    let mut context = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::Default,
                ).await?;
                    let outcome = context.insert_if_absent::<Self>(input).await?;
                    context.commit_and_emit().await?;
                    Ok(outcome)
                }

                pub async fn update_by_key(
                    db: &::graphql_orm::db::Database<#backend_marker>, key: &#key_type, input: #update_input,
                ) -> ::graphql_orm::Result<Option<Self>> {
                    let mut context = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::Default,
                ).await?;
                    let entity = context.update_by_key::<Self>(key, input).await?;
                    context.commit_and_emit().await?;
                    Ok(entity)
                }

                pub async fn delete_by_key(
                    db: &::graphql_orm::db::Database<#backend_marker>, key: &#key_type,
                ) -> ::graphql_orm::Result<bool> {
                    let mut context = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::Default,
                ).await?;
                    let deleted = context.delete_by_key::<Self>(key).await?;
                    context.commit_and_emit().await?;
                    Ok(deleted)
                }

                pub async fn update_where_bounded(
                    db: &::graphql_orm::db::Database<#backend_marker>,
                    where_input: #where_input,
                    input: #update_input,
                    limit: ::graphql_orm::graphql::orm::MutationLimit,
                ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome> {
                    let mut context = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                ).await?;
                    let outcome = context.update_where_bounded::<Self>(where_input, input, limit).await?;
                    context.commit_and_emit().await?;
                    Ok(outcome)
                }

                pub async fn delete_where_bounded(
                    db: &::graphql_orm::db::Database<#backend_marker>,
                    where_input: #where_input,
                    limit: ::graphql_orm::graphql::orm::MutationLimit,
                ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome> {
                    let mut context = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                ).await?;
                    let outcome = context.delete_where_bounded::<Self>(where_input, limit).await?;
                    context.commit_and_emit().await?;
                    Ok(outcome)
                }

                pub async fn update_if(
                    db: &::graphql_orm::db::Database<#backend_marker>,
                    key: &#key_type,
                    expected: #where_input,
                    input: #update_input,
                ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::PredicateUpdateOutcome<Self>> {
                    let mut context = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::Default,
                ).await?;
                    let outcome = context.update_if::<Self>(key, expected, input).await?;
                    context.commit_and_emit().await?;
                    Ok(outcome)
                }

                #upsert_repository_method
            }

            impl ::graphql_orm::graphql::orm::MutationContextInsert<#backend_marker> for #struct_name {
                type CreateInput = #create_input;
                fn insert_in_mutation_context<'a>(
                    context: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>, input: Self::CreateInput,
                ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<Self>> {
                    Box::pin(async move { Self::__gom_insert_with_mutation_context(context, input).await })
                }
            }

            impl ::graphql_orm::graphql::orm::MutationContextInsertIfAbsent<#backend_marker> for #struct_name {
                type CreateInput = #create_input;
                fn insert_if_absent_in_mutation_context<'a>(
                    context: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    input: Self::CreateInput,
                ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<::graphql_orm::graphql::orm::InsertIfAbsentOutcome<Self>>> {
                    Box::pin(async move { Self::__gom_insert_if_absent_with_mutation_context(context, input).await })
                }
            }

            impl ::graphql_orm::graphql::orm::MutationContextFindByKey<#backend_marker> for #struct_name {
                type Key = #key_type;
                fn find_by_key_in_mutation_context<'a>(
                    context: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>, key: &'a Self::Key,
                ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<Option<Self>>> {
                    Box::pin(async move {
                        let entity = Self::__gom_fetch_by_key_on(context, key).await?;
                        if let Some(entity) = entity.as_ref() {
                            if !context.database().can_read_row(None, #entity_name_lit,
                                <Self as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                                ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                                entity as &(dyn ::std::any::Any + Send + Sync),
                            ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)? {
                                return Ok(None);
                            }
                        }
                        Ok(entity)
                    })
                }
            }

            impl ::graphql_orm::graphql::orm::MutationContextUpdateByKey<#backend_marker> for #struct_name {
                type Key = #key_type;
                type UpdateInput = #update_input;
                fn update_by_key_in_mutation_context<'a>(
                    context: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    key: &'a Self::Key,
                    input: Self::UpdateInput,
                ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<Option<Self>>> {
                    Box::pin(async move { Self::__gom_update_by_key_with_mutation_context(context, key, input).await })
                }
            }

            impl ::graphql_orm::graphql::orm::MutationContextDeleteByKey<#backend_marker> for #struct_name {
                type Key = #key_type;
                fn delete_by_key_in_mutation_context<'a>(
                    context: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    key: &'a Self::Key,
                ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<bool>> {
                    Box::pin(async move { Self::__gom_delete_by_key_with_mutation_context(context, key).await })
                }
            }

            impl ::graphql_orm::graphql::orm::MutationContextBoundedUpdateWhere<#backend_marker> for #struct_name {
                type WhereInput = #where_input;
                type UpdateInput = #update_input;
                fn update_where_bounded_in_mutation_context<'a>(
                    context: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    where_input: Self::WhereInput,
                    input: Self::UpdateInput,
                    limit: ::graphql_orm::graphql::orm::MutationLimit,
                ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome>> {
                    Box::pin(async move { Self::__gom_update_where_bounded_with_mutation_context(context, where_input, input, limit).await })
                }
            }

            impl ::graphql_orm::graphql::orm::MutationContextBoundedDeleteWhere<#backend_marker> for #struct_name {
                type WhereInput = #where_input;
                fn delete_where_bounded_in_mutation_context<'a>(
                    context: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    where_input: Self::WhereInput,
                    limit: ::graphql_orm::graphql::orm::MutationLimit,
                ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome>> {
                    Box::pin(async move { Self::__gom_delete_where_bounded_with_mutation_context(context, where_input, limit).await })
                }
            }

            impl ::graphql_orm::graphql::orm::MutationContextPredicateUpdate<#backend_marker> for #struct_name {
                type Key = #key_type;
                type Expected = #where_input;
                type UpdateInput = #update_input;
                fn update_if_in_mutation_context<'a>(
                    context: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    key: &'a Self::Key,
                    expected: Self::Expected,
                    input: Self::UpdateInput,
                ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<::graphql_orm::graphql::orm::PredicateUpdateOutcome<Self>>> {
                    Box::pin(async move { Self::__gom_update_if_with_mutation_context(context, key, expected, input).await })
                }
            }

            #upsert_trait_impl
        }
    } else {
        quote! {}
    };

    let single_bounded_helper_methods = if !has_composite_primary_key
        && !schema_policy_read_only
        && !entity_meta.append_only
    {
        quote! {
            #[doc(hidden)]
            async fn __gom_update_where_bounded_with_mutation_context(
                hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                where_input: #where_input,
                input: #update_input,
                limit: ::graphql_orm::graphql::orm::MutationLimit,
            ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome> = async {
                hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                where_input.__gom_require_scalar_write_filter().map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                use ::graphql_orm::graphql::orm::{DatabaseFilter, EntityQuery};
                if where_input.is_empty() {
                    return Err(Self::__gom_runtime_error("bounded update requires a non-empty typed filter"));
                }
                let query = EntityQuery::<Self, #backend_marker>::new()
                    .filter_with_entity_matching(&where_input);
                let rows = hook_ctx
                    .__fetch_bounded_mutation_sentinel(query, limit)
                    .await?;
                let maximum = usize::try_from(limit.get())
                    .map_err(|_| Self::__gom_runtime_error("bounded update limit conversion overflow"))?;
                if rows.len() > maximum {
                    return Ok(::graphql_orm::graphql::orm::BoundedMutationOutcome::LimitExceeded { maximum: limit.get() });
                }
                let db = hook_ctx.database().clone();
                for row in &rows {
                    db.ensure_writable_row(None, #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                        row as &(dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                }
                let selected = u32::try_from(rows.len())
                    .map_err(|_| Self::__gom_runtime_error("bounded update selected-row count overflow"))?;
                let ids = rows.iter().map(|row| row.#pk_field.clone()).collect::<Vec<_>>();
                let mut affected = 0u32;
                for id in ids {
                    if Self::__gom_update_by_id_with_mutation_context(hook_ctx, &id, input.clone()).await?.is_some() {
                        affected = affected.checked_add(1)
                            .ok_or_else(|| Self::__gom_runtime_error("bounded update affected-row count overflow"))?;
                    }
                }
                if affected != selected {
                    return Err(Self::__gom_runtime_error("bounded update cardinality changed"));
                }
                Ok(::graphql_orm::graphql::orm::BoundedMutationOutcome::Applied { affected })
            }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }

            #[doc(hidden)]
            async fn __gom_delete_where_bounded_with_mutation_context(
                hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                where_input: #where_input,
                limit: ::graphql_orm::graphql::orm::MutationLimit,
            ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome> = async {
                hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                where_input.__gom_require_scalar_write_filter().map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                use ::graphql_orm::graphql::orm::{DatabaseFilter, EntityQuery};
                if where_input.is_empty() {
                    return Err(Self::__gom_runtime_error("bounded delete requires a non-empty typed filter"));
                }
                let query = EntityQuery::<Self, #backend_marker>::new()
                    .filter_with_entity_matching(&where_input);
                let rows = hook_ctx
                    .__fetch_bounded_mutation_sentinel(query, limit)
                    .await?;
                let maximum = usize::try_from(limit.get())
                    .map_err(|_| Self::__gom_runtime_error("bounded delete limit conversion overflow"))?;
                if rows.len() > maximum {
                    return Ok(::graphql_orm::graphql::orm::BoundedMutationOutcome::LimitExceeded { maximum: limit.get() });
                }
                let db = hook_ctx.database().clone();
                for row in &rows {
                    db.ensure_writable_row(None, #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                        row as &(dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                }
                let selected = u32::try_from(rows.len())
                    .map_err(|_| Self::__gom_runtime_error("bounded delete selected-row count overflow"))?;
                let ids = rows.iter().map(|row| row.#pk_field.clone()).collect::<Vec<_>>();
                let mut affected = 0u32;
                for id in ids {
                    if Self::__gom_delete_by_id_with_mutation_context(hook_ctx, &id).await? {
                        affected = affected.checked_add(1)
                            .ok_or_else(|| Self::__gom_runtime_error("bounded delete affected-row count overflow"))?;
                    }
                }
                if affected != selected {
                    return Err(Self::__gom_runtime_error("bounded delete cardinality changed"));
                }
                Ok(::graphql_orm::graphql::orm::BoundedMutationOutcome::Applied { affected })
            }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }
        }
    } else {
        quote! {}
    };

    let single_bounded_repository_methods = if !has_composite_primary_key
        && !schema_policy_read_only
        && !entity_meta.append_only
    {
        quote! {
            pub async fn update_where_bounded(
                db: &::graphql_orm::db::Database<#backend_marker>, where_input: #where_input,
                input: #update_input, limit: ::graphql_orm::graphql::orm::MutationLimit,
            ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome> {
                let mut context = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                ).await?;
                let outcome = context.update_where_bounded::<Self>(where_input, input, limit).await?;
                context.commit_and_emit().await?;
                Ok(outcome)
            }

            pub async fn delete_where_bounded(
                db: &::graphql_orm::db::Database<#backend_marker>, where_input: #where_input,
                limit: ::graphql_orm::graphql::orm::MutationLimit,
            ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome> {
                let mut context = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                ).await?;
                let outcome = context.delete_where_bounded::<Self>(where_input, limit).await?;
                context.commit_and_emit().await?;
                Ok(outcome)
            }
        }
    } else {
        quote! {}
    };

    let single_bounded_trait_impls = if !has_composite_primary_key
        && !schema_policy_read_only
        && !entity_meta.append_only
    {
        quote! {
            impl ::graphql_orm::graphql::orm::MutationContextBoundedUpdateWhere<#backend_marker> for #struct_name {
                type WhereInput = #where_input;
                type UpdateInput = #update_input;
                fn update_where_bounded_in_mutation_context<'a>(
                    context: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    where_input: Self::WhereInput, input: Self::UpdateInput,
                    limit: ::graphql_orm::graphql::orm::MutationLimit,
                ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome>> {
                    Box::pin(async move { Self::__gom_update_where_bounded_with_mutation_context(context, where_input, input, limit).await })
                }
            }
            impl ::graphql_orm::graphql::orm::MutationContextBoundedDeleteWhere<#backend_marker> for #struct_name {
                type WhereInput = #where_input;
                fn delete_where_bounded_in_mutation_context<'a>(
                    context: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                    where_input: Self::WhereInput, limit: ::graphql_orm::graphql::orm::MutationLimit,
                ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<::graphql_orm::graphql::orm::BoundedMutationOutcome>> {
                    Box::pin(async move { Self::__gom_delete_where_bounded_with_mutation_context(context, where_input, limit).await })
                }
            }
        }
    } else {
        quote! {}
    };

    let aggregate_definitions = if entity_meta.aggregate {
        quote! {
            /// One reviewed aggregate expression for this entity.
            #[derive(Clone, Debug, ::graphql_orm::async_graphql::InputObject)]
            #[graphql(rename_fields = #field_case_rule)]
            pub struct #aggregate_metric_input {
                /// Canonical aggregate operator.
                pub operator: ::graphql_orm::graphql::orm::GraphqlAggregateInputOperator,
                /// Selected public field. Omit only for row count.
                pub field: Option<#aggregate_field>,
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
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: Option<#where_input>,
                #[graphql(name = #group_by_arg_name, desc = #group_by_arg_description)] group_by: Vec<#aggregate_field>,
                #[graphql(name = #metrics_arg_name, desc = #metrics_arg_description)] metrics: Vec<#aggregate_metric_input>,
                #[graphql(name = #group_limit_arg_name, desc = #group_limit_arg_description)] group_limit: i32,
            ) -> ::graphql_orm::async_graphql::Result<Vec<::graphql_orm::graphql::orm::GraphqlAggregateResultRow>> {
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #aggregate_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                )?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                #filter_read_authorization
                let mut query = #struct_name::aggregate(db).with_auth(auth_context.as_ref());
                if let Some(filter) = where_input {
                    query = query.filter(filter);
                }
                for field in group_by {
                    query = query.group_by(field).map_err(
                        ::graphql_orm::graphql::errors::graphql_error_from_sqlx
                    )?;
                }
                for metric in metrics {
                    use ::graphql_orm::graphql::orm::GraphqlAggregateInputOperator as Operator;
                    query = match (metric.operator, metric.field) {
                        (Operator::Count, Some(field)) => query.count(field),
                        (Operator::Count, None) => query.count_rows(),
                        (Operator::Min, Some(field)) => query.min(field),
                        (Operator::Max, Some(field)) => query.max(field),
                        (Operator::Sum, Some(field)) => query.sum(field),
                        (_, None) => Err(::graphql_orm::sqlx::Error::Protocol(
                            "aggregate metric field is required for this operator".to_owned()
                        )),
                    }.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                }
                query = query.group_limit(i64::from(group_limit)).map_err(
                    ::graphql_orm::graphql::errors::graphql_error_from_sqlx
                )?;
                let rows = query.fetch_with_graphql_context(ctx).await.map_err(
                    ::graphql_orm::graphql::errors::graphql_error_from_sqlx
                )?;
                Ok(rows.into_iter().map(Into::into).collect())
            }
        }
    } else {
        quote! {}
    };
    let backend_name = backend.name();
    let entity_identity = match &entity_meta.stable_type_id {
        Some(identity) => quote! { #identity },
        None => quote! { concat!(module_path!(), "::", stringify!(#struct_name)) },
    };
    // Argument lists are read from the resolver signatures after expansion;
    // see `catalog_arguments`. Descriptors name only their root coordinate.
    use crate::catalog_arguments::{RootKind, placeholder as root_arguments};
    // Every generated root field, recorded as its descriptor is built, for
    // the `schema_roots!` collision check against handwritten roots.
    let generated_root_fields = ::std::cell::RefCell::new(Vec::<(RootKind, String)>::new());
    let descriptor = |kind: RootKind,
                      category: proc_macro2::TokenStream,
                      field_name: &str,
                      description: &str,
                      rust_result_type: proc_macro2::TokenStream,
                      operation_authorization: &proc_macro2::TokenStream| {
        let kind_tokens = match kind {
            RootKind::Query => quote! { ::graphql_orm::graphql::orm::GraphqlOperationKind::Query },
            RootKind::Mutation => {
                quote! { ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation }
            }
            RootKind::Subscription => {
                quote! { ::graphql_orm::graphql::orm::GraphqlOperationKind::Subscription }
            }
        };
        generated_root_fields
            .borrow_mut()
            .push((kind, field_name.to_owned()));
        let arguments = root_arguments(kind, field_name);
        quote! {
            ::graphql_orm::graphql::orm::GeneratedGraphqlOperationDescriptor::generated_with_authorization_and_description(
                #entity_identity,
                stringify!(#struct_name),
                #table_name,
                #backend_name,
                #kind_tokens,
                #category,
                #field_name,
                #description,
                #arguments,
                stringify!(#rust_result_type),
                <#rust_result_type as ::graphql_orm::async_graphql::OutputType>
                    ::qualified_type_name(),
                #resolver_schema_signature,
                #operation_authorization,
            ).with_diagnostic_rust_type(concat!(module_path!(), "::", stringify!(#struct_name)))
        }
    };
    let with_query_shape = |descriptor: proc_macro2::TokenStream,
                            shape: proc_macro2::TokenStream| {
        quote! { #descriptor.with_query_shape(#shape) }
    };
    let ai_mutation_policy =
        |category: &str| match entity_meta.ai_mutations.get(category).map(String::as_str) {
            Some("automatic") => quote! {
                ::graphql_orm::graphql::orm::AiMutationExecutionPolicy::Automatic
            },
            Some("approval_required") => quote! {
                ::graphql_orm::graphql::orm::AiMutationExecutionPolicy::ApprovalRequired
            },
            Some("prohibited") | None => quote! {
                ::graphql_orm::graphql::orm::AiMutationExecutionPolicy::Prohibited
            },
            Some(_) => unreachable!("entity parser validates AI mutation policies"),
        };
    let mutation_descriptor =
        |category: proc_macro2::TokenStream,
         category_name: &str,
         field_name: &str,
         description: &str,
         rust_result_type: proc_macro2::TokenStream,
         operation_authorization: &proc_macro2::TokenStream| {
            let policy = ai_mutation_policy(category_name);
            generated_root_fields
                .borrow_mut()
                .push((RootKind::Mutation, field_name.to_owned()));
            let arguments = root_arguments(RootKind::Mutation, field_name);
            quote! {
                ::graphql_orm::graphql::orm::GeneratedGraphqlOperationDescriptor::generated_with_authorization_description_and_ai_policy(
                    #entity_identity,
                    stringify!(#struct_name),
                    #table_name,
                    #backend_name,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation,
                    #category,
                    #field_name,
                    #description,
                    #arguments,
                    stringify!(#rust_result_type),
                    <#rust_result_type as ::graphql_orm::async_graphql::OutputType>
                        ::qualified_type_name(),
                    #resolver_schema_signature,
                    #operation_authorization,
                    #policy,
                ).with_diagnostic_rust_type(concat!(module_path!(), "::", stringify!(#struct_name)))
            }
        };

    let mut operation_descriptors = vec![with_query_shape(
        descriptor(
            RootKind::Query,
            quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlOperationCategory::List },
            &list_query_name,
            &list_operation_description,
            quote! { #connection_type },
            &list_operation_authorization,
        ),
        quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlQueryShape::List },
    )];
    if entity_meta.aggregate {
        operation_descriptors.push(with_query_shape(
            descriptor(
                RootKind::Query,
                quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlOperationCategory::Aggregate },
                &aggregate_query_name,
                &aggregate_operation_description,
                quote! { Vec<::graphql_orm::graphql::orm::GraphqlAggregateResultRow> },
                &aggregate_operation_authorization,
            ),
            quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlQueryShape::Aggregate },
        ));
    }
    if has_search {
        operation_descriptors.push(with_query_shape(
            descriptor(
                RootKind::Query,
                quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlOperationCategory::Search },
                &search_query_name,
                &search_operation_description,
                quote! { #search_connection_type },
                &search_operation_authorization,
            ),
            quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlQueryShape::Search },
        ));
    }
    if keyset_parts.is_some() {
        operation_descriptors.push(with_query_shape(
            descriptor(
                RootKind::Query,
                quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlOperationCategory::KeysetList },
                &keyset_query_name,
                &keyset_operation_description,
                quote! { #connection_type },
                &keyset_list_operation_authorization,
            ),
            quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlQueryShape::Keyset },
        ));
        operation_descriptors.push(with_query_shape(
            descriptor(
                RootKind::Query,
                quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlOperationCategory::KeysetList },
                &scan_query_name,
                scan_operation_description,
                quote! { #scan_connection_type },
                &keyset_list_operation_authorization,
            ),
            quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlQueryShape::Scan },
        ));
    }
    operation_descriptors.push(with_query_shape(
        descriptor(
            RootKind::Query,
            quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlOperationCategory::SingleRead },
            &single_query_name,
            &single_operation_description,
            quote! { Option<#struct_name> },
            &single_read_operation_authorization,
        ),
        quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlQueryShape::Single },
    ));

    let create_descriptor = || {
        mutation_descriptor(
            quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlOperationCategory::Create },
            "create",
            &create_mutation_name,
            &create_operation_description,
            quote! { #result_type },
            &create_operation_authorization,
        )
    };
    let subscription_descriptor = || {
        descriptor(
            RootKind::Subscription,
            quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlOperationCategory::Subscription },
            &subscription_name,
            &subscription_operation_description,
            quote! { #changed_event },
            &subscription_operation_authorization,
        )
    };

    if entity_meta.append_only {
        if generates_public_create {
            operation_descriptors.push(create_descriptor());
        }
        operation_descriptors.push(subscription_descriptor());
    } else if backend_writable && !schema_policy_read_only && !has_composite_primary_key {
        if generates_public_create {
            operation_descriptors.push(create_descriptor());
        }
        if generates_public_upsert {
            operation_descriptors.push(mutation_descriptor(
                quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlOperationCategory::Upsert },
                "upsert",
                &upsert_mutation_name,
                &upsert_operation_description,
                quote! { #upsert_result_type },
                &upsert_operation_authorization,
            ));
        }
        if generates_public_update {
            operation_descriptors.push(mutation_descriptor(
                quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlOperationCategory::Update },
                "update",
                &update_mutation_name,
                &update_operation_description,
                quote! { #result_type },
                &update_operation_authorization,
            ));
            operation_descriptors.push(mutation_descriptor(
                quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlOperationCategory::UpdateMany },
                "update_many",
                &update_many_mutation_name,
                &update_many_operation_description,
                quote! { #update_many_result_type },
                &update_many_operation_authorization,
            ));
        }
        operation_descriptors.push(mutation_descriptor(
            quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlOperationCategory::Delete },
            "delete",
            &delete_mutation_name,
            &delete_operation_description,
            quote! { #result_type },
            &delete_operation_authorization,
        ));
        operation_descriptors.push(mutation_descriptor(
            quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlOperationCategory::DeleteMany },
            "delete_many",
            &delete_many_mutation_name,
            &delete_many_operation_description,
            quote! { #delete_many_result_type },
            &delete_many_operation_authorization,
        ));
        operation_descriptors.push(subscription_descriptor());
    } else {
        operation_descriptors.push(subscription_descriptor());
    }
    if emits_write_placeholder || emits_append_only_write_placeholder {
        operation_descriptors.push(mutation_descriptor(
            quote! { ::graphql_orm::graphql::orm::GeneratedGraphqlOperationCategory::Unavailable },
            "unavailable",
            &write_placeholder_field,
            write_placeholder_description,
            quote! { bool },
            &quote! { ::graphql_orm::graphql::orm::GraphqlAuthorizationRequirement::unsatisfiable() },
        ));
    }

    // Federation entity keys are resolved here because the generated
    // `{Entity}Queries` object below is the only place an `#[graphql(entity)]`
    // resolver — and therefore a resolvable `@key` — can be emitted.
    let primary_key_rust_names = primary_key_fields
        .iter()
        .map(|field| field.rust_name.clone())
        .collect::<Vec<_>>();
    let federation_keys = crate::federation::resolve_federation_keys(
        struct_name,
        &entity_meta,
        fields,
        graphql_rename_fields,
        serde_rename_all,
        backend,
        &primary_key_rust_names,
    )?;
    let single_read_argument_names = if has_composite_primary_key {
        primary_key_fields
            .iter()
            .map(|field| {
                (
                    field.rust_name.clone(),
                    apply_graphql_case(&field.graphql_name, argument_case),
                )
            })
            .collect::<Vec<_>>()
    } else {
        vec![(first_primary_key.rust_name.clone(), id_arg_name.clone())]
    };
    let federation_scope_enforcements = federation_keys
        .iter()
        .map(|key| {
            let arguments =
                crate::federation::scope_template_aliases(key, &single_read_argument_names)
                    .into_iter()
                    .map(|(graphql_name, rust_ident, rust_type)| {
                        ScopeTemplateArgumentSpec::scalar(graphql_name, rust_ident, rust_type)
                    })
                    .collect::<Vec<_>>();
            scope_enforcement_tokens(
                "single_read",
                operation_authorizations.get("single_read"),
                &arguments,
            )
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let federation_entity_resolvers = crate::federation::federation_entity_resolver_tokens(
        struct_name,
        &entity_name_lit,
        &backend_marker,
        &resolver_auth_mode,
        &relation_preload_single,
        &federation_keys,
        &federation_scope_enforcements,
        &single_query_name,
    );
    let node_id_entity_resolver = match crate::federation::resolve_node_id_parts(
        struct_name,
        &entity_meta,
        &primary_key_fields,
        backend,
    )? {
        Some(parts) => {
            let mut arguments = Vec::new();
            for (index, (field, part)) in primary_key_fields.iter().zip(&parts).enumerate() {
                let ident = crate::federation::node_id_part_ident(index);
                let mut names = vec![field.graphql_name.clone()];
                for (rust_name, argument_name) in &single_read_argument_names {
                    if rust_name == &field.rust_name && !names.contains(argument_name) {
                        names.push(argument_name.clone());
                    }
                }
                for name in names {
                    arguments.push(ScopeTemplateArgumentSpec::scalar(
                        name,
                        ident.clone(),
                        part.rust_type.clone(),
                    ));
                }
            }
            let scope_enforcement = scope_enforcement_tokens(
                "single_read",
                operation_authorizations.get("single_read"),
                &arguments,
            )?;
            crate::federation::node_id_entity_resolver_tokens(
                struct_name,
                &entity_name_lit,
                &backend_marker,
                &resolver_auth_mode,
                &relation_preload_single,
                &parts,
                &scope_enforcement,
                &single_query_name,
            )
        }
        None => quote! {},
    };
    if let Some(shapes) = &entity_meta.query_shapes {
        for shape in shapes {
            let available = match shape.as_str() {
                "keyset" | "scan" => keyset_parts.is_some(),
                "search" => has_search,
                "aggregate" => entity_meta.aggregate,
                _ => true,
            };
            if !available {
                return Err(syn::Error::new_spanned(
                    struct_name,
                    format!("query_shapes selects unavailable {shape} capability"),
                ));
            }
        }
    }
    let disabled_query_fields = [
        ("single", single_query_name.as_str()),
        ("list", list_query_name.as_str()),
        ("keyset", keyset_query_name.as_str()),
        ("scan", scan_query_name.as_str()),
        ("search", search_query_name.as_str()),
        ("aggregate", aggregate_query_name.as_str()),
    ]
    .into_iter()
    .filter(|(shape, _)| !query_shape_enabled(shape))
    .map(|(_, name)| name)
    .collect::<Vec<_>>();
    let query_shape_metadata_filter = if disabled_query_fields.is_empty() {
        quote! {}
    } else {
        quote! {
            operations.retain(|operation| !(operation.kind() == ::graphql_orm::graphql::orm::GraphqlOperationKind::Query
                && [#(#disabled_query_fields),*].contains(&operation.field_name())));
        }
    };
    let mut disabled_mutation_fields = entity_meta
        .mutation_shapes
        .as_ref()
        .map(|selected| {
            [
                ("create", create_mutation_name.as_str()),
                ("upsert", upsert_mutation_name.as_str()),
                ("update", update_mutation_name.as_str()),
                ("update_many", update_many_mutation_name.as_str()),
                ("delete", delete_mutation_name.as_str()),
                ("delete_many", delete_many_mutation_name.as_str()),
            ]
            .into_iter()
            .filter(|(shape, _)| !selected.iter().any(|name| name == shape))
            .map(|(_, name)| name)
            .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if entity_meta.dependent_deletes {
        disabled_mutation_fields.push(delete_many_mutation_name.as_str());
    }
    let mutation_shape_metadata_filter = if disabled_mutation_fields.is_empty() {
        quote! {}
    } else {
        quote! {
            operations.retain(|operation| !(operation.kind() == ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation
                && [#(#disabled_mutation_fields),*].contains(&operation.field_name())));
        }
    };
    // The root fields each generated root object exposes: the descriptors
    // above, less the shapes the entity does not select.
    let exposed_root_fields = |kind: RootKind, disabled: &[&str]| {
        generated_root_fields
            .borrow()
            .iter()
            .filter(|(field_kind, name)| *field_kind == kind && !disabled.contains(&name.as_str()))
            .map(|(_, name)| name.clone())
            .collect::<Vec<_>>()
    };
    let query_root_fields = exposed_root_fields(RootKind::Query, &disabled_query_fields);
    let mutation_root_fields = exposed_root_fields(RootKind::Mutation, &disabled_mutation_fields);
    let subscription_root_fields = exposed_root_fields(RootKind::Subscription, &[]);
    let operation_metadata_impl = quote! {
        impl #struct_name {
            #[doc(hidden)]
            pub const __GOM_QUERY_ROOT_FIELDS: &'static [&'static str] =
                &[#(#query_root_fields),*];
            #[doc(hidden)]
            pub const __GOM_MUTATION_ROOT_FIELDS: &'static [&'static str] =
                &[#(#mutation_root_fields),*];
            #[doc(hidden)]
            pub const __GOM_SUBSCRIPTION_ROOT_FIELDS: &'static [&'static str] =
                &[#(#subscription_root_fields),*];
        }

        impl ::graphql_orm::graphql::orm::GraphqlOperationMetadata for #struct_name {
            fn generated_graphql_operations(
            ) -> &'static [::graphql_orm::graphql::orm::GeneratedGraphqlOperationDescriptor] {
                static OPERATIONS: ::std::sync::OnceLock<
                    Box<[::graphql_orm::graphql::orm::GeneratedGraphqlOperationDescriptor]>
                > = ::std::sync::OnceLock::new();
                OPERATIONS.get_or_init(|| {
                    let mut operations = ::std::vec::Vec::from([
                        #(#operation_descriptors),*
                    ]);
                    #query_shape_metadata_filter
                    #mutation_shape_metadata_filter
                    operations.sort_by(|left, right| {
                        (
                            left.kind(),
                            left.field_name(),
                            left.category(),
                        ).cmp(&(
                            right.kind(),
                            right.field_name(),
                            right.category(),
                        ))
                    });
                    operations.into_boxed_slice()
                })
            }
        }
    };

    let subscription_release_key_field = if has_composite_primary_key {
        quote! { key: event.key.clone(), }
    } else {
        quote! {}
    };
    let generated_subscription_surface = quote! {
        /// Generated subscriptions for this entity
        #[derive(Default)]
        pub struct #subscriptions_struct;

        #[::graphql_orm::async_graphql::Subscription]
        impl #subscriptions_struct {
            #[doc = #subscription_operation_description]
            #[graphql(name = #subscription_name, #subscription_requires_scopes #authenticated_directive)]
            async fn on_changed(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #filter_arg_name, desc = #filter_arg_description)] _filter: Option<::graphql_orm::graphql::orm::SubscriptionFilterInput>,
                #[graphql(name = #subscription_key_arg_name, desc = "Exact complete primary key to observe")] key: Option<#key_type>,
                #[graphql(name = #subscription_keys_arg_name, desc = #subscription_keys_arg_description)] keys: Option<Vec<#key_type>>,
            ) -> ::graphql_orm::async_graphql::Result<impl ::graphql_orm::futures::Stream<Item = ::graphql_orm::async_graphql::Result<::graphql_orm::graphql::subscriptions::ChangedPayload<#changed_event>>>> {
                use ::graphql_orm::futures::StreamExt;
                    ::graphql_orm::graphql::subscriptions::admit_generated_operation(ctx)?;
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #subscription_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Subscription,
                )?;
                let db = ctx.data::<::graphql_orm::db::Database<#backend_marker>>().map_err(|_| {
                    ::graphql_orm::async_graphql::Error::new(
                        "graphql-orm Database runtime not registered; build the schema with schema_builder(database) or add Database to schema data",
                    )
                })?;
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlSubscription,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                    if _filter.as_ref().is_some_and(|filter| filter.dummy.is_some()) {
                        return Err(::graphql_orm::graphql::errors::OrmPublicError::new(::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput).into_graphql_error());
                    }
                    ::graphql_orm::graphql::subscriptions::validate_subscription_selection::<#struct_name, #backend_marker>(ctx, #entity_result_field_name)?;
                    let keys = ::graphql_orm::graphql::subscriptions::admit_subscription_keys(
                        db,
                        key.is_some(),
                        keys.map(|keys| keys.iter().map(#struct_name::__gom_key_values).collect()),
                    )?;
                let actions = _filter.and_then(|filter| filter.actions);
                    let rx = db.ensure_event_sender::<::graphql_orm::graphql::subscriptions::SubscriptionEvent<#changed_event, #backend_marker>>().subscribe();
                    let db = db.clone();
                    let interval = db.subscription_authority_recheck_interval();
                    let timer_db = db.clone();
                    let events = ::graphql_orm::graphql::subscriptions::continuity_events(rx).filter_map(move |result| {
                        let db = db.clone();
                        let actions = actions.clone();
                        let key = key.clone();
                        let keys = keys.clone();
                        async move {
                            let witness = match result {
                                Ok(witness) => witness,
                                // Lost continuity or a closed event source: one error, then the stream ends.
                                Err(error) => return Some(Err(error)),
                            };
                            // Host policies may buffer private diagnostics before denying an event.
                            // Cover every filter and release-proof drop, not only payload preparation.
                            let errors_before = ctx.query_env.errors.lock().unwrap().len();
                            let delivered: Option<::graphql_orm::graphql::subscriptions::ChangedPayload<#changed_event>> = async move {
                                let event = witness.event.as_ref();
                                if key.as_ref().is_some_and(|key| ::graphql_orm::graphql::loaders::RelationKey::from_sql_values(&#struct_name::__gom_key_values(key)) != ::graphql_orm::graphql::loaders::RelationKey::from_sql_values(&witness.keys)) { return None; }
                                if keys.as_ref().is_some_and(|keys| !keys.contains(&::graphql_orm::graphql::loaders::RelationKey::from_sql_values(&witness.keys))) { return None; }
                                if ::graphql_orm::graphql::loaders::RelationKey::from_sql_values(&witness.keys)
                                    != ::graphql_orm::graphql::loaders::RelationKey::from_sql_values(&#struct_name::__gom_key_values(&#subscription_event_key)) { return None; }
                                if actions.as_ref().is_some_and(|actions| !actions.contains(&event.action)) { return None; }
                                let authorize_current = || async {
                                    let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                                    #subscription_scope_enforcement
                                    ::graphql_orm::graphql::assurance::enforce_resolver_assurance(ctx, ::graphql_orm::graphql::orm::GraphqlOperationKind::Subscription)?;
                                    witness.authorize_provenance(&db, ctx, event.source_entity.as_deref(), event.source_id.as_deref(), &event.path, event.change_kind == ::graphql_orm::graphql::orm::ChangeKind::Propagated).await?;
                                    if witness.trusted_delete
                                        && event.entity.as_ref().is_some_and(|entity|
                                            ::graphql_orm::graphql::loaders::RelationKey::from_sql_values(&#struct_name::__gom_subscription_key_values(entity))
                                            != ::graphql_orm::graphql::loaders::RelationKey::from_sql_values(&witness.keys)
                                        ) { return Err(::graphql_orm::graphql::errors::OrmPublicError::forbidden().into_graphql_error()); }
                                    let preimage = if witness.trusted_delete && event.action == ::graphql_orm::graphql::orm::ChangeAction::Deleted { event.entity.as_ref() } else { None };
                                    let row = ::graphql_orm::graphql::subscriptions::authorize_subscription_row::<#struct_name, #backend_marker>(&db, ctx, &witness.keys, preimage).await?
                                        .ok_or_else(|| ::graphql_orm::graphql::errors::OrmPublicError::forbidden().into_graphql_error())?;
                                    if key.is_some() || keys.is_some() {
                                        ::graphql_orm::graphql::subscriptions::authorize_subscription_keys::<#struct_name, #backend_marker>(&db, ctx, &row).await?;
                                    }
                                    ::graphql_orm::graphql::subscriptions::authorize_subscription_fields(&db, ctx, &row, #entity_result_field_name).await?;
                                    // Query and field-policy awaits must not outlive current authority.
                                    db.ensure_entity_access(Some(ctx), #entity_name_lit,
                                        <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                                        ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                                        ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlSubscription).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                                    let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                                    #subscription_scope_enforcement
                                    ::graphql_orm::graphql::assurance::enforce_resolver_assurance(ctx, ::graphql_orm::graphql::orm::GraphqlOperationKind::Subscription)?;
                                    witness.authorize_provenance(&db, ctx, event.source_entity.as_deref(), event.source_id.as_deref(), &event.path, event.change_kind == ::graphql_orm::graphql::orm::ChangeKind::Propagated).await?;
                                    #struct_name::__gom_authorize_changed_operation(ctx)?;
                                    Ok::<#struct_name, ::graphql_orm::async_graphql::Error>(row)
                                };
                                match authorize_current().await {
                                    Ok(row) => {
                                        let prepared = ::graphql_orm::graphql::subscriptions::ChangedPayload::prepare(#changed_event {
                                            action: event.action,
                                            change_kind: event.change_kind,
                                            id: event.id.clone(),
                                            #subscription_release_key_field
                                            source_entity: event.source_entity.clone(),
                                            source_id: event.source_id.clone(),
                                            path: event.path.clone(),
                                            entity: Some(row),
                                        }, ctx).await?;
                                        // Successful serialization can itself await or revoke authority.
                                        // Recheck the complete current release contract before yielding.
                                        match authorize_current().await {
                                            Ok(_) => Some(prepared),
                                            Err(error) => { let _ = ::graphql_orm::graphql::loaders::relation_authorization_error(error); None }
                                        }
                                    },
                                    Err(error) => { let _ = ::graphql_orm::graphql::loaders::relation_authorization_error(error); None }
                                }
                            }.await;
                            if delivered.is_none() {
                                ctx.query_env.errors.lock().unwrap().truncate(errors_before);
                            }
                            delivered.map(Ok)
                        }
                    });
                    Ok(::graphql_orm::graphql::subscriptions::watch_subscription_authority(
                        events, interval, move || {
                            let db = timer_db.clone();
                            async move {
                                let result = async {
                                #struct_name::__gom_authorize_changed_operation(ctx)?;
                                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                                    ctx, ::graphql_orm::graphql::orm::GraphqlOperationKind::Subscription)?;
                                if !db.can_access_entity(Some(ctx), #entity_name_lit,
                                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                                    ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlSubscription).await? {
                                    return Err(::graphql_orm::graphql::errors::OrmPublicError::forbidden().into_graphql_error());
                                }
                                // Current principal/scopes/assurance must survive the policy await.
                                #struct_name::__gom_authorize_changed_operation(ctx)?;
                                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                                    ctx, ::graphql_orm::graphql::orm::GraphqlOperationKind::Subscription)
                                }.await;
                                if result.is_err() {
                                    // A timer may preempt a pending event preparation.
                                    // Never attach its private diagnostics to the terminal frame.
                                    ctx.query_env.errors.lock().unwrap().clear();
                                }
                                result
                            }
                        }
                    ))
            }
        }

    };
    let generated_propagation_driver = quote! {
            #[doc(hidden)]
            #[allow(clippy::too_many_arguments)]
            #subscription_key_visibility fn __gom_propagate_changed_event<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                action: ::graphql_orm::graphql::orm::ChangeAction,
                entity: &'a Self,
                source_entity: &'a str,
                source_id: &'a str,
                path: Vec<String>,
                visited: &'a mut ::std::collections::HashSet<String>,
                provenance: &'a [::graphql_orm::graphql::subscriptions::SubscriptionEventSource<#backend_marker>],
            ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<()>> {
                Box::pin(async move {
                    if provenance.len() >= ::graphql_orm::graphql::subscriptions::MAX_SUBSCRIPTION_PROVENANCE_HOPS
                            || visited.len() >= ::graphql_orm::graphql::subscriptions::MAX_SUBSCRIPTION_PROPAGATION_EVENTS { return Ok(()); }
                    #(#propagation_blocks)*
                    Ok(())
                })
            }

    };
    let readonly_composite_changed = if (has_composite_primary_key
        || schema_policy_read_only
        || !backend_writable)
        && !entity_meta.append_only
    {
        quote! {
            #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject, serde::Serialize, serde::Deserialize)]
            #[graphql(name = #changed_event_str, rename_fields = #field_case_rule)]
            pub struct #changed_event {
                pub action: ::graphql_orm::graphql::orm::ChangeAction,
                #[graphql(name = #change_kind_field_name)]
                pub change_kind: ::graphql_orm::graphql::orm::ChangeKind,
                pub id: #subscription_identity_type,
                #subscription_extra_key_field
                #[graphql(name = #source_entity_field_name)]
                pub source_entity: Option<String>,
                #[graphql(name = #source_id_field_name)]
                pub source_id: Option<String>,
                pub path: Vec<String>,
                #[graphql(name = #entity_result_field_name)]
                pub entity: Option<#struct_name>,
            }
            #generated_subscription_surface
            impl #struct_name {
                #subscription_key_method
                #propagated_subscription_emitter
                #generated_propagation_driver
            }
        }
    } else {
        quote! {}
    };

    if !backend_writable
        || schema_policy_read_only
        || has_composite_primary_key
        || entity_meta.append_only
    {
        return Ok(quote! {
            #operation_metadata_impl

            impl #struct_name {
                #managed_version_helper_methods
            }

            // ============================================================================
            // Connection/Edge Types (for pagination)
            // ============================================================================

            #key_type_definition

            #[derive(::graphql_orm::async_graphql::SimpleObject, Debug, Clone)]
            #[graphql(name = #edge_type_str, rename_fields = #field_case_rule)]
            pub struct #edge_type {
                pub node: #struct_name,
                pub cursor: String,
            }

            #[derive(::graphql_orm::async_graphql::SimpleObject, Debug, Clone)]
            #[graphql(name = #connection_type_str, rename_fields = #field_case_rule)]
            pub struct #connection_type {
                pub edges: Vec<#edge_type>,
                #[graphql(name = #page_info_field_name)]
                pub page_info: ::graphql_orm::graphql::pagination::PageInfo,
            }

            impl #connection_type {
                pub fn from_generic(conn: ::graphql_orm::graphql::pagination::Connection<#struct_name>) -> Self {
                    Self {
                        edges: conn.edges.into_iter().map(|e| #edge_type {
                            node: e.node,
                            cursor: e.cursor,
                        }).collect(),
                        page_info: conn.page_info,
                    }
                }

                pub fn empty() -> Self {
                    Self {
                        edges: Vec::new(),
                        page_info: ::graphql_orm::graphql::pagination::PageInfo::default(),
                    }
                }
            }

            #search_connection_definitions
            #aggregate_definitions
            #authorized_keyset_definitions

            // ============================================================================
            // Query Struct
            // ============================================================================

            #[derive(Default)]
            pub struct #queries_struct;

            #[::graphql_orm::async_graphql::Object]
            impl #queries_struct {
                #aggregate_query_method

                #[doc = #list_operation_description]
                #[graphql(#list_query_skip name = #list_query_name, #list_requires_scopes #authenticated_directive)]
                async fn list(
                    &self,
                    ctx: &::graphql_orm::async_graphql::Context<'_>,
                    #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: Option<#where_input>,
                    #[graphql(name = #order_by_arg_name, desc = #order_by_arg_description)] order_by: Option<Vec<#order_by_input>>,
                    #[graphql(name = #page_arg_name, desc = #page_arg_description)] page: Option<::graphql_orm::graphql::orm::PageInput>,
                ) -> ::graphql_orm::async_graphql::Result<#connection_type> {
                    use ::graphql_orm::graphql::orm::{DatabaseOrderBy, EntityQuery};

                    let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                    #list_scope_enforcement
                    ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                        ctx,
                        ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                    )?;
                    let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                    let pool = db.pool();
                    let auth_context = ctx
                        .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                        .cloned();
                    db.ensure_entity_access(
                        Some(ctx),
                        #entity_name_lit,
                        <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                        ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                    ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                    #filter_read_authorization
                    #order_read_authorization
                    let mut query = EntityQuery::<#struct_name, #backend_marker>::new();

                    if let Some(ref filter) = where_input {
                        query = query.filter_with_entity_matching(filter);
                    }

                    if let Some(ref orders) = order_by {
                        for order in orders {
                            query = query.order_by_with_context(order, ctx)?;
                        }
                    }

                    if query.order_clauses.is_empty() {
                        query = query.default_order();
                    }

                    let visibility = db.read_visibility::<#struct_name>(
                        Some(ctx), ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                    ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                    if visibility.requires_residual_checks() {
                        let resolved_page = db.pagination_config().resolve_page(page.as_ref(), true);
                        let authorized_page = ::graphql_orm::graphql::loaders::scan_authorized_query(
                            db, ctx, query, auth_context.as_ref(), &visibility, resolved_page,
                            ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                        ).await?;
                        let total = authorized_page.total_count;
                        let offset = authorized_page.offset.max(0) as usize;
                        let paged_rows = authorized_page.entities;

                        let has_next_page = (offset as i64 + paged_rows.len() as i64) < total;
                        let has_previous_page = offset > 0;

                        let mut generic_conn = ::graphql_orm::graphql::pagination::Connection {
                            edges: paged_rows.into_iter().enumerate().map(|(index, node)| {
                                ::graphql_orm::graphql::pagination::Edge {
                                    cursor: ::graphql_orm::graphql::pagination::encode_cursor((offset + index) as i64),
                                    node,
                                }
                            }).collect::<Vec<_>>(),
                            page_info: ::graphql_orm::graphql::pagination::PageInfo {
                                has_next_page,
                                has_previous_page,
                                start_cursor: None,
                                end_cursor: None,
                                total_count: Some(total),
                            },
                        };

                        generic_conn.page_info.start_cursor = generic_conn.edges.first().map(|edge| edge.cursor.clone());
                        generic_conn.page_info.end_cursor = generic_conn.edges.last().map(|edge| edge.cursor.clone());

                        #relation_preload_list

                        return Ok(#connection_type::from_generic(generic_conn));
                    }

                    query = query.with_read_visibility(&visibility)?;
                    if let Some(ref p) = page {
                        query = query.paginate(p);
                    }

                    let mut generic_conn = query
                        .fetch_connection_selected_with_auth(db, auth_context.as_ref(), ::graphql_orm::graphql::pagination::ConnectionSelection::from_context(ctx))
                        .await
                        .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                    #relation_preload_list

                    Ok(#connection_type::from_generic(generic_conn))
                }

                #search_query_method
                #keyset_graphql_method

                #[doc = #single_operation_description]
                #[graphql(#single_query_skip name = #single_query_name, #single_read_requires_scopes #authenticated_directive)]
                async fn get_by_id(
                    &self,
                    ctx: &::graphql_orm::async_graphql::Context<'_>,
                    #(#single_query_args)*
                ) -> ::graphql_orm::async_graphql::Result<Option<#struct_name>> {
                    use ::graphql_orm::graphql::orm::EntityQuery;

                    let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                    #single_read_scope_enforcement
                    #single_query_key_init
                    ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                        ctx,
                        ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                    )?;
                    let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                    let pool = db.pool();
                    let auth_context = ctx
                        .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                        .cloned();
                    db.ensure_entity_access(
                        Some(ctx),
                        #entity_name_lit,
                        <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                        ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                    ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                    // The same row authority as generated list reads.
                    let mut entity = ::graphql_orm::graphql::orm::fetch_authorized_point_row(
                        db,
                        ctx,
                        EntityQuery::<#struct_name, #backend_marker>::new()
                            .where_values(&#struct_name::__gom_key_where_clause(), #struct_name::__gom_key_values(&key)),
                    ).await?;

                    #relation_preload_single

                    Ok(entity)
                }

                #federation_entity_resolvers
                #node_id_entity_resolver
            }

            impl #struct_name {
                #key_condition_method

                pub fn query<'a>(pool: &'a #pool_type) -> ::graphql_orm::graphql::orm::FindQuery<'a, Self, #where_input, #order_by_input, #backend_marker> {
                    ::graphql_orm::graphql::orm::FindQuery::new(pool)
                }

                #search_repository_method
                #keyset_repository_methods
                #search_rebuild_methods

                #read_repository_key_methods

                pub fn count_query<'a>(pool: &'a #pool_type) -> ::graphql_orm::graphql::orm::EntityCountQuery<'a, Self, #where_input, #backend_marker> {
                    ::graphql_orm::graphql::orm::EntityCountQuery::new(pool)
                }
            }

            #append_only_surface
            #retention_surface
            #composite_mutation_surface
            #composite_write_stubs
            #readonly_composite_changed
        });
    }

    // A GraphQL input object must declare at least one field, so the update
    // input and the roots that accept it exist only with a public member.
    let graphql_update_input_definition = if generates_public_update {
        quote! {
            #[derive(::graphql_orm::async_graphql::InputObject, Clone, Debug, Default)]
            #[graphql(name = #update_input_str, rename_fields = #field_case_rule)]
            struct #graphql_update_input {
                #(#graphql_update_input_fields)*
            }

            impl From<#graphql_update_input> for #update_input {
                fn from(input: #graphql_update_input) -> Self {
                    Self {
                        #(#update_input_from_graphql_fields)*
                    }
                }
            }
        }
    } else {
        quote! {}
    };
    let update_graphql_method = if generates_public_update {
        quote! {
            #[doc = #update_operation_description]
            #[graphql(name = #update_mutation_name, #update_requires_scopes #authenticated_directive)]
            async fn update(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #id_arg_name, desc = #id_arg_description)] id: #pk_type,
                #[graphql(name = #input_arg_name, desc = #input_arg_description)] input: #graphql_update_input,
                #expected_version_param
            ) -> ::graphql_orm::async_graphql::Result<#result_type> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};
                let __gom_expected_version: Option<i64> = #expected_version_binding;

                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #update_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation,
                )?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                let pool = db.pool();
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                let mut input: #update_input = input.into();
                #[allow(unused_mut)]
                let mut __gom_client_supplied: ::std::collections::HashSet<&'static str> =
                    ::std::collections::HashSet::new();
                {
                    let __gom_client_input = &input;
                    #(#update_client_supplied)*
                }
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin_with_auth(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                    auth_context.as_ref(),
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                let current_entity = ::std::boxed::Box::pin(::graphql_orm::graphql::orm::with_write_target_prefilter(
                    db,
                    Some(ctx),
                    EntityQuery::<#struct_name, #backend_marker>::new()
                        .where_clause(
                            &format!("{} = {}", #sql_dialect.render_identifier_path(#struct_name::PRIMARY_KEY), #struct_name::__gom_placeholder(1)),
                            #pk_bind_value
                        ),
                ))
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?
                    .fetch_one_for_write_in_transaction(&mut hook_ctx)
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                if current_entity.is_none() {
                    return Ok(#result_type::err("Entity not found"));
                }
                let current_entity = current_entity.expect("checked above");
                if !db.authorize_write_target(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                    &current_entity as &(dyn ::std::any::Any + Send + Sync),
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)? {
                    // Neither readable nor writable: indistinguishable from a missing row.
                    return Ok(#result_type::err("Entity not found"));
                }
                if __gom_expected_version.is_some()
                    && #struct_name::__gom_managed_version_of(&current_entity) != __gom_expected_version
                {
                    return Err(::graphql_orm::graphql::errors::OrmPublicError::new(
                        ::graphql_orm::graphql::errors::OrmErrorCode::Conflict,
                    ).into_graphql_error());
                }
                {
                    let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::graphql_in_transaction(
                        ctx,
                        #entity_name_lit,
                        &mut hook_ctx,
                    );
                    db.run_before_update_with_context(
                        &mut write_ctx,
                        Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                        &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                }

                // Build dynamic UPDATE SQL based on provided fields
                let mut set_clauses: Vec<String> = Vec::new();
                let mut changed_fields: Vec<&str> = Vec::new();
                let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();

                #(#update_policy_checks)*
                #update_assignments_graphql_call

                // Update timestamp column when this entity defines one
                if #has_updated_at_column {
                    set_clauses.push(format!("updated_at = {}", #struct_name::__gom_current_updated_at_expr()));
                }
                if let Some(assignment) = #struct_name::__gom_managed_version_assignment() {
                    set_clauses.push(assignment);
                }

                if set_clauses.is_empty() {
                    return Ok(#result_type::err("No fields to update"));
                }
                #struct_name::__gom_require_managed_version_headroom(&current_entity)
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                let before_state = #struct_name::__gom_capture_entity_state(&current_entity)
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                hook_ctx.run_mutation_hook(
                    Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #pk_id_for_id,
                        changes: mutation_changes.clone(),
                        before_state: Some(before_state.clone()),
                        after_state: None,
                    },
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                let sql = #struct_name::__gom_rebind_sql(&format!(
                    "UPDATE {} SET {} WHERE {} = ?{}",
                    #sql_table_name,
                    set_clauses.join(", "),
                    #sql_dialect.render_identifier_path(#struct_name::PRIMARY_KEY),
                    format!(
                        "{}{}",
                        #struct_name::__gom_managed_version_guard(),
                        #struct_name::__gom_managed_version_expectation(__gom_expected_version),
                    ),
                ), 1);

                // Add the ID to the values for the WHERE clause
                values.push(#pk_bind_value);

                let result = hook_ctx.execute(&sql, &values).await;

                match result {
                    Ok(r) if <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&r) == 1 => {
                        let entity = #struct_name::__gom_fetch_by_id_on(&mut hook_ctx, &id)
                            .await
                            .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                        match entity {
                            Some(entity) => {
                                #graphql_post_image_check
                                let after_state = Some(
                                    #struct_name::__gom_capture_entity_state(&entity)
                                        .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?
                                );

                                hook_ctx.run_mutation_hook(
                                    Some(ctx),
                                    &::graphql_orm::graphql::orm::MutationEvent {
                                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                                        entity_name: #entity_name_lit,
                                        table_name: #table_name,
                                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                                        id: #pk_id_for_entity,
                                        changes: mutation_changes.clone(),
                                        before_state: Some(before_state),
                                        after_state,
                                    },
                                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                                #struct_name::__gom_queue_changed_event(
                                    &mut hook_ctx,
                                    ::graphql_orm::graphql::orm::ChangeAction::Updated,
                                    Some(&entity),
                                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                                hook_ctx.commit_and_emit().await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                                // Invoke optional post-mutation hook.
                                #notify_on_updated

                                let mut result = #result_type::ok(entity);
                                #mutation_output_release
                                Ok(result)
                            },
                            None => Ok(#result_type::err("Entity not found after update")),
                        }
                    }
                    Ok(_) => Err(::graphql_orm::async_graphql::Error::new("authorized update did not affect the exact locked row")),
                    Err(e) => Ok(#result_type::err(::graphql_orm::graphql::errors::graphql_error_from_sqlx(e).message)),
                }
            }
        }
    } else {
        quote! {}
    };
    let update_many_graphql_method = if generates_public_update {
        quote! {
            #[doc = #update_many_operation_description]
            #[graphql(name = #update_many_mutation_name, #update_many_requires_scopes #authenticated_directive)]
            async fn update_many(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: Option<#where_input>,
                #[graphql(name = #input_arg_name, desc = #input_arg_description)] input: #graphql_update_input,
            ) -> ::graphql_orm::async_graphql::Result<#update_many_result_type> {
                use ::graphql_orm::graphql::orm::{DatabaseFilter, EntityQuery};

                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #update_many_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation,
                )?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                if let Err(error) = db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                ).await {
                    return Ok(#update_many_result_type::from_graphql_error(::graphql_orm::graphql::errors::graphql_error_from_callback(error)));
                }
                if let Err(error) = db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy) {
                    return Ok(#update_many_result_type::from_graphql_error(error));
                }
                let input: #update_input = input.into();

                let filter = match where_input {
                    Some(ref f) if !f.is_empty() => f,
                    _ => return Ok(#update_many_result_type::err("Where filter is required for bulk update and must not be empty")),
                };

                if let Err(error) = filter.__gom_require_scalar_write_filter() {
                    return Ok(#update_many_result_type::from_graphql_error(error.into_graphql_error()));
                }
                // A bulk-write filter reveals the fields it names through the
                // affected count: it needs the same read authority as a query.
                if let Err(error) = ::graphql_orm::graphql::orm::authorize_read_fields::<#struct_name, #backend_marker>(
                    db, ctx, ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter),
                ).await {
                    return Ok(#update_many_result_type::from_graphql_error(error));
                }
                let mut notification_rows = Vec::new();
                match #struct_name::__gom_update_where_with_auth(db, filter.clone(), input, auth_context.as_ref(), Some(ctx), Some(&mut notification_rows)).await {
                    Ok(affected_count) => {
                        #[allow(unused_mut)]
                        let mut result = #update_many_result_type::ok(affected_count);
                        #notify_bulk_updated
                        Ok(result)
                    },
                    Err(e) => {
                        let error = ::graphql_orm::graphql::errors::graphql_error_from_sqlx(e);
                        Ok(#update_many_result_type::from_graphql_error(error))
                    },
                }
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #operation_metadata_impl

        // ============================================================================
        // Connection/Edge Types (for pagination)
        // ============================================================================

        #key_type_definition

        /// Edge containing a node and cursor
        #[derive(::graphql_orm::async_graphql::SimpleObject, Debug, Clone)]
        #[graphql(name = #edge_type_str, rename_fields = #field_case_rule)]
        pub struct #edge_type {
            /// The item at the end of the edge
            pub node: #struct_name,
            /// A cursor for pagination
            pub cursor: String,
        }

        /// Connection containing edges and page info
        #[derive(::graphql_orm::async_graphql::SimpleObject, Debug, Clone)]
        #[graphql(name = #connection_type_str, rename_fields = #field_case_rule)]
        pub struct #connection_type {
            /// The edges in this connection
            pub edges: Vec<#edge_type>,
            /// Pagination information
            #[graphql(name = #page_info_field_name)]
            pub page_info: ::graphql_orm::graphql::pagination::PageInfo,
        }

        impl #connection_type {
            /// Create from a generic Connection
            pub fn from_generic(conn: ::graphql_orm::graphql::pagination::Connection<#struct_name>) -> Self {
                Self {
                    edges: conn.edges.into_iter().map(|e| #edge_type {
                        node: e.node,
                        cursor: e.cursor,
                    }).collect(),
                    page_info: conn.page_info,
                }
            }

            /// Create an empty connection
            pub fn empty() -> Self {
                Self {
                    edges: Vec::new(),
                    page_info: ::graphql_orm::graphql::pagination::PageInfo::default(),
                }
            }
        }

        #search_connection_definitions

        // ============================================================================
        // Create/Update Input Types
        // ============================================================================

        /// Input for creating an entity.
        #[derive(Clone, Debug)]
        pub struct #create_input {
            #(#create_input_fields)*
        }

        /// Input for updating an entity.
        #[derive(Clone, Debug, Default)]
        pub struct #update_input {
            #(#update_input_fields)*
        }

        #update_assignment_helpers

        #graphql_create_input_definition

        #graphql_update_input_definition

        /// Mutation result for this entity.
        #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
        #[graphql(name = #result_type_str, rename_fields = #field_case_rule)]
        pub struct #result_type {
            pub success: bool,
            pub error: Option<String>,
            #[graphql(name = #entity_result_field_name)]
            pub entity: Option<#struct_name>,
        }

        impl #result_type {
            /// Create a successful result with the entity
            pub fn ok(entity: #struct_name) -> Self {
                Self { success: true, error: None, entity: Some(entity) }
            }
            /// Create an error result
            pub fn err(msg: impl Into<String>) -> Self {
                Self { success: false, error: Some(msg.into()), entity: None }
            }
        }

        #upsert_result_definition

        /// Subscription change event for this entity.
        #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject, serde::Serialize, serde::Deserialize)]
        #[graphql(name = #changed_event_str, rename_fields = #field_case_rule)]
        pub struct #changed_event {
            pub action: ::graphql_orm::graphql::orm::ChangeAction,
            #[graphql(name = #change_kind_field_name)]
            pub change_kind: ::graphql_orm::graphql::orm::ChangeKind,
            pub id: #pk_type,
            #[graphql(name = #source_entity_field_name)]
            pub source_entity: Option<String>,
            #[graphql(name = #source_id_field_name)]
            pub source_id: Option<String>,
            pub path: Vec<String>,
            #[graphql(name = #entity_result_field_name)]
            pub entity: Option<#struct_name>,
        }

        /// Result of bulk delete by Where filter
        #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
        #[graphql(name = #delete_many_result_type_str, rename_fields = #field_case_rule)]
        pub struct #delete_many_result_type {
            pub success: bool,
            /// Failures after commit; success and count still describe the committed write.
            pub notification_errors: Option<Vec<::graphql_orm::graphql::orm::NotificationError>>,
            pub error: Option<String>,
            /// Application refusal code, or the safe database error category.
            pub error_code: Option<String>,
            #[graphql(name = #deleted_count_field_name)]
            pub deleted_count: i64,
        }

        impl #delete_many_result_type {
            pub fn ok(deleted_count: i64) -> Self {
                Self { notification_errors: None, success: true, error: None, error_code: None, deleted_count }
            }
            pub fn err(msg: impl Into<String>) -> Self {
                Self { notification_errors: None, success: false, error: Some(msg.into()), error_code: None, deleted_count: 0 }
            }
            /// Create a refusal payload preserving the GraphQL message and string code.
            pub fn from_graphql_error(error: ::graphql_orm::async_graphql::Error) -> Self {
                let error_code = error.extensions.as_ref().and_then(|ext| ext.get("code")).and_then(|value| match value {
                    ::graphql_orm::async_graphql::Value::String(code) => Some(code.clone()),
                    _ => None,
                });
                Self { error_code, ..Self::err(error.message) }
            }

        }

        /// Result of bulk update by Where filter
        #[derive(Debug, Clone, ::graphql_orm::async_graphql::SimpleObject)]
        #[graphql(name = #update_many_result_type_str, rename_fields = #field_case_rule)]
        pub struct #update_many_result_type {
            pub success: bool,
            /// Failures after commit; success and count still describe the committed write.
            pub notification_errors: Option<Vec<::graphql_orm::graphql::orm::NotificationError>>,
            pub error: Option<String>,
            /// Application refusal code, or the safe database error category.
            pub error_code: Option<String>,
            #[graphql(name = #affected_count_field_name)]
            pub affected_count: i64,
        }

        impl #update_many_result_type {
            pub fn ok(affected_count: i64) -> Self {
                Self { notification_errors: None, success: true, error: None, error_code: None, affected_count }
            }
            pub fn err(msg: impl Into<String>) -> Self {
                Self { notification_errors: None, success: false, error: Some(msg.into()), error_code: None, affected_count: 0 }
            }
            /// Create a refusal payload preserving the GraphQL message and string code.
            pub fn from_graphql_error(error: ::graphql_orm::async_graphql::Error) -> Self {
                let error_code = error.extensions.as_ref().and_then(|ext| ext.get("code")).and_then(|value| match value {
                    ::graphql_orm::async_graphql::Value::String(code) => Some(code.clone()),
                    _ => None,
                });
                Self { error_code, ..Self::err(error.message) }
            }

        }

        // ============================================================================
        // Query Struct
        // ============================================================================

        #aggregate_definitions
            #authorized_keyset_definitions

        /// Generated queries for this entity
        #[derive(Default)]
        pub struct #queries_struct;

        #[::graphql_orm::async_graphql::Object]
        impl #queries_struct {
            #aggregate_query_method

            #[doc = #list_operation_description]
            #[graphql(#list_query_skip name = #list_query_name, #list_requires_scopes #authenticated_directive)]
            async fn list(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: Option<#where_input>,
                #[graphql(name = #order_by_arg_name, desc = #order_by_arg_description)] order_by: Option<Vec<#order_by_input>>,
                #[graphql(name = #page_arg_name, desc = #page_arg_description)] page: Option<::graphql_orm::graphql::orm::PageInput>,
            ) -> ::graphql_orm::async_graphql::Result<#connection_type> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, DatabaseOrderBy, EntityQuery, FromSqlRow};
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #list_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                )?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                let pool = db.pool();
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                #filter_read_authorization
                #order_read_authorization
                let mut query = EntityQuery::<#struct_name, #backend_marker>::new();

                if let Some(ref filter) = where_input {
                    query = query.filter_with_entity_matching(filter);
                }

                if let Some(ref orders) = order_by {
                    for order in orders {
                        query = query.order_by_with_context(order, ctx)?;
                    }
                }

                if query.order_clauses.is_empty() {
                    query = query.default_order();
                }

                let visibility = db.read_visibility::<#struct_name>(
                    Some(ctx), ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                if visibility.requires_residual_checks() {
                    let resolved_page = db.pagination_config().resolve_page(page.as_ref(), true);
                    let authorized_page = ::graphql_orm::graphql::loaders::scan_authorized_query(
                        db, ctx, query, auth_context.as_ref(), &visibility, resolved_page,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                    ).await?;
                    let total = authorized_page.total_count;
                    let offset = authorized_page.offset.max(0) as usize;
                    let paged_rows = authorized_page.entities;

                    let has_next_page = (offset as i64 + paged_rows.len() as i64) < total;
                    let has_previous_page = offset > 0;

                    let mut generic_conn = ::graphql_orm::graphql::pagination::Connection {
                        edges: paged_rows.into_iter().enumerate().map(|(index, node)| {
                            ::graphql_orm::graphql::pagination::Edge {
                                cursor: ::graphql_orm::graphql::pagination::encode_cursor((offset + index) as i64),
                                node,
                            }
                        }).collect::<Vec<_>>(),
                        page_info: ::graphql_orm::graphql::pagination::PageInfo {
                            has_next_page,
                            has_previous_page,
                            start_cursor: None,
                            end_cursor: None,
                            total_count: Some(total),
                        },
                    };

                    generic_conn.page_info.start_cursor = generic_conn.edges.first().map(|edge| edge.cursor.clone());
                    generic_conn.page_info.end_cursor = generic_conn.edges.last().map(|edge| edge.cursor.clone());

                    #relation_preload_list

                    return Ok(#connection_type::from_generic(generic_conn));
                }

                query = query.with_read_visibility(&visibility)?;
                if let Some(ref p) = page {
                    query = query.paginate(p);
                }

                let mut generic_conn = query
                    .fetch_connection_selected_with_auth(db, auth_context.as_ref(), ::graphql_orm::graphql::pagination::ConnectionSelection::from_context(ctx))
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                #relation_preload_list

                Ok(#connection_type::from_generic(generic_conn))
            }

            #search_query_method
            #keyset_graphql_method

            #[doc = #single_operation_description]
            #[graphql(#single_query_skip name = #single_query_name, #single_read_requires_scopes #authenticated_directive)]
            async fn get_by_id(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #(#single_query_args)*
            ) -> ::graphql_orm::async_graphql::Result<Option<#struct_name>> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow};
                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #single_read_scope_enforcement
                #single_query_key_init
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Query,
                )?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                let pool = db.pool();
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                // The same row authority as generated list reads.
                let mut entity = ::graphql_orm::graphql::orm::fetch_authorized_point_row(
                    db,
                    ctx,
                    EntityQuery::<#struct_name, #backend_marker>::new()
                        .where_values(&#struct_name::__gom_key_where_clause(), #struct_name::__gom_key_values(&key)),
                ).await?;

                #relation_preload_single

                Ok(entity)
            }

            #federation_entity_resolvers
                #node_id_entity_resolver
        }

        // ============================================================================
        // Mutation Struct
        // ============================================================================

        /// Generated mutations for this entity
        #[derive(Default)]
        pub struct #mutations_struct;

        #[::graphql_orm::async_graphql::Object]
        impl #mutations_struct {
            #create_graphql_method

            #upsert_graphql_method

            #update_graphql_method

            #[doc = #delete_operation_description]
            #[graphql(name = #delete_mutation_name, #delete_requires_scopes #authenticated_directive)]
            async fn delete(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #id_arg_name, desc = #id_arg_description)] id: #pk_type,
                #expected_version_param
            ) -> ::graphql_orm::async_graphql::Result<#result_type> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, SqlValue};
                let __gom_expected_version: Option<i64> = #expected_version_binding;

                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #delete_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation,
                )?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                let pool = db.pool();
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin_with_auth(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                    auth_context.as_ref(),
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                // Fetch entity before deletion for notification purposes
                let entity = ::std::boxed::Box::pin(::graphql_orm::graphql::orm::with_write_target_prefilter(
                    db,
                    Some(ctx),
                    EntityQuery::<#struct_name, #backend_marker>::new()
                        .where_clause(
                            &format!("{} = {}", #sql_dialect.render_identifier_path(#struct_name::PRIMARY_KEY), #struct_name::__gom_placeholder(1)),
                            #pk_bind_value
                        ),
                ))
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?
                    .fetch_one_for_write_in_transaction(&mut hook_ctx)
                    .await
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                if entity.is_none() {
                    return Ok(#result_type::err("Entity not found"));
                }
                let entity = entity.unwrap();
                if !db.authorize_write_target(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                    &entity as &(dyn ::std::any::Any + Send + Sync),
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)? {
                    // Neither readable nor writable: indistinguishable from a missing row.
                    return Ok(#result_type::err("Entity not found"));
                }
                if __gom_expected_version.is_some()
                    && #struct_name::__gom_managed_version_of(&entity) != __gom_expected_version
                {
                    return Err(::graphql_orm::graphql::errors::OrmPublicError::new(
                        ::graphql_orm::graphql::errors::OrmErrorCode::Conflict,
                    ).into_graphql_error());
                }
                let before_state = #struct_name::__gom_capture_entity_state(&entity)
                    .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                hook_ctx.run_mutation_hook(
                    Some(ctx),
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #pk_id_for_entity,
                        changes: Vec::new(),
                        before_state: Some(before_state.clone()),
                        after_state: None,
                    },
                ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;

                let sql = #struct_name::__gom_rebind_sql(
                    &format!(
                        "DELETE FROM {} WHERE {} = ?{}",
                        #sql_table_name,
                        #sql_dialect.render_identifier_path(#struct_name::PRIMARY_KEY),
                        #struct_name::__gom_managed_version_expectation(__gom_expected_version),
                    ),
                    1
                );
                let values = [#pk_bind_value];
                let result = hook_ctx.execute(&sql, &values).await;

                match result {
                    Ok(r) if <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&r) == 1 => {
                        hook_ctx.run_mutation_hook(
                            Some(ctx),
                            &::graphql_orm::graphql::orm::MutationEvent {
                                phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                                entity_name: #entity_name_lit,
                                table_name: #table_name,
                                metadata: <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata(),
                                id: #pk_id_for_entity,
                                changes: Vec::new(),
                                before_state: Some(before_state),
                                after_state: None,
                            },
                        ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_callback)?;
                        #struct_name::__gom_queue_changed_event(
                            &mut hook_ctx,
                            ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                            Some(&entity),
                        ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                        hook_ctx.commit_and_emit().await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;

                        // Invoke optional post-mutation hook.
                        #notify_on_deleted

                        Ok(#result_type {
                            success: true,
                            error: None,
                            entity: None
                        })
                    },
                    Ok(_) => Err(::graphql_orm::async_graphql::Error::new("authorized delete did not affect the exact locked row")),
                    Err(e) => Ok(#result_type::err(::graphql_orm::graphql::errors::graphql_error_from_sqlx(e).message)),
                }
            }

            #update_many_graphql_method

            #[doc = #delete_many_operation_description]
            #[graphql(name = #delete_many_mutation_name, #delete_many_requires_scopes #authenticated_directive)]
            async fn delete_many(
                &self,
                ctx: &::graphql_orm::async_graphql::Context<'_>,
                #[graphql(name = #where_arg_name, desc = #where_arg_description)] where_input: Option<#where_input>,
            ) -> ::graphql_orm::async_graphql::Result<#delete_many_result_type> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, EntityQuery};

                let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?; #rls_request_guard
                #delete_many_scope_enforcement
                ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
                    ctx,
                    ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation,
                )?;
                let db = ctx.data_unchecked::<::graphql_orm::db::Database<#backend_marker>>();
                let auth_context = ctx
                    .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                    .cloned();
                if let Err(error) = db.ensure_entity_access(
                    Some(ctx),
                    #entity_name_lit,
                    <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation,
                ).await {
                    return Ok(#delete_many_result_type::from_graphql_error(::graphql_orm::graphql::errors::graphql_error_from_callback(error)));
                }
                if let Err(error) = db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy) {
                    return Ok(#delete_many_result_type::from_graphql_error(error));
                }

                let filter = match where_input {
                    Some(ref f) if !f.is_empty() => f,
                    _ => return Ok(#delete_many_result_type::err("Where filter is required for bulk delete and must not be empty")),
                };

                if let Err(error) = filter.__gom_require_scalar_write_filter() {
                    return Ok(#delete_many_result_type::from_graphql_error(error.into_graphql_error()));
                }
                // A bulk-write filter reveals the fields it names through the
                // affected count: it needs the same read authority as a query.
                if let Err(error) = ::graphql_orm::graphql::orm::authorize_read_fields::<#struct_name, #backend_marker>(
                    db, ctx, ::graphql_orm::graphql::orm::DatabaseFilter::referenced_fields(filter),
                ).await {
                    return Ok(#delete_many_result_type::from_graphql_error(error));
                }
                let mut notification_rows = Vec::new();
                match #struct_name::__gom_delete_where_with_auth(db, filter.clone(), auth_context.as_ref(), Some(ctx), Some(&mut notification_rows)).await {
                    Ok(deleted_count) => {
                        #[allow(unused_mut)]
                        let mut result = #delete_many_result_type::ok(deleted_count);
                        #notify_bulk_deleted
                        Ok(result)
                    },
                    Err(e) => Ok(#delete_many_result_type::from_graphql_error(::graphql_orm::graphql::errors::graphql_error_from_sqlx(e))),
                }
            }
        }

        // ============================================================================
        // Subscription Struct
        // ============================================================================

        #generated_subscription_surface

        // ============================================================================
        // Repository Trait Implementation
        // ============================================================================

        /// Repository implementation for this entity
        ///
        /// Provides static async methods for common database operations.
        impl #struct_name {
            #[doc(hidden)]
            fn __gom_runtime_error(message: impl Into<String>) -> ::graphql_orm::sqlx::Error {
                ::graphql_orm::sqlx::Error::Protocol(message.into())
            }

            #key_condition_method

            #[doc(hidden)]
            fn __gom_emit_changed_event(
                db: &::graphql_orm::db::Database<#backend_marker>,
                action: ::graphql_orm::graphql::orm::ChangeAction,
                entity: Option<&Self>,
            ) {
                if let Some(entity) = entity {
                    let event = #changed_event {
                        action,
                        change_kind: ::graphql_orm::graphql::orm::ChangeKind::Direct,
                        id: entity.#pk_field.clone(),
                        source_entity: None,
                        source_id: None,
                        path: Vec::new(),
                        entity: Some(entity.clone()),
                    };
                    db.emit_event(event.clone());
                    #emit_subscription_witness
                }
            }

            #subscription_key_method
            #propagated_subscription_emitter

            #generated_propagation_driver

            #[doc(hidden)]
            async fn __gom_queue_changed_event(
                hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                action: ::graphql_orm::graphql::orm::ChangeAction,
                entity: Option<&Self>,
            ) -> ::graphql_orm::Result<()> {
                if let Some(entity) = entity {
                    #search_write_maintenance
                    let source_id = #pk_id_for_entity;
                    let event = #changed_event {
                        action,
                        change_kind: ::graphql_orm::graphql::orm::ChangeKind::Direct,
                        id: entity.#pk_field.clone(),
                        source_entity: None,
                        source_id: None,
                        path: Vec::new(),
                        entity: Some(entity.clone()),
                    };
                    hook_ctx.queue_event(event.clone());
                    #queue_subscription_witness
                    #propagation_visited_seed
                    Self::__gom_propagate_changed_event(
                        hook_ctx,
                        action,
                        entity,
                        #entity_name_lit,
                        &source_id,
                        Vec::new(),
                        &mut visited,
                        &[],
                    ).await?;
                }
                Ok(())
            }

            #[doc(hidden)]
            fn __gom_capture_entity_state(
                entity: &Self,
            ) -> ::graphql_orm::Result<::graphql_orm::graphql::orm::EntityState> {
                ::graphql_orm::graphql::orm::entity_state(entity)
            }

            #[doc(hidden)]
            #[allow(clippy::ptr_arg)]
            pub(crate) async fn __gom_fetch_by_id_on(
                hook_ctx: &mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                id: &#pk_type_ty,
            ) -> ::graphql_orm::Result<Option<Self>> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, FromSqlRow, SqlValue};

                let sql = Self::__gom_rebind_sql(
                    &format!(
                        "SELECT {} FROM {}{} WHERE {} = ?{}",
                        <Self as ::graphql_orm::graphql::orm::DatabaseEntity>::column_names().join(", "),
                        #sql_table_name,
                        #upsert_read_lock_hint,
                        #sql_dialect.render_identifier_path(Self::PRIMARY_KEY),
                        #write_read_lock_suffix,
                    ),
                    1,
                );
                let values = [#pk_bind_value_ref];
                let rows = hook_ctx.fetch_rows(&sql, &values).await?;
                rows.first()
                    .map(<Self as ::graphql_orm::graphql::orm::FromSqlRow<#backend_marker>>::from_row)
                    .transpose()
            }

            #[doc(hidden)]
            async fn __gom_insert_with_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                mut input: #create_input,
            ) -> ::graphql_orm::Result<Self> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<Self> = async {
                hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, SqlValue};

                let db = hook_ctx.database().clone();
                let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::internal(
                    #entity_name_lit,
                    hook_ctx,
                );
                db.run_before_create_with_context(
                    &mut write_ctx,
                    &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                <Self as ::graphql_orm::graphql::orm::Entity>::authorize_repository_create_fields(
                    &db,
                    &input as &(dyn ::std::any::Any + Send + Sync),
                ).await?;
                #(#repository_create_field_checks)*
                #created_pk_init
                let sql = Self::__gom_rebind_sql(#insert_sql, 1);
                let mut bind_values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();
                #prepend_pk_bind
                #(#insert_binds_repo)*

                let before_state = None;
                hook_ctx
                    .run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: #created_pk_id_string,
                            changes: ::graphql_orm::graphql::orm::mutation_changes(&[#(#create_mutation_field_literals),*], &bind_values),
                            before_state,
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

                hook_ctx.execute(&sql, &bind_values).await?;
                let entity = Self::__gom_fetch_by_id_on(hook_ctx, &created_pk).await?
                    .ok_or(::graphql_orm::sqlx::Error::RowNotFound)?;
                #repository_post_image_check
                let after_state = Some(Self::__gom_capture_entity_state(&entity)?);

                hook_ctx
                    .run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Created,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: #pk_id_for_entity,
                            changes: ::graphql_orm::graphql::orm::mutation_changes(&[#(#create_mutation_field_literals),*], &bind_values),
                            before_state: None,
                            after_state,
                        },
                    )
                    .await
                    .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

                Self::__gom_queue_changed_event(
                    hook_ctx,
                    ::graphql_orm::graphql::orm::ChangeAction::Created,
                    Some(&entity),
                ).await?;

                Ok(entity)
            }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }

            #upsert_helper_methods
            #cas_helper_methods
            #single_bounded_helper_methods

            #[doc(hidden)]
            async fn __gom_update_by_id_with_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                id: &#pk_type_ty,
                mut input: #update_input,
            ) -> ::graphql_orm::Result<Option<Self>> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<Option<Self>> = async {
                hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                let current_entity = Self::__gom_fetch_by_id_on(hook_ctx, id).await?;
                let Some(current_entity) = current_entity else {
                    return Ok(None);
                };
                let db = hook_ctx.database().clone();
                db.ensure_writable_row(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                    &current_entity as &(dyn ::std::any::Any + Send + Sync),
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::internal(
                    #entity_name_lit,
                    hook_ctx,
                );
                db.run_before_update_with_context(
                    &mut write_ctx,
                    Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                    &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                <Self as ::graphql_orm::graphql::orm::Entity>::authorize_repository_update_fields(
                    &db,
                    Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                    &input as &(dyn ::std::any::Any + Send + Sync),
                ).await?;
                Self::__gom_authorize_repository_update_field_writes(
                    &db,
                    Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                    &input,
                ).await?;

                let mut set_clauses: Vec<String> = Vec::new();
                let mut changed_fields: Vec<&str> = Vec::new();
                let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();

                #update_assignments_repo_call

                if #has_updated_at_column {
                    set_clauses.push(format!("updated_at = {}", Self::__gom_current_updated_at_expr()));
                }
                if let Some(assignment) = Self::__gom_managed_version_assignment() {
                    set_clauses.push(assignment);
                }

                if set_clauses.is_empty() {
                    return Err(Self::__gom_runtime_error("No fields to update"));
                }

                Self::__gom_require_managed_version_headroom(&current_entity)?;
                let before_state = Self::__gom_capture_entity_state(&current_entity)?;
                let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #pk_id_for_current,
                        changes: mutation_changes.clone(),
                        before_state: Some(before_state.clone()),
                        after_state: None,
                    },
                )
                .await
                .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

                let sql = Self::__gom_rebind_sql(
                    &format!("UPDATE {} SET {} WHERE {} = ?{}", #sql_table_name, set_clauses.join(", "), #sql_dialect.render_identifier_path(Self::PRIMARY_KEY), Self::__gom_managed_version_guard()),
                    1,
                );
                values.push(#pk_bind_value_ref);

                let result = hook_ctx.execute(&sql, &values).await?;
                if <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&result) != 1 {
                    return Err(Self::__gom_runtime_error("authorized update did not affect the exact locked row"));
                }

                let entity = Self::__gom_fetch_by_id_on(hook_ctx, id).await?;
                let Some(entity) = entity else {
                    return Ok(None);
                };
                #repository_post_image_check
                let after_state = Some(Self::__gom_capture_entity_state(&entity)?);

                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #pk_id_for_entity,
                        changes: mutation_changes,
                        before_state: Some(before_state),
                        after_state,
                    },
                )
                .await
                .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

                Self::__gom_queue_changed_event(
                    hook_ctx,
                    ::graphql_orm::graphql::orm::ChangeAction::Updated,
                    Some(&entity),
                ).await?;

                Ok(Some(entity))
            }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }

            #[doc(hidden)]
            async fn __gom_update_where_with_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                where_input: #where_input,
                input: #update_input,
            ) -> ::graphql_orm::Result<i64> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<i64> = async {
                hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                where_input.__gom_require_scalar_write_filter().map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, EntityQuery, FromSqlRow, SqlValue};

                if where_input.is_empty() {
                    return Err(Self::__gom_runtime_error("Where filter is required for bulk update and must not be empty"));
                }

                let matched_entities = EntityQuery::<Self, #backend_marker>::new()
                    .filter_with_entity_matching(&where_input)
                    .fetch_all_for_write_in_transaction(hook_ctx)
                    .await?;

                if matched_entities.is_empty() {
                    return Ok(0);
                }
                let db = hook_ctx.database().clone();
                for entity in &matched_entities {
                    db.ensure_writable_row(
                        None,
                        #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                        entity as &(dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                }
                let original_input = input;
                let mut row_plans = Vec::with_capacity(matched_entities.len());
                for policy_entity in &matched_entities {
                    let mut input = original_input.clone();
                    let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::internal(
                        #entity_name_lit,
                        hook_ctx,
                    );
                    db.run_before_update_with_context(
                        &mut write_ctx,
                        Some(policy_entity as &(dyn ::std::any::Any + Send + Sync)),
                        &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    <Self as ::graphql_orm::graphql::orm::Entity>::authorize_repository_update_fields(
                        &db,
                        Some(policy_entity as &(dyn ::std::any::Any + Send + Sync)),
                        &input as &(dyn ::std::any::Any + Send + Sync),
                    ).await?;
                    Self::__gom_authorize_repository_update_field_writes(
                        &db,
                        Some(policy_entity as &(dyn ::std::any::Any + Send + Sync)),
                        &input,
                    ).await?;

                    let mut set_clauses: Vec<String> = Vec::new();
                    let mut changed_fields: Vec<&str> = Vec::new();
                    let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();

                    #update_assignments_repo_call

                    if #has_updated_at_column {
                        set_clauses.push(format!("updated_at = {}", Self::__gom_current_updated_at_expr()));
                    }
                    if let Some(assignment) = Self::__gom_managed_version_assignment() {
                        set_clauses.push(assignment);
                    }

                    if set_clauses.is_empty() {
                        return Err(Self::__gom_runtime_error("No fields to update"));
                    }

                    let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                    let sql = Self::__gom_rebind_sql(&format!(
                        "UPDATE {} SET {} WHERE {} = ?{}",
                        #sql_table_name,
                        set_clauses.join(", "),
                        #sql_dialect.render_identifier_path(Self::PRIMARY_KEY),
                        Self::__gom_managed_version_guard(),
                    ), 1);
                    row_plans.push((sql, values, mutation_changes));
                }
                for entity in &matched_entities {
                    Self::__gom_require_managed_version_headroom(entity)?;
                }
                for (entity, (_, _, mutation_changes)) in matched_entities.iter().zip(&row_plans) {
                    hook_ctx.run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: #pk_id_for_entity,
                            changes: mutation_changes.clone(),
                            before_state: Some(Self::__gom_capture_entity_state(entity)?),
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                }

                let mut affected = 0_i64;
                for (entity, (sql, values, _)) in matched_entities.iter().zip(&row_plans) {
                    let mut row_values = values.clone();
                    row_values.push(#pk_bind_value_for_entity);
                    let result = hook_ctx.execute(sql, &row_values).await?;
                    if <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&result) != 1 {
                        return Err(Self::__gom_runtime_error("authorized bulk update did not affect an exact locked row"));
                    }
                    affected = affected.checked_add(1)
                        .ok_or_else(|| Self::__gom_runtime_error("bulk update affected-row count overflow"))?;
                }

                for (previous, (_, _, mutation_changes)) in matched_entities.into_iter().zip(row_plans) {
                    let entity = Self::__gom_fetch_by_id_on(hook_ctx, &previous.#pk_field).await?
                        .ok_or_else(|| Self::__gom_runtime_error("authorized bulk update row disappeared before after-hook"))?;
                    #repository_post_image_check
                        hook_ctx.run_mutation_hook(
                            None,
                            &::graphql_orm::graphql::orm::MutationEvent {
                                phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                                entity_name: #entity_name_lit,
                                table_name: #table_name,
                                metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                                id: #pk_id_for_entity,
                                changes: mutation_changes.clone(),
                                before_state: Some(Self::__gom_capture_entity_state(&previous)?),
                                after_state: Some(Self::__gom_capture_entity_state(&entity)?),
                            },
                        )
                        .await
                        .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                        Self::__gom_queue_changed_event(
                            hook_ctx,
                            ::graphql_orm::graphql::orm::ChangeAction::Updated,
                            Some(&entity),
                        ).await?;
                }

                Ok(affected)
            }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }

            #[doc(hidden)]
            async fn __gom_delete_by_id_with_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                id: &#pk_type_ty,
            ) -> ::graphql_orm::Result<bool> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<bool> = async {
                hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, SqlValue};

                let entity = Self::__gom_fetch_by_id_on(hook_ctx, id).await?;
                let Some(entity) = entity else {
                    return Ok(false);
                };
                let db = hook_ctx.database().clone();
                db.ensure_writable_row(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                    &entity as &(dyn ::std::any::Any + Send + Sync),
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                let before_state = Self::__gom_capture_entity_state(&entity)?;

                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #pk_id_for_entity,
                        changes: Vec::new(),
                        before_state: Some(before_state.clone()),
                        after_state: None,
                    },
                )
                .await
                .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

                let sql = Self::__gom_rebind_sql(
                    &format!("DELETE FROM {} WHERE {} = ?", #sql_table_name, #sql_dialect.render_identifier_path(Self::PRIMARY_KEY)),
                    1,
                );
                let values = [#pk_bind_value_ref];
                let result = hook_ctx.execute(&sql, &values).await?;
                if <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&result) != 1 {
                    return Err(Self::__gom_runtime_error("authorized delete did not affect the exact locked row"));
                }

                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #pk_id_for_entity,
                        changes: Vec::new(),
                        before_state: Some(before_state),
                        after_state: None,
                    },
                )
                .await
                .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

                Self::__gom_queue_changed_event(
                    hook_ctx,
                    ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                    Some(&entity),
                ).await?;

                Ok(true)
            }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }

            #[doc(hidden)]
            async fn __gom_delete_where_with_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                where_input: #where_input,
            ) -> ::graphql_orm::Result<i64> {
                    let __gom_write_tracker = hook_ctx.__track_writes();
                    let __gom_result: ::graphql_orm::Result<i64> = async {
                hook_ctx.database().ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                where_input.__gom_require_scalar_write_filter().map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, EntityQuery, FromSqlRow};

                if where_input.is_empty() {
                    return Err(Self::__gom_runtime_error("Where filter is required for bulk delete and must not be empty"));
                }

                let matched_entities = EntityQuery::<Self, #backend_marker>::new()
                    .filter_with_entity_matching(&where_input)
                    .fetch_all_for_write_in_transaction(hook_ctx)
                    .await?;

                if matched_entities.is_empty() {
                    return Ok(0);
                }

                let db = hook_ctx.database().clone();
                for entity in &matched_entities {
                    db.ensure_writable_row(
                        None,
                        #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                        entity as &(dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                }

                for entity in &matched_entities {
                    hook_ctx.run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: #pk_id_for_entity,
                            changes: Vec::new(),
                            before_state: Some(Self::__gom_capture_entity_state(entity)?),
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                }

                let sql = Self::__gom_rebind_sql(
                    &format!("DELETE FROM {} WHERE {} = ?", #sql_table_name, #sql_dialect.render_identifier_path(Self::PRIMARY_KEY)),
                    1,
                );
                let mut deleted = 0_i64;
                for entity in &matched_entities {
                    let values = [#pk_bind_value_for_entity];
                    let result = hook_ctx.execute(&sql, &values).await?;
                    if <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&result) != 1 {
                        return Err(Self::__gom_runtime_error("authorized bulk delete did not affect an exact locked row"));
                    }
                    deleted = deleted.checked_add(1)
                        .ok_or_else(|| Self::__gom_runtime_error("bulk delete affected-row count overflow"))?;
                }

                for entity in matched_entities {
                    hook_ctx.run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: #pk_id_for_entity,
                            changes: Vec::new(),
                            before_state: Some(Self::__gom_capture_entity_state(&entity)?),
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    Self::__gom_queue_changed_event(
                        hook_ctx,
                        ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        Some(&entity),
                    ).await?;
                }

                Ok(deleted)
            }.await;
                    __gom_write_tracker.finish(&__gom_result);
                    __gom_result
                }

            /// Insert a new entity record using the generated create input.
            pub async fn insert<P>(
                provider: &P,
                input: #create_input,
            ) -> ::graphql_orm::Result<Self>
            where
                P: ::graphql_orm::graphql::orm::PoolProvider<#backend_marker> + ::std::any::Any,
            {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                // Every provider takes the same authorized path. A provider that is
                // not a `Database` carries no policies, hooks or transforms, so it
                // is the default handle over its pool: an entity or field that
                // declares a policy key is refused as misconfigured, and the
                // schema policy is the one the pool stands for.
                let __gom_pool_database;
                let db = match (provider as &dyn ::std::any::Any).downcast_ref::<::graphql_orm::db::Database<#backend_marker>>() {
                    Some(db) => db,
                    None => {
                        // Boxed: this future is embedded in every caller's.
                        __gom_pool_database = ::std::boxed::Box::new(::graphql_orm::db::Database::<#backend_marker>::__gom_default_for_pool(provider.pool().clone()));
                        &*__gom_pool_database
                    }
                };
                db.ensure_entity_access(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::Default,
                ).await?;
                let entity = Self::__gom_insert_with_mutation_context(&mut hook_ctx, input).await?;
                hook_ctx.commit_and_emit().await?;
                Ok(entity)
            }

            /// Insert multiple entity records transactionally using the generated create input.
            pub async fn insert_many<I>(
                db: &::graphql_orm::db::Database<#backend_marker>,
                inputs: I,
            ) -> ::graphql_orm::Result<Vec<Self>>
            where
                I: IntoIterator<Item = #create_input>,
            {
                db.ensure_entity_access(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

                let inputs: Vec<#create_input> = inputs.into_iter().collect();
                if inputs.is_empty() {
                    return Ok(Vec::new());
                }

                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::Default,
                ).await?;
                let mut entities = Vec::with_capacity(inputs.len());
                for input in inputs {
                    entities.push(Self::__gom_insert_with_mutation_context(&mut hook_ctx, input).await?);
                }
                hook_ctx.commit_and_emit().await?;
                Ok(entities)
            }

            #upsert_repository_method
            #single_bounded_repository_methods

            /// Find all entities matching the given filter
            pub fn query<'a>(pool: &'a #pool_type) -> ::graphql_orm::graphql::orm::FindQuery<'a, Self, #where_input, #order_by_input, #backend_marker> {
                ::graphql_orm::graphql::orm::FindQuery::new(pool)
            }

            /// Fetch all entities through a `Database` handle without exposing the raw pool type.
            pub async fn find_all(
                db: &::graphql_orm::db::Database<#backend_marker>,
            ) -> ::graphql_orm::Result<Vec<Self>> {
                ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                    .fetch_all(db)
                    .await
            }

            /// Fetch entities matching a typed where filter through a `Database` handle.
            pub async fn find_many(
                db: &::graphql_orm::db::Database<#backend_marker>,
                where_input: #where_input,
            ) -> ::graphql_orm::Result<Vec<Self>> {
                ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                    .filter_with_entity_matching(&where_input)
                    .fetch_all(db)
                    .await
            }

            #search_repository_method
            #keyset_repository_methods
            #search_rebuild_methods

            #read_repository_key_methods

            /// Update a single entity by primary key using the generated update input.
            #[allow(clippy::ptr_arg)]
            pub async fn update_by_id(
                db: &::graphql_orm::db::Database<#backend_marker>,
                id: &#pk_type_ty,
                mut input: #update_input,
            ) -> ::graphql_orm::Result<Option<Self>> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow, SqlValue};

                db.ensure_entity_access(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                ).await?;
                let current_entity = EntityQuery::<Self, #backend_marker>::new()
                    .where_clause(
                        &format!("{} = {}", #sql_dialect.render_identifier_path(Self::PRIMARY_KEY), Self::__gom_placeholder(1)),
                        #pk_bind_value_ref
                    )
                    .fetch_one_for_write_in_transaction(&mut hook_ctx)
                    .await?;

                let Some(current_entity) = current_entity else {
                    return Ok(None);
                };
                db.ensure_writable_row(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                    &current_entity as &(dyn ::std::any::Any + Send + Sync),
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::repository_in_transaction(
                    #entity_name_lit,
                    &mut hook_ctx,
                );
                db.run_before_update_with_context(
                    &mut write_ctx,
                    Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                    &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                <Self as ::graphql_orm::graphql::orm::Entity>::authorize_repository_update_fields(
                    db,
                    Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                    &input as &(dyn ::std::any::Any + Send + Sync),
                ).await?;
                Self::__gom_authorize_repository_update_field_writes(
                    db,
                    Some(&current_entity as &(dyn ::std::any::Any + Send + Sync)),
                    &input,
                ).await?;

                let mut set_clauses: Vec<String> = Vec::new();
                let mut changed_fields: Vec<&str> = Vec::new();
                let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();

                #update_assignments_repo_call

                if #has_updated_at_column {
                    set_clauses.push(format!("updated_at = {}", Self::__gom_current_updated_at_expr()));
                }
                if let Some(assignment) = Self::__gom_managed_version_assignment() {
                    set_clauses.push(assignment);
                }

                if set_clauses.is_empty() {
                    return Err(Self::__gom_runtime_error("No fields to update"));
                }

                Self::__gom_require_managed_version_headroom(&current_entity)?;
                let before_state = Self::__gom_capture_entity_state(&current_entity)?;
                let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #pk_id_for_current,
                        changes: mutation_changes.clone(),
                        before_state: Some(before_state.clone()),
                        after_state: None,
                    },
                )
                .await
                .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

                let sql = Self::__gom_rebind_sql(
                    &format!("UPDATE {} SET {} WHERE {} = ?{}", #sql_table_name, set_clauses.join(", "), #sql_dialect.render_identifier_path(Self::PRIMARY_KEY), Self::__gom_managed_version_guard()),
                    1,
                );
                values.push(#pk_bind_value_ref);

                let result = hook_ctx.execute(&sql, &values).await?;
                if <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&result) != 1 {
                    return Err(Self::__gom_runtime_error("authorized update did not affect the exact locked row"));
                }

                let entity = Self::__gom_fetch_by_id_on(&mut hook_ctx, id).await?;

                let Some(entity) = entity else {
                    return Ok(None);
                };
                #repository_post_image_check
                let after_state = Some(Self::__gom_capture_entity_state(&entity)?);

                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #pk_id_for_entity,
                        changes: mutation_changes,
                        before_state: Some(before_state),
                        after_state,
                    },
                )
                .await
                .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                Self::__gom_queue_changed_event(
                    &mut hook_ctx,
                    ::graphql_orm::graphql::orm::ChangeAction::Updated,
                    Some(&entity),
                ).await?;
                hook_ctx.commit_and_emit().await?;

                Ok(Some(entity))
            }

            /// Update multiple entities matching a typed where filter.
            pub async fn update_where(
                db: &::graphql_orm::db::Database<#backend_marker>,
                where_input: #where_input,
                input: #update_input,
            ) -> ::graphql_orm::Result<i64> {
                Self::__gom_update_where_with_auth(db, where_input, input, None, None, None).await
            }

            #[doc(hidden)]
            pub(crate) async fn __gom_update_where_with_auth(
                db: &::graphql_orm::db::Database<#backend_marker>,
                where_input: #where_input,
                input: #update_input,
                auth_context: Option<&::graphql_orm::graphql::orm::DbAuthContext>,
                graphql_ctx: Option<&::graphql_orm::async_graphql::Context<'_>>,
                notification_rows: Option<&mut Vec<Self>>,
            ) -> ::graphql_orm::Result<i64> {
                where_input.__gom_require_scalar_write_filter().map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, EntityQuery, FromSqlRow, SqlValue};

                if where_input.is_empty() {
                    return Err(Self::__gom_runtime_error("Where filter is required for bulk update and must not be empty"));
                }

                let access_surface = if graphql_ctx.is_some() {
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation
                } else {
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository
                };
                db.ensure_entity_access(
                    graphql_ctx,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    access_surface,
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin_with_auth(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                    auth_context,
                )
                .await?;
                // The runtime selects, locks and classifies the targets. A row a
                // GraphQL caller can neither read nor write is left out before any
                // count or refusal can depend on it, and the bulk write bound counts
                // targets only; a repository write keeps the unbounded read.
                let matched_entities = ::std::boxed::Box::pin(::graphql_orm::graphql::orm::bulk_write_targets::<Self, #backend_marker>(
                    db,
                    graphql_ctx,
                    &mut hook_ctx,
                    EntityQuery::<Self, #backend_marker>::new()
                        .filter_with_entity_matching(&where_input),
                ))
                .await?;

                if #has_notify_handler && graphql_ctx.is_some() && matched_entities.len() > 1000 {
                    return Err(::graphql_orm::graphql::errors::sqlx_error_from_public(
                        ::graphql_orm::graphql::errors::OrmPublicError::with_message(
                            ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                            "notify_with bulk writes are limited to 1000 rows",
                        ),
                    ));
                }
                if matched_entities.is_empty() {
                    return Ok(0);
                }
                let original_input = input;
                #[allow(unused_mut)]
                let mut __gom_client_supplied: ::std::collections::HashSet<&'static str> =
                    ::std::collections::HashSet::new();
                {
                    let __gom_client_input = &original_input;
                    #(#update_client_supplied)*
                }
                let mut row_inputs = Vec::with_capacity(matched_entities.len());
                #[allow(unused_mut)]
                let mut __gom_omitted_fields: ::std::collections::HashSet<&'static str> = ::std::collections::HashSet::new();
                for policy_entity in &matched_entities {
                    let mut input = original_input.clone();
                    if let Some(ctx) = graphql_ctx {
                        let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::graphql_in_transaction(
                            ctx,
                            #entity_name_lit,
                            &mut hook_ctx,
                        );
                        db.run_before_update_with_context(
                            &mut write_ctx,
                            Some(policy_entity as &(dyn ::std::any::Any + Send + Sync)),
                            &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                        ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                        #(#update_runtime_policy_checks)*
                    } else {
                        let mut write_ctx = ::graphql_orm::graphql::orm::WriteInputContext::repository_in_transaction(
                            #entity_name_lit,
                            &mut hook_ctx,
                        );
                        db.run_before_update_with_context(
                            &mut write_ctx,
                            Some(policy_entity as &(dyn ::std::any::Any + Send + Sync)),
                            &mut input as &mut (dyn ::std::any::Any + Send + Sync),
                        ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                        <Self as ::graphql_orm::graphql::orm::Entity>::authorize_repository_update_fields(
                            db,
                            Some(policy_entity as &(dyn ::std::any::Any + Send + Sync)),
                            &input as &(dyn ::std::any::Any + Send + Sync),
                        ).await?;
                        Self::__gom_authorize_repository_update_field_writes(
                            db,
                            Some(policy_entity as &(dyn ::std::any::Any + Send + Sync)),
                            &input,
                        ).await?;
                    }

                    row_inputs.push(input);
                }
                let mut row_plans = Vec::with_capacity(matched_entities.len());
                for input in row_inputs {
                    #[allow(unused_mut)]
                    let mut input = input;
                    // Preserve legacy operation-wide omission without repeating callbacks.
                    #(#update_bulk_omission_checks)*

                    let mut set_clauses: Vec<String> = Vec::new();
                    let mut changed_fields: Vec<&str> = Vec::new();
                    let mut values: Vec<::graphql_orm::graphql::orm::SqlValue> = Vec::new();

                    #update_assignments_repo_call

                    if #has_updated_at_column {
                        set_clauses.push(format!("updated_at = {}", Self::__gom_current_updated_at_expr()));
                    }
                    if let Some(assignment) = Self::__gom_managed_version_assignment() {
                        set_clauses.push(assignment);
                    }

                    if set_clauses.is_empty() {
                        return Err(Self::__gom_runtime_error("No fields to update"));
                    }

                    let mutation_changes = ::graphql_orm::graphql::orm::mutation_changes(&changed_fields, &values);
                    let sql = Self::__gom_rebind_sql(&format!(
                        "UPDATE {} SET {} WHERE {} = ?{}",
                        #sql_table_name,
                        set_clauses.join(", "),
                        #sql_dialect.render_identifier_path(Self::PRIMARY_KEY),
                        Self::__gom_managed_version_guard(),
                    ), 1);
                    row_plans.push((sql, values, mutation_changes));
                }
                for entity in &matched_entities {
                    Self::__gom_require_managed_version_headroom(entity)?;
                }
                for (entity, (_, _, mutation_changes)) in matched_entities.iter().zip(&row_plans) {
                    hook_ctx.run_mutation_hook(
                        graphql_ctx,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: #pk_id_for_entity,
                            changes: mutation_changes.clone(),
                            before_state: Some(Self::__gom_capture_entity_state(entity)?),
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                }

                let mut affected = 0_i64;
                for (entity, (sql, values, _)) in matched_entities.iter().zip(&row_plans) {
                    let mut row_values = values.clone();
                    row_values.push(#pk_bind_value_for_entity);
                    let result = hook_ctx.execute(sql, &row_values).await?;
                    if <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&result) != 1 {
                        return Err(Self::__gom_runtime_error("authorized bulk update did not affect an exact locked row"));
                    }
                    affected = affected.checked_add(1)
                        .ok_or_else(|| Self::__gom_runtime_error("bulk update affected-row count overflow"))?;
                }

                let mut committed_rows = Vec::new();
                for (previous, (_, _, mutation_changes)) in matched_entities.into_iter().zip(row_plans) {
                    let entity = Self::__gom_fetch_by_id_on(&mut hook_ctx, &previous.#pk_field).await?
                        .ok_or_else(|| Self::__gom_runtime_error("authorized bulk update row disappeared before after-hook"))?;
                    hook_ctx.__ensure_written_row(
                        graphql_ctx,
                        #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        access_surface,
                        &entity as &(dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                        hook_ctx.run_mutation_hook(
                            graphql_ctx,
                            &::graphql_orm::graphql::orm::MutationEvent {
                                phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                                action: ::graphql_orm::graphql::orm::ChangeAction::Updated,
                                entity_name: #entity_name_lit,
                                table_name: #table_name,
                                metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                                id: #pk_id_for_entity,
                                changes: mutation_changes.clone(),
                                before_state: Some(Self::__gom_capture_entity_state(&previous)?),
                                after_state: Some(Self::__gom_capture_entity_state(&entity)?),
                            },
                        )
                        .await
                        .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                        Self::__gom_queue_changed_event(
                            &mut hook_ctx,
                            ::graphql_orm::graphql::orm::ChangeAction::Updated,
                            Some(&entity),
                        ).await?;
                        if notification_rows.is_some() { committed_rows.push(entity); }
                }
                hook_ctx.commit_and_emit().await?;

                if let Some(rows) = notification_rows { rows.extend(committed_rows); }
                Ok(affected)
            }

            /// Delete a single entity by primary key.
            #[allow(clippy::ptr_arg)]
            pub async fn delete_by_id(
                db: &::graphql_orm::db::Database<#backend_marker>,
                id: &#pk_type_ty,
            ) -> ::graphql_orm::Result<bool> {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, SqlValue};

                db.ensure_entity_access(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                ).await?;
                let entity = EntityQuery::<Self, #backend_marker>::new()
                    .where_clause(
                        &format!("{} = {}", #sql_dialect.render_identifier_path(Self::PRIMARY_KEY), Self::__gom_placeholder(1)),
                        #pk_bind_value_ref
                    )
                    .fetch_one_for_write_in_transaction(&mut hook_ctx)
                    .await?;

                let Some(entity) = entity else {
                    return Ok(false);
                };
                db.ensure_writable_row(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                    &entity as &(dyn ::std::any::Any + Send + Sync),
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                let before_state = Self::__gom_capture_entity_state(&entity)?;
                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #pk_id_for_entity,
                        changes: Vec::new(),
                        before_state: Some(before_state.clone()),
                        after_state: None,
                    },
                )
                .await
                .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

                let sql = Self::__gom_rebind_sql(
                    &format!("DELETE FROM {} WHERE {} = ?", #sql_table_name, #sql_dialect.render_identifier_path(Self::PRIMARY_KEY)),
                    1,
                );
                let values = [#pk_bind_value_ref];
                let result = hook_ctx.execute(&sql, &values).await?;
                if <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&result) != 1 {
                    return Err(Self::__gom_runtime_error("authorized delete did not affect the exact locked row"));
                }

                hook_ctx.run_mutation_hook(
                    None,
                    &::graphql_orm::graphql::orm::MutationEvent {
                        phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                        action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        entity_name: #entity_name_lit,
                        table_name: #table_name,
                        metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                        id: #pk_id_for_entity,
                        changes: Vec::new(),
                        before_state: Some(before_state),
                        after_state: None,
                    },
                )
                .await
                .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                Self::__gom_queue_changed_event(
                    &mut hook_ctx,
                    ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                    Some(&entity),
                ).await?;
                hook_ctx.commit_and_emit().await?;

                Ok(true)
            }

            /// Delete multiple entities matching a typed where filter.
            pub async fn delete_where(
                db: &::graphql_orm::db::Database<#backend_marker>,
                where_input: #where_input,
            ) -> ::graphql_orm::Result<i64> {
                Self::__gom_delete_where_with_auth(db, where_input, None, None, None).await
            }

            /// Delete every row for this entity.
            ///
            /// This is intentionally separate from `delete_where`; empty
            /// `delete_where` filters continue to fail so accidental table-wide
            /// deletes remain visible in code review.
            pub async fn delete_all(
                db: &::graphql_orm::db::Database<#backend_marker>,
            ) -> ::graphql_orm::Result<i64> {
                Self::__gom_delete_matching_with_auth(db, #where_input::default(), None, None, true, None).await
            }

            /// Replace every row for this entity in one transaction.
            ///
            /// Existing rows are deleted, then the provided create inputs are
            /// inserted using the normal generated ORM write path. Mutation
            /// hooks, policies, search maintenance, and change events are
            /// preserved.
            pub async fn replace_all<I>(
                db: &::graphql_orm::db::Database<#backend_marker>,
                inputs: I,
            ) -> ::graphql_orm::Result<Vec<Self>>
            where
                I: IntoIterator<Item = #create_input>,
            {
                use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, FromSqlRow};

                db.ensure_entity_access(
                    None,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;

                let inputs: Vec<#create_input> = inputs.into_iter().collect();
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                ).await?;
                let existing_entities = EntityQuery::<Self, #backend_marker>::new()
                    .fetch_all_for_write_in_transaction(&mut hook_ctx)
                    .await?;

                for entity in &existing_entities {
                    db.ensure_writable_row(
                        None,
                        #entity_name_lit,
                        <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::Repository,
                        entity as &(dyn ::std::any::Any + Send + Sync),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                }

                for entity in &existing_entities {
                    hook_ctx.run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: #pk_id_for_entity,
                            changes: Vec::new(),
                            before_state: Some(Self::__gom_capture_entity_state(entity)?),
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                }

                if !existing_entities.is_empty() {
                    let sql = Self::__gom_rebind_sql(
                        &format!("DELETE FROM {} WHERE {} = ?", #sql_table_name, #sql_dialect.render_identifier_path(Self::PRIMARY_KEY)),
                        1,
                    );
                    for entity in &existing_entities {
                        let values = [#pk_bind_value_for_entity];
                        let result = hook_ctx.execute(&sql, &values).await?;
                        if <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&result) != 1 {
                            return Err(Self::__gom_runtime_error("replace_all did not delete an exact locked row"));
                        }
                    }
                }

                for entity in &existing_entities {
                    hook_ctx.run_mutation_hook(
                        None,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: #pk_id_for_entity,
                            changes: Vec::new(),
                            before_state: Some(Self::__gom_capture_entity_state(entity)?),
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    Self::__gom_queue_changed_event(
                        &mut hook_ctx,
                        ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        Some(entity),
                    ).await?;
                }

                let mut inserted = Vec::with_capacity(inputs.len());
                for input in inputs {
                    inserted.push(Self::__gom_insert_with_mutation_context(&mut hook_ctx, input).await?);
                }

                hook_ctx.commit_and_emit().await?;
                Ok(inserted)
            }

            #[doc(hidden)]
            pub(crate) async fn __gom_delete_where_with_auth(
                db: &::graphql_orm::db::Database<#backend_marker>,
                where_input: #where_input,
                auth_context: Option<&::graphql_orm::graphql::orm::DbAuthContext>,
                graphql_ctx: Option<&::graphql_orm::async_graphql::Context<'_>>,
                notification_rows: Option<&mut Vec<Self>>,
            ) -> ::graphql_orm::Result<i64> {
                Self::__gom_delete_matching_with_auth(db, where_input, auth_context, graphql_ctx, false, notification_rows).await
            }

            #[doc(hidden)]
            pub(crate) async fn __gom_delete_matching_with_auth(
                db: &::graphql_orm::db::Database<#backend_marker>,
                where_input: #where_input,
                auth_context: Option<&::graphql_orm::graphql::orm::DbAuthContext>,
                graphql_ctx: Option<&::graphql_orm::async_graphql::Context<'_>>,
                allow_empty_filter: bool,
                notification_rows: Option<&mut Vec<Self>>,
            ) -> ::graphql_orm::Result<i64> {
                where_input.__gom_require_scalar_write_filter().map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, EntityQuery, FromSqlRow};

                let filter_is_empty = where_input.is_empty();
                if filter_is_empty && !allow_empty_filter {
                    return Err(Self::__gom_runtime_error("Where filter is required for bulk delete and must not be empty"));
                }

                let access_surface = if graphql_ctx.is_some() {
                    ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation
                } else {
                    ::graphql_orm::graphql::orm::EntityAccessSurface::Repository
                };
                db.ensure_entity_access(
                    graphql_ctx,
                    #entity_name_lit,
                    <Self as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy,
                    ::graphql_orm::graphql::orm::EntityAccessKind::Write,
                    access_surface,
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                db.ensure_row_write_provider(#entity_name_lit, <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().write_policy).map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                let mut hook_ctx = ::graphql_orm::graphql::orm::MutationContext::<#backend_marker>::begin_with_auth(
                    db,
                    ::graphql_orm::graphql::orm::TransactionMode::StateMachine,
                    auth_context,
                )
                .await?;
                // The runtime selects, locks and classifies the targets. A row a
                // GraphQL caller can neither read nor write is left out before any
                // count or refusal can depend on it, and the bulk write bound counts
                // targets only; a repository write keeps the unbounded read.
                let matched_entities = ::std::boxed::Box::pin(::graphql_orm::graphql::orm::bulk_write_targets::<Self, #backend_marker>(
                    db,
                    graphql_ctx,
                    &mut hook_ctx,
                    EntityQuery::<Self, #backend_marker>::new()
                        .filter_with_entity_matching(&where_input),
                ))
                .await?;

                if #has_notify_handler && graphql_ctx.is_some() && matched_entities.len() > 1000 {
                    return Err(::graphql_orm::graphql::errors::sqlx_error_from_public(
                        ::graphql_orm::graphql::errors::OrmPublicError::with_message(
                            ::graphql_orm::graphql::errors::OrmErrorCode::InvalidInput,
                            "notify_with bulk writes are limited to 1000 rows",
                        ),
                    ));
                }
                if matched_entities.is_empty() {
                    return Ok(0);
                }

                for entity in &matched_entities {
                    hook_ctx.run_mutation_hook(
                        graphql_ctx,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::Before,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: #pk_id_for_entity,
                            changes: Vec::new(),
                            before_state: Some(Self::__gom_capture_entity_state(entity)?),
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                }

                let sql = Self::__gom_rebind_sql(
                    &format!("DELETE FROM {} WHERE {} = ?", #sql_table_name, #sql_dialect.render_identifier_path(Self::PRIMARY_KEY)),
                    1,
                );
                let mut deleted = 0_i64;
                for entity in &matched_entities {
                    let values = [#pk_bind_value_for_entity];
                    let result = hook_ctx.execute(&sql, &values).await?;
                    if <#backend_marker as ::graphql_orm::graphql::orm::WriteBackend>::rows_affected(&result) != 1 {
                        return Err(Self::__gom_runtime_error("authorized bulk delete did not affect an exact locked row"));
                    }
                    deleted = deleted.checked_add(1)
                        .ok_or_else(|| Self::__gom_runtime_error("bulk delete affected-row count overflow"))?;
                }

                for entity in &matched_entities {
                    hook_ctx.run_mutation_hook(
                        graphql_ctx,
                        &::graphql_orm::graphql::orm::MutationEvent {
                            phase: ::graphql_orm::graphql::orm::MutationPhase::After,
                            action: ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                            entity_name: #entity_name_lit,
                            table_name: #table_name,
                            metadata: <Self as ::graphql_orm::graphql::orm::Entity>::metadata(),
                            id: #pk_id_for_entity,
                            changes: Vec::new(),
                            before_state: Some(Self::__gom_capture_entity_state(entity)?),
                            after_state: None,
                        },
                    )
                    .await
                    .map_err(::graphql_orm::graphql::errors::sqlx_error_from_graphql)?;
                    Self::__gom_queue_changed_event(
                        &mut hook_ctx,
                        ::graphql_orm::graphql::orm::ChangeAction::Deleted,
                        Some(entity),
                    ).await?;
                }
                hook_ctx.commit_and_emit().await?;

                if let Some(rows) = notification_rows { rows.extend(matched_entities); }
                Ok(deleted)
            }

            /// Count entities matching the given filter
            pub fn count_query<'a>(pool: &'a #pool_type) -> ::graphql_orm::graphql::orm::EntityCountQuery<'a, Self, #where_input, #backend_marker> {
                ::graphql_orm::graphql::orm::EntityCountQuery::new(pool)
            }

            /// Count all entities through a `Database` handle without exposing the raw pool type.
            pub async fn count_all(
                db: &::graphql_orm::db::Database<#backend_marker>,
            ) -> ::graphql_orm::Result<i64> {
                ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                    .count(db)
                    .await
            }

            /// Count entities matching a typed where filter through a `Database` handle.
            pub async fn count(
                db: &::graphql_orm::db::Database<#backend_marker>,
                where_input: #where_input,
            ) -> ::graphql_orm::Result<i64> {
                ::graphql_orm::graphql::orm::EntityQuery::<Self, #backend_marker>::new()
                    .filter_with_entity_matching(&where_input)
                    .count(db)
                    .await
            }

            /// Search entities with fuzzy/similar text matching
            ///
            /// # Arguments
            /// * `pool` - Database connection pool
            /// * `field` - Name of the field to search (snake_case)
            /// * `query` - The search query text
            /// * `threshold` - Minimum similarity score (0.0-1.0, recommended: 0.5-0.7)
            /// * `filter` - Optional additional filter to apply
            /// * `limit` - Maximum number of results to return, at most
            ///   `graphql_orm::graphql::orm::SIMILAR_SEARCH_MAX_LIMIT` (1,000). The
            ///   helper scores up to five candidates per requested result. A
            ///   larger limit fails with `PAGE_LIMIT_EXCEEDED` and a negative one
            ///   with `INVALID_INPUT`; without a limit every matching row is scored.
            ///
            /// # Returns
            /// Vector of (entity, score) tuples, sorted by score descending
            ///
            /// # Example
            /// ```rust,ignore
            /// let matches = Entity::search_similar(
            ///     &pool,
            ///     "name",
            ///     "example",
            ///     0.6,
            ///     Some(EntityWhereInput {
            ///         name: Some(StringFilter::contains("ex")),
            ///         ..Default::default()
            ///     }),
            ///     Some(25),
            /// ).await?;
            ///
            /// for (entity, score) in matches {
            ///     println!("{}: {:.2}", entity.name, score);
            /// }
            /// ```
            pub async fn search_similar(
                pool: &#pool_type,
                field: &str,
                query: &str,
                threshold: f64,
                filter: Option<#where_input>,
                limit: Option<i64>,
            ) -> ::graphql_orm::Result<Vec<(Self, f64)>> {
                use ::graphql_orm::graphql::orm::FuzzyMatcher;

                // Fetch candidates (optionally filtered)
                let mut q = Self::query(pool);
                if let Some(f) = filter {
                    q = q.filter(f);
                }
                // Fetch more than limit to account for fuzzy filtering. The
                // candidate bound is this read's own maximum, so it is honoured
                // exactly rather than reduced to the page maximum.
                if let Some(l) = limit {
                    let candidates = ::graphql_orm::graphql::orm::similar_search_candidate_limit(l)?;
                    q = q.with_max_limit(candidates).limit(candidates);
                }
                let candidates = q.fetch_all().await?;

                // Score with fuzzy matcher
                let matcher = FuzzyMatcher::new(query).with_threshold(threshold);
                let mut results = matcher.filter_and_score(candidates, |entity| {
                    Self::get_searchable_field(entity, field)
                });

                // Apply limit
                if let Some(l) = limit {
                    results.truncate(l as usize);
                }

                Ok(results.into_iter().map(|m| (m.entity, m.score)).collect())
            }

            /// Get a searchable field value by name (for fuzzy matching)
            #[doc(hidden)]
            fn get_searchable_field<'a>(entity: &'a Self, field: &str) -> Option<&'a str> {
                #searchable_field_match
            }
        }

        impl ::graphql_orm::graphql::orm::MutationContextInsert<#backend_marker> for #struct_name {
            type CreateInput = #create_input;

            fn insert_in_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                input: Self::CreateInput,
            ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<Self>> {
                Box::pin(async move { Self::__gom_insert_with_mutation_context(hook_ctx, input).await })
            }
        }

        #upsert_trait_impl
        #single_bounded_trait_impls
        #retention_surface
        #cas_trait_impl
        #keyset_trait_impl

        impl #struct_name {
            #managed_version_helper_methods
        }

        impl ::graphql_orm::graphql::orm::MutationContextUpdateById<#backend_marker> for #struct_name {
            type Id = #pk_type_ty;
            type UpdateInput = #update_input;

            fn update_by_id_in_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                id: &'a Self::Id,
                input: Self::UpdateInput,
            ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<Option<Self>>> {
                Box::pin(async move { Self::__gom_update_by_id_with_mutation_context(hook_ctx, id, input).await })
            }
        }

        impl ::graphql_orm::graphql::orm::MutationContextUpdateWhere<#backend_marker> for #struct_name {
            type WhereInput = #where_input;
            type UpdateInput = #update_input;

            fn update_where_in_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                where_input: Self::WhereInput,
                input: Self::UpdateInput,
            ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<i64>> {
                Box::pin(async move { Self::__gom_update_where_with_mutation_context(hook_ctx, where_input, input).await })
            }
        }

        impl ::graphql_orm::graphql::orm::MutationContextDeleteById<#backend_marker> for #struct_name {
            type Id = #pk_type_ty;

            fn delete_by_id_in_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                id: &'a Self::Id,
            ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<bool>> {
                Box::pin(async move { Self::__gom_delete_by_id_with_mutation_context(hook_ctx, id).await })
            }
        }

        impl ::graphql_orm::graphql::orm::MutationContextDeleteWhere<#backend_marker> for #struct_name {
            type WhereInput = #where_input;

            fn delete_where_in_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                where_input: Self::WhereInput,
            ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<i64>> {
                Box::pin(async move { Self::__gom_delete_where_with_mutation_context(hook_ctx, where_input).await })
            }
        }

        impl ::graphql_orm::graphql::orm::MutationContextFindById<#backend_marker> for #struct_name {
            type Id = #pk_type_ty;

            fn find_by_id_in_mutation_context<'a>(
                hook_ctx: &'a mut ::graphql_orm::graphql::orm::MutationContext<'_, #backend_marker>,
                id: &'a Self::Id,
            ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<Option<Self>>> {
                Box::pin(async move { Self::__gom_fetch_by_id_on(hook_ctx, id).await })
            }
        }
    })
}

#[cfg(test)]
mod composite_keyset_tests {
    use super::generate_graphql_operations;

    fn entity(order: &str) -> syn::DeriveInput {
        let order = syn::LitStr::new(order, proc_macro2::Span::call_site());
        syn::parse_quote! {
            #[graphql_entity(table = "composite_keyset_fixture", keyset = #order)]
            struct CompositeKeysetFixture {
                #[primary_key]
                #[graphql_orm(auto_generated = false)]
                partition_key: String,
                #[primary_key]
                #[graphql_orm(auto_generated = false)]
                item_no: i64,
                rank: i64,
            }
        }
    }

    #[test]
    fn complete_tuple_is_required_as_the_unique_order_suffix() {
        assert!(
            generate_graphql_operations(&entity("rank desc, partition_key asc, item_no asc"))
                .is_ok()
        );
        for order in [
            "rank desc, partition_key asc",
            "rank desc, item_no asc",
            "rank desc, item_no asc, partition_key asc",
        ] {
            let error = generate_graphql_operations(&entity(order)).unwrap_err();
            assert!(
                error.to_string().contains("every primary-key field"),
                "{error}"
            );
        }
    }

    #[test]
    fn duplicate_composite_order_fields_are_rejected() {
        let error = generate_graphql_operations(&entity(
            "partition_key asc, rank desc, partition_key asc, item_no asc",
        ))
        .unwrap_err();
        assert!(
            error.to_string().contains("must not repeat a field"),
            "{error}"
        );
    }
}

#[cfg(test)]
mod query_shape_tests {
    use super::generate_graphql_operations;

    fn entity(shapes: &str) -> syn::DeriveInput {
        let shapes = syn::LitStr::new(shapes, proc_macro2::Span::call_site());
        syn::parse_quote! {
            #[graphql_entity(table = "query_shape_fixture", query_shapes = #shapes)]
            struct QueryShapeFixture {
                #[primary_key]
                id: String,
            }
        }
    }

    #[test]
    fn malformed_or_unavailable_query_selections_fail_closed() {
        assert!(generate_graphql_operations(&entity("list")).is_ok());
        for shapes in [
            "",
            "unknown",
            "single,single",
            "keyset",
            "scan",
            "search",
            "aggregate",
        ] {
            assert!(
                generate_graphql_operations(&entity(shapes)).is_err(),
                "{shapes}"
            );
        }
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod upsert_primary_key_tests {
    use super::generate_graphql_operations;

    #[test]
    fn the_whole_composite_primary_key_is_an_upsert_target() {
        let input: syn::DeriveInput = syn::parse_quote! {
            #[graphql_entity(backend = "sqlite", table = "allocations", upsert = "region, slot")]
            struct Allocation {
                #[primary_key]
                #[graphql_orm(auto_generated = false)]
                region: String,
                #[primary_key]
                #[graphql_orm(auto_generated = false, db_column = "slot_number")]
                slot: i64,
                label: String,
            }
        };
        if let Err(error) = generate_graphql_operations(&input) {
            panic!("{error}");
        }
        let partial: syn::DeriveInput = syn::parse_quote! {
            #[graphql_entity(backend = "sqlite", table = "allocations", upsert = "slot")]
            struct Allocation {
                #[primary_key]
                #[graphql_orm(auto_generated = false)]
                region: String,
                #[primary_key]
                #[graphql_orm(auto_generated = false)]
                slot: i64,
            }
        };
        let error = generate_graphql_operations(&partial).unwrap_err();
        assert!(
            error.to_string().contains("must match a unique field"),
            "{error}"
        );
    }

    #[test]
    fn an_implicit_renamed_id_key_is_an_upsert_target() {
        let input: syn::DeriveInput = syn::parse_quote! {
            #[graphql_entity(backend = "sqlite", table = "records", upsert = "id")]
            struct Record {
                #[graphql_orm(auto_generated = false, db_column = "record_key")]
                id: String,
                label: String,
            }
        };
        if let Err(error) = generate_graphql_operations(&input) {
            panic!("{error}");
        }
    }
}

#[cfg(test)]
mod update_assignment_tests {
    use super::{
        contains_question_mark, ensure_pure_update_assignments, first_identifier_in,
        generate_graphql_operations,
    };
    use quote::quote;

    fn generated(entity: syn::DeriveInput) -> String {
        generate_graphql_operations(&entity)
            .expect("operations generate")
            .to_string()
    }

    fn count(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    /// The per-field UPDATE assignments are emitted once per entity and called
    /// from every generated update path (#677).
    #[test]
    fn infallible_assignments_are_emitted_once_and_shared_with_graphql() {
        let generated = generated(syn::parse_quote! {
            #[graphql_entity(table = "update_assignment_fixture")]
            struct UpdateAssignmentFixture {
                #[primary_key]
                id: String,
                title: String,
                rank: Option<i64>,
            }
        });
        assert_eq!(
            count(&generated, "changed_fields . push (\"title\")"),
            1,
            "the title assignment is emitted once"
        );
        assert_eq!(count(&generated, "changed_fields . push (\"rank\")"), 1);
        assert_eq!(count(&generated, "fn __gom_push_update_assignments ("), 1);
        assert_eq!(
            count(&generated, "fn __gom_push_update_assignments_graphql ("),
            0,
            "an infallible list needs no GraphQL-typed copy"
        );
        // update_by_id, update_where and their two transaction forms.
        assert_eq!(
            count(
                &generated,
                ":: __gom_push_update_assignments (& input , & mut set_clauses , & mut changed_fields , & mut values ,) ?"
            ),
            4
        );
        // The GraphQL resolver sanitizes an error from the shared helper.
        assert_eq!(
            count(
                &generated,
                ":: __gom_push_update_assignments (& input , & mut set_clauses , & mut changed_fields , & mut values ,) . map_err (:: graphql_orm :: graphql :: errors :: graphql_error_from_sqlx) ?"
            ),
            1
        );
    }

    /// A fallible conversion names its error type, so the GraphQL resolver
    /// keeps a copy that fails with `async_graphql::Error` as before.
    #[test]
    fn fallible_assignments_keep_a_graphql_typed_copy() {
        let generated = generated(syn::parse_quote! {
            #[graphql_entity(table = "update_assignment_json_fixture")]
            struct UpdateAssignmentJsonFixture {
                #[primary_key]
                id: String,
                tags: Vec<String>,
            }
        });
        assert_eq!(count(&generated, "changed_fields . push (\"tags\")"), 2);
        assert_eq!(count(&generated, "fn __gom_push_update_assignments ("), 1);
        assert_eq!(
            count(&generated, "fn __gom_push_update_assignments_graphql ("),
            1
        );
        assert_eq!(
            count(
                &generated,
                ":: __gom_push_update_assignments_graphql (& input , & mut set_clauses , & mut changed_fields , & mut values ,) ?"
            ),
            1
        );
        assert_eq!(
            count(
                &generated,
                ":: __gom_push_update_assignments (& input , & mut set_clauses , & mut changed_fields , & mut values ,) ?"
            ),
            4
        );
    }

    /// Field write checks run before the shared assignment helper (#677, #678).
    #[test]
    fn field_write_checks_precede_the_assignment_helper() {
        let generated = generated(syn::parse_quote! {
            #[graphql_entity(table = "update_assignment_policy_fixture")]
            struct UpdateAssignmentPolicyFixture {
                #[primary_key]
                id: String,
                #[graphql_orm(write_policy = "fixture.title.write")]
                title: String,
            }
        });
        let body_of = |signature: &str| -> &str {
            let start = generated
                .find(signature)
                .unwrap_or_else(|| panic!("generated code has `{signature}`"));
            let rest = &generated[start..];
            let end = rest[signature.len()..]
                .find("async fn ")
                .map_or(rest.len(), |next| next + signature.len());
            &rest[..end]
        };
        let helper_call = ":: __gom_push_update_assignments (";

        let resolver = body_of("async fn update (");
        let field_check = resolver
            .find("check_field_write (")
            .expect("the GraphQL update resolver checks the field write policy");
        assert!(resolver.contains("\"fixture.title.write\""));
        let assignments = resolver
            .find(helper_call)
            .expect("the GraphQL update resolver calls the assignment helper");
        assert!(
            field_check < assignments,
            "the field write check precedes the assignments in the GraphQL update resolver"
        );
        assert_eq!(count(resolver, helper_call), 1);

        let repository = body_of("pub async fn update_by_id (");
        let field_checks = repository
            .find("authorize_repository_update_fields (")
            .expect("update_by_id authorizes the updated fields");
        let assignments = repository
            .find(helper_call)
            .expect("update_by_id calls the assignment helper");
        assert!(
            field_checks < assignments,
            "the field authorization precedes the assignments in update_by_id"
        );
        // The generated field write decision (asked for every supplied field,
        // key or no key) also precedes the assignments.
        let field_decision = repository
            .find("__gom_authorize_repository_update_field_writes (")
            .expect("update_by_id asks the repository field write decision");
        assert!(
            field_decision < assignments,
            "the field write decision precedes the assignments in update_by_id"
        );
        assert_eq!(count(repository, helper_call), 1);
    }

    #[test]
    fn impure_assignment_lists_are_refused() {
        let span = proc_macro2::Span::call_site();
        let pure = [quote! {
            if let Some(ref val) = input.title {
                changed_fields.push("title");
                set_clauses.push(format!("{} = ?", "title"));
                values.push(convert(val)?);
            }
        }];
        assert!(ensure_pure_update_assignments("update_field_checks_repo", &pure, span).is_ok());

        for (tokens, found) in [
            (
                quote! { if !db.check_field_write(input).await? { input.title = None; } },
                "db",
            ),
            (quote! { let allowed = { policy(ctx) }; }, "ctx"),
            (quote! { let allowed = check(input).await; }, "await"),
            (
                quote! { if input.title.is_none() { return Ok(()); } },
                "return",
            ),
        ] {
            let error =
                ensure_pure_update_assignments("update_field_checks_graphql", &[tokens], span)
                    .expect_err("an impure entry is refused")
                    .to_string();
            assert!(error.contains(&format!("`update_field_checks_graphql` contains `{found}`")));
            assert!(error.contains("update_policy_checks"));
            assert!(error.contains("update_runtime_policy_checks"));
            assert!(error.contains("authorize_repository_update_fields"));
        }
    }

    #[test]
    fn identifier_detection_ignores_literals_and_longer_names() {
        assert_eq!(
            first_identifier_in(
                quote! { values.push(db_column("ctx return await", context)); },
                &["await", "return", "ctx", "db"],
                &["await"]
            ),
            None
        );
        assert_eq!(
            first_identifier_in(quote! { values.push({ [db] }); }, &["ctx", "db"], &[]),
            Some("db")
        );
    }

    /// `input.db` and `input.ctx` read fields the entity declares; a
    /// free-standing `db` or `ctx`, and `.await`, are still reported.
    #[test]
    fn identifier_detection_ignores_member_names_but_not_await() {
        let names = ["await", "return", "ctx", "db"];
        assert_eq!(
            first_identifier_in(
                quote! {
                    if let Some(ref val) = input.db { values.push(convert(val.ctx())); }
                    if let Some(ref val) = input.r#return { values.push(convert(val)); }
                    if let Some(ref val) = input.r#await { values.push(r#return(val)); }
                },
                &names,
                &["await"]
            ),
            None
        );
        assert_eq!(
            first_identifier_in(quote! { input.db.push(db.pool()); }, &names, &["await"]),
            Some("db")
        );
        assert_eq!(
            first_identifier_in(quote! { check(input.ctx, ctx); }, &names, &["await"]),
            Some("ctx")
        );
        assert_eq!(
            first_identifier_in(quote! { check(input.db).await; }, &names, &["await"]),
            Some("await")
        );
        assert_eq!(
            first_identifier_in(quote! { value.return_code(); return; }, &names, &["await"]),
            Some("return")
        );
    }

    /// An entity may name its own fields `db` and `ctx` (#680).
    #[test]
    fn fields_named_db_and_ctx_generate() {
        let generated = generated(syn::parse_quote! {
            #[graphql_entity(table = "update_assignment_names_fixture")]
            struct UpdateAssignmentNamesFixture {
                #[primary_key]
                id: String,
                db: String,
                ctx: Option<String>,
            }
        });
        assert_eq!(count(&generated, "changed_fields . push (\"db\")"), 1);
        assert_eq!(count(&generated, "changed_fields . push (\"ctx\")"), 1);
        assert_eq!(count(&generated, "= input . db"), 1);
        assert_eq!(count(&generated, "= input . ctx"), 1);
    }

    /// Raw identifiers are not the keywords the purity check looks for.
    #[test]
    fn raw_identifier_fields_generate() {
        let generated = generated(syn::parse_quote! {
            #[graphql_entity(table = "update_assignment_raw_fixture")]
            struct UpdateAssignmentRawFixture {
                #[primary_key]
                id: String,
                r#return: String,
                r#await: Option<String>,
            }
        });
        assert_eq!(count(&generated, "= input . r#return"), 1);
        assert_eq!(count(&generated, "= input . r#await"), 1);
    }

    #[test]
    fn question_mark_detection_ignores_literals_and_reads_groups() {
        assert!(!contains_question_mark(
            quote! { set_clauses.push(format!("{} = ?", "title")); }
        ));
        assert!(contains_question_mark(
            quote! { values.push({ convert(value)? }); }
        ));
    }
}

#[cfg(test)]
mod write_transform_rejection_tests {
    #[test]
    fn rejects_write_transforms_instead_of_skipping_typed_binds() {
        let input = syn::parse_quote! {
            #[graphql_entity(backend = "sqlite", table = "transform_records")]
            struct TransformRecord {
                #[primary_key]
                id: String,
                #[transform(write = "protect")]
                value: i64,
            }
        };
        let error = super::generate_graphql_operations(&input)
            .expect_err("write transforms must not be silently bypassed");
        assert!(error.to_string().contains("transform(write"));
    }
}
