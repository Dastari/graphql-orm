//! Keep generated identity fields authoritative on composed complex objects.
//!
//! A composed object serves `NodeId` and `RecordVersion` from generated,
//! policy-checked resolvers. `#[graphql_complex_object]` mounts it before the
//! handwritten fields, so it always resolves first, and mounts it again last,
//! so its field definitions are the advertised ones. The checks here turn a
//! handwritten field that would carry either name into a compile error.

use syn::punctuated::Punctuated;
use syn::{Expr, Lit, Meta, Token};

/// GraphQL names of the identity fields a composed object may generate.
pub(crate) const IDENTITY_FIELD_NAMES: [&str; 2] = ["NodeId", "RecordVersion"];

/// Read a string literal, looking through the invisible groups that
/// `macro_rules!` fragments such as `$name:expr` produce.
pub(crate) fn string_literal(expr: &Expr, option: &str) -> syn::Result<String> {
    match expr {
        Expr::Group(group) => string_literal(&group.expr, option),
        Expr::Paren(paren) => string_literal(&paren.expr, option),
        Expr::Lit(literal) => match &literal.lit {
            Lit::Str(value) => Ok(value.value()),
            other => Err(syn::Error::new_spanned(
                other,
                format!("`{option}` requires a string literal"),
            )),
        },
        other => Err(syn::Error::new_spanned(
            other,
            format!("`{option}` requires a string literal"),
        )),
    }
}

fn parse_options(attribute: &syn::Attribute) -> syn::Result<Punctuated<Meta, Token![,]>> {
    attribute
        .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        .map_err(|error| {
            syn::Error::new(
                error.span(),
                format!(
                    "graphql_complex_object could not read this #[graphql(...)] attribute: {error}"
                ),
            )
        })
}

/// The identity field a name would produce, if any.
///
/// Explicit names match exactly. A name derived from a Rust identifier can only
/// be capitalised under `PascalCase`, so it matches when its letters do,
/// ignoring case and underscores; async-graphql's own case conversion may
/// split mixed-case identifiers differently from this crate's.
fn identity_name(name: &str, explicit: bool) -> Option<&'static str> {
    if explicit {
        return IDENTITY_FIELD_NAMES
            .into_iter()
            .find(|identity| *identity == name);
    }
    let folded: String = name
        .chars()
        .filter(|character| *character != '_')
        .flat_map(char::to_lowercase)
        .collect();
    IDENTITY_FIELD_NAMES
        .into_iter()
        .find(|identity| identity.to_lowercase() == folded)
}

/// What a field's or method's `#[graphql(...)]` attributes say about its names.
#[derive(Default)]
struct GraphqlNames {
    /// The explicit `name`, which also names every derived copy.
    explicit: Option<String>,
    /// `skip` or `skip_output`.
    skipped: bool,
    /// Identifiers of `derived(name = "...")` copies.
    derived: Vec<String>,
}

/// Whether a boolean option such as `skip` is set, as `skip` or `skip = true`.
fn flag(option: &Meta, name: &str) -> syn::Result<bool> {
    match option {
        Meta::Path(path) if path.is_ident(name) => Ok(true),
        Meta::NameValue(value) if value.path.is_ident(name) => {
            let mut expr = &value.value;
            while let Expr::Group(group) = expr {
                expr = &group.expr;
            }
            match expr {
                Expr::Lit(syn::ExprLit {
                    lit: Lit::Bool(value),
                    ..
                }) => Ok(value.value),
                other => Err(syn::Error::new_spanned(
                    other,
                    format!("`{name}` requires a boolean literal"),
                )),
            }
        }
        _ => Ok(false),
    }
}

/// Read every `#[graphql(...)]` attribute, refusing any it cannot read.
fn read_graphql_names(
    attributes: &[syn::Attribute],
    skip_options: &[&str],
) -> syn::Result<GraphqlNames> {
    let mut names = GraphqlNames::default();
    for attribute in attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("graphql"))
    {
        for option in parse_options(attribute)? {
            for skip in skip_options {
                names.skipped |= flag(&option, skip)?;
            }
            match &option {
                Meta::NameValue(value) if value.path.is_ident("name") => {
                    names.explicit = Some(string_literal(&value.value, "name")?);
                }
                Meta::List(list) if list.path.is_ident("derived") => {
                    let nested =
                        list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
                    for meta in nested {
                        if let Meta::NameValue(value) = meta
                            && value.path.is_ident("name")
                        {
                            names
                                .derived
                                .push(string_literal(&value.value, "derived(name)")?);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(names)
}

/// The identity field a published name would be, following async-graphql.
///
/// An explicit `name` is used as written. Otherwise async-graphql renames the
/// identifier by `rename_fields`, camelCase by default, and only `PascalCase`
/// can produce a capitalised identity name.
fn published_identity(explicit: Option<&str>, ident: &str, pascal: bool) -> Option<&'static str> {
    match explicit {
        Some(name) => identity_name(name, true),
        None if pascal => identity_name(ident, false),
        None => None,
    }
}

/// Whether a `rename_fields` value is `PascalCase`.
pub(crate) fn rename_is_pascal(rule: Option<&str>) -> bool {
    rule == Some("PascalCase")
}

/// Compile-time assertions for handwritten composed fields named like a
/// generated identity field.
///
/// Each candidate is checked against the names the entity's composed object
/// actually generates, so a handwritten `RecordVersion` stays valid on an
/// entity without a managed version.
pub(crate) fn shadow_checks(
    args: &proc_macro2::TokenStream,
    item_impl: &syn::ItemImpl,
    self_ty: &syn::Type,
) -> syn::Result<Vec<proc_macro2::TokenStream>> {
    use syn::ext::IdentExt as _;
    let mut pascal = false;
    if !args.is_empty() {
        let options = syn::parse::Parser::parse2(
            Punctuated::<Meta, Token![,]>::parse_terminated,
            args.clone(),
        )?;
        for option in options {
            if let Meta::NameValue(value) = option
                && value.path.is_ident("rename_fields")
            {
                pascal = string_literal(&value.value, "rename_fields")? == "PascalCase";
            }
        }
    }

    let mut checks = Vec::new();
    for item in &item_impl.items {
        let syn::ImplItem::Fn(method) = item else {
            continue;
        };
        // async-graphql copies the method, attributes included, for each
        // `derived` entry, so copies share its explicit name and its skip.
        let names = read_graphql_names(&method.attrs, &["skip"])?;
        let ident = method.sig.ident.unraw().to_string();
        let mut candidates = Vec::new();
        if !names.skipped {
            candidates.extend(published_identity(
                names.explicit.as_deref(),
                &ident,
                pascal,
            ));
            for derived in &names.derived {
                candidates.extend(published_identity(
                    names.explicit.as_deref(),
                    derived,
                    pascal,
                ));
            }
        }
        for name in candidates {
            let message = format!(
                "`{ident}` declares the GraphQL field `{name}`, which the composed object already generates with its declared policy; remove this handwritten field"
            );
            checks.push(quote::quote_spanned! {method.sig.ident.span()=>
                const _: () = ::std::assert!(
                    !::graphql_orm::graphql::orm::generated_field_names_contain(
                        <#self_ty as ::graphql_orm::graphql::orm::GeneratedRelationsObject>
                            ::GENERATED_IDENTITY_FIELDS,
                        #name,
                    ),
                    #message,
                );
            });
        }
    }
    Ok(checks)
}

/// Refuse struct fields whose GraphQL name is a generated identity field.
///
/// `SimpleObject` resolves its own and derived fields before any complex
/// field, so such a field would be served in place of the generated one. Its
/// names follow async-graphql: an explicit `name` names the field and every
/// derived copy, otherwise the struct's `rename_fields` (camelCase by default)
/// renames the identifier, and derived copies exist even when the base field
/// is skipped. A generated object names its fields as the ORM does.
pub(crate) fn check_struct_fields(
    fields: &[crate::entity::ParsedField],
    simple_object: bool,
    graphql_rename_fields: Option<&str>,
    serde_rename_all: Option<&str>,
    node_id: bool,
    managed_version: bool,
) -> syn::Result<()> {
    use syn::ext::IdentExt as _;
    let reserved = |name: &str| match name {
        "NodeId" => node_id,
        "RecordVersion" => managed_version,
        _ => false,
    };
    let pascal = rename_is_pascal(graphql_rename_fields);
    for parsed in fields {
        let names = read_graphql_names(&parsed.field.attrs, &["skip", "skip_output"])?;
        let ident = parsed
            .field
            .ident
            .as_ref()
            .map(|ident| ident.unraw().to_string())
            .unwrap_or_default();
        let base = if names.skipped || !parsed.meta.graphql_readable() || parsed.meta.is_relation {
            None
        } else if simple_object {
            published_identity(names.explicit.as_deref(), &ident, pascal)
        } else {
            identity_name(
                &crate::naming::graphql_field_name(
                    &parsed.meta,
                    &ident,
                    graphql_rename_fields,
                    serde_rename_all,
                ),
                true,
            )
        };
        if let Some(name) = base.filter(|name| reserved(name)) {
            return Err(syn::Error::new_spanned(
                &parsed.field,
                format!(
                    "this field is published as `{name}`, which this entity serves as a generated identity field; rename this field"
                ),
            ));
        }
        if !simple_object {
            continue;
        }
        for derived in &names.derived {
            if let Some(name) = published_identity(names.explicit.as_deref(), derived, pascal)
                .filter(|name| reserved(name))
            {
                return Err(syn::Error::new_spanned(
                    &parsed.field,
                    format!(
                        "derived field `{derived}` is published as `{name}`, which this entity serves as a generated identity field; choose another name"
                    ),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_names_match_explicitly_or_by_letters() {
        assert_eq!(identity_name("RecordVersion", true), Some("RecordVersion"));
        assert_eq!(identity_name("recordVersion", true), None);
        assert_eq!(identity_name("node_id", false), Some("NodeId"));
        assert_eq!(identity_name("NODeId", false), Some("NodeId"));
        assert_eq!(
            identity_name("record_version", false),
            Some("RecordVersion")
        );
        assert_eq!(identity_name("label", false), None);
    }

    #[test]
    fn literals_are_read_through_macro_groups() {
        let grouped = Expr::Group(syn::ExprGroup {
            attrs: Vec::new(),
            group_token: Default::default(),
            expr: Box::new(syn::parse_quote!("NodeId")),
        });
        assert_eq!(string_literal(&grouped, "name").unwrap(), "NodeId");
        assert!(string_literal(&syn::parse_quote!(NAME), "name").is_err());
    }
}
