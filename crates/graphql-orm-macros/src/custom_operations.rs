use proc_macro::TokenStream;
use quote::{ToTokens, quote};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    FnArg, GenericArgument, ImplItem, ItemImpl, Meta, Pat, PathArguments, ReturnType, Token, Type,
};

use crate::entity::{
    semantic_classification_tokens, semantic_doc_description, validate_semantic_classification,
    validate_semantic_description,
};

struct CustomRootArgs {
    kind: String,
    authorization: DeclaredAuthorization,
    ai_execution: Option<String>,
    observation: Option<String>,
    maximum_duration_seconds: Option<u32>,
    maximum_events: Option<u32>,
}

impl Parse for CustomRootArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let items = Punctuated::<Meta, Token![,]>::parse_terminated(input)?;
        let mut kind = None;
        let mut authorization = None;
        let mut scopes = None;
        let mut ai_execution = None;
        let mut observation = None;
        let mut maximum_duration_seconds = None;
        let mut maximum_events = None;
        for item in items {
            let Meta::NameValue(value) = item else {
                return Err(syn::Error::new_spanned(
                    item,
                    "expected a name-value argument",
                ));
            };
            if value.path.is_ident("kind") {
                let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(lit),
                    ..
                }) = value.value
                else {
                    return Err(syn::Error::new_spanned(value, "kind must be a string"));
                };
                kind = Some(lit.value());
            } else if value.path.is_ident("authorization") {
                let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Bool(lit),
                    ..
                }) = value.value
                else {
                    return Err(syn::Error::new_spanned(
                        value,
                        "authorization must be a boolean",
                    ));
                };
                if authorization.replace(lit.value).is_some() {
                    return Err(syn::Error::new_spanned(
                        value.path,
                        "duplicate authorization",
                    ));
                }
            } else if value.path.is_ident("all_scopes") || value.path.is_ident("any_scopes") {
                let any = value.path.is_ident("any_scopes");
                let alternatives = parse_scope_alternatives(&value.value, any)?;
                if scopes.replace(alternatives).is_some() {
                    return Err(syn::Error::new_spanned(
                        value.path,
                        "declare one of all_scopes or any_scopes, once",
                    ));
                }
            } else if value.path.is_ident("ai_execution") {
                let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(lit),
                    ..
                }) = value.value
                else {
                    return Err(syn::Error::new_spanned(
                        value,
                        "ai_execution must be a string",
                    ));
                };
                if !matches!(
                    lit.value().as_str(),
                    "automatic" | "approval_required" | "prohibited"
                ) {
                    return Err(syn::Error::new_spanned(
                        lit,
                        "ai_execution must be automatic, approval_required, or prohibited",
                    ));
                }
                ai_execution = Some(lit.value());
            } else if value.path.is_ident("observation") {
                let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(lit),
                    ..
                }) = value.value
                else {
                    return Err(syn::Error::new_spanned(
                        value,
                        "observation must be a string",
                    ));
                };
                if !matches!(lit.value().as_str(), "best_effort" | "replay_then_live") {
                    return Err(syn::Error::new_spanned(
                        lit,
                        "observation must be best_effort or replay_then_live",
                    ));
                }
                observation = Some(lit.value());
            } else if value.path.is_ident("maximum_duration_seconds") {
                let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Int(lit),
                    ..
                }) = value.value
                else {
                    return Err(syn::Error::new_spanned(
                        value,
                        "maximum_duration_seconds must be a positive integer",
                    ));
                };
                maximum_duration_seconds = Some(lit.base10_parse::<u32>()?);
            } else if value.path.is_ident("maximum_events") {
                let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Int(lit),
                    ..
                }) = value.value
                else {
                    return Err(syn::Error::new_spanned(
                        value,
                        "maximum_events must be a positive integer",
                    ));
                };
                maximum_events = Some(lit.base10_parse::<u32>()?);
            } else {
                return Err(syn::Error::new_spanned(
                    value.path,
                    "unsupported semantic-root argument",
                ));
            }
        }
        let kind = kind.ok_or_else(|| syn::Error::new(input.span(), "kind is required"))?;
        if !matches!(kind.as_str(), "query" | "mutation" | "subscription") {
            return Err(syn::Error::new(
                input.span(),
                "kind must be query, mutation, or subscription",
            ));
        }
        let has_observation_option =
            observation.is_some() || maximum_duration_seconds.is_some() || maximum_events.is_some();
        if has_observation_option && kind != "subscription" {
            return Err(syn::Error::new(
                input.span(),
                "observation options are valid only for subscription roots",
            ));
        }
        if ai_execution.is_some() && kind != "mutation" {
            return Err(syn::Error::new(
                input.span(),
                "ai_execution is valid only for mutation roots",
            ));
        }
        if observation.is_some() && (maximum_duration_seconds.is_none() || maximum_events.is_none())
        {
            return Err(syn::Error::new(
                input.span(),
                "subscription observation requires maximum_duration_seconds and maximum_events",
            ));
        }
        if observation.is_none() && (maximum_duration_seconds.is_some() || maximum_events.is_some())
        {
            return Err(syn::Error::new(
                input.span(),
                "subscription observation bounds require observation",
            ));
        }
        if maximum_duration_seconds == Some(0) || maximum_events == Some(0) {
            return Err(syn::Error::new(
                input.span(),
                "subscription observation bounds must be positive",
            ));
        }
        // A described root is protected unless it says otherwise.
        let authorization = DeclaredAuthorization::declared(authorization, scopes, input.span())?;
        Ok(Self {
            kind,
            authorization,
            ai_execution,
            observation,
            maximum_duration_seconds,
            maximum_events,
        })
    }
}

/// Scope alternatives: the subject must hold every scope of at least one.
type ScopeAlternatives = Vec<Vec<String>>;

/// What a described root or resolver declares about authorization, and so
/// what the generated code enforces before the resolver body runs.
///
/// There is no third state: a root is explicitly public, or it carries a
/// requirement the generated guard enforces.
#[derive(Clone, PartialEq)]
enum DeclaredAuthorization {
    /// `authorization = false`: no ORM check, and the metadata says so.
    Public,
    /// An authenticated subject that satisfies every listed group of scope
    /// alternatives. No group means authentication alone.
    Required(Vec<ScopeAlternatives>),
}

impl DeclaredAuthorization {
    /// A root's declaration from `authorization` and `all_scopes`/`any_scopes`.
    fn declared(
        authorization: Option<bool>,
        scopes: Option<ScopeAlternatives>,
        span: proc_macro2::Span,
    ) -> syn::Result<Self> {
        match (authorization, scopes) {
            (Some(false), Some(_)) => Err(syn::Error::new(
                span,
                "`authorization = false` declares a public root and cannot be combined with \
                 all_scopes or any_scopes; remove the scopes, or remove `authorization = false`",
            )),
            (Some(false), None) => Ok(Self::Public),
            (_, Some(scopes)) => Ok(Self::Required(vec![scopes])),
            (_, None) => Ok(Self::Required(Vec::new())),
        }
    }

    fn is_required(&self) -> bool {
        matches!(self, Self::Required(_))
    }

    /// The `&[&[&str]]` scope alternatives of one requirement group.
    fn alternatives_tokens(alternatives: &[Vec<String>]) -> proc_macro2::TokenStream {
        let alternatives = alternatives.iter().map(|alternative| {
            let scopes = alternative
                .iter()
                .map(|scope| syn::LitStr::new(scope, proc_macro2::Span::call_site()));
            quote! { &[#(#scopes),*] }
        });
        quote! { &[#(#alternatives),*] }
    }

    /// The guard expressions that enforce this declaration, in order.
    fn guard_sources(&self) -> Vec<String> {
        match self {
            Self::Public => Vec::new(),
            Self::Required(groups) if groups.is_empty() => vec![
                "::graphql_orm::graphql::auth::DeclaredAuthorizationGuard::authenticated()"
                    .to_owned(),
            ],
            Self::Required(groups) => groups
                .iter()
                .map(|alternatives| {
                    format!(
                        "::graphql_orm::graphql::auth::DeclaredAuthorizationGuard::scopes({})",
                        Self::alternatives_tokens(alternatives)
                    )
                })
                .collect(),
        }
    }

    /// Statements that enforce this declaration against `context`, each
    /// ending in `?`. Empty for a public declaration.
    fn enforcement_tokens(&self, context: &syn::Ident) -> proc_macro2::TokenStream {
        let Self::Required(groups) = self else {
            return quote! {};
        };
        let authenticated = [Vec::new()];
        let groups = if groups.is_empty() {
            &authenticated[..]
        } else {
            &groups[..]
        };
        let checks = groups.iter().map(|alternatives| {
            let alternatives = Self::alternatives_tokens(alternatives);
            quote! {
                ::graphql_orm::graphql::auth::enforce_declared_authorization(
                    #context,
                    #alternatives,
                )?;
            }
        });
        quote! { #(#checks)* }
    }
}

/// What one resolver declares about its root's authorization.
///
/// A resolver may only narrow its root's requirement unless it says, in so
/// many words, that it replaces it or is public: a requirement weaker than
/// the root's is always visible at the resolver.
#[derive(Clone)]
enum ResolverAuthorization {
    /// `authorization = true` and/or `all_scopes`/`any_scopes`: the root's
    /// requirement, plus authentication, plus these scopes.
    Narrow(Option<ScopeAlternatives>),
    /// `authorization(replace, ...)`: this requirement instead of the root's.
    Replace(Option<ScopeAlternatives>),
    /// `authorization = false`: this resolver is public.
    Public,
}

impl ResolverAuthorization {
    /// The requirement a resolver carries under its root's declaration.
    fn apply(declared: Option<&Self>, root: &DeclaredAuthorization) -> DeclaredAuthorization {
        match declared {
            None => root.clone(),
            Some(Self::Public) => DeclaredAuthorization::Public,
            Some(Self::Replace(scopes)) => {
                DeclaredAuthorization::Required(scopes.iter().cloned().collect())
            }
            Some(Self::Narrow(scopes)) => {
                let mut groups = match root {
                    DeclaredAuthorization::Required(groups) => groups.clone(),
                    DeclaredAuthorization::Public => Vec::new(),
                };
                groups.extend(scopes.iter().cloned());
                DeclaredAuthorization::Required(groups)
            }
        }
    }
}

/// Parse `all_scopes = ["a", "b"]` (one alternative) or
/// `any_scopes = [["a"], ["b", "c"]]` (several) into scope alternatives.
///
/// The rules are those of an entity's fixed `operation_authorization` scopes:
/// no empty list, and no empty, whitespace or brace-carrying scope.
fn parse_scope_alternatives(value: &syn::Expr, any: bool) -> syn::Result<ScopeAlternatives> {
    let name = if any { "any_scopes" } else { "all_scopes" };
    let scope_list = |expr: &syn::Expr| -> syn::Result<Vec<String>> {
        let syn::Expr::Array(array) = expr else {
            return Err(syn::Error::new_spanned(
                expr,
                format!("{name} must be an array of string literals"),
            ));
        };
        if array.elems.is_empty() {
            return Err(syn::Error::new_spanned(
                array,
                format!("{name} must not contain an empty scope list"),
            ));
        }
        array
            .elems
            .iter()
            .map(|element| {
                let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(scope),
                    ..
                }) = element
                else {
                    return Err(syn::Error::new_spanned(
                        element,
                        format!("{name} must contain only string literals"),
                    ));
                };
                let text = scope.value();
                if text.is_empty()
                    || text.contains(['{', '}'])
                    || text
                        .chars()
                        .any(|character| character.is_whitespace() || character.is_control())
                {
                    return Err(syn::Error::new_spanned(
                        scope,
                        "scope strings must not be empty or contain whitespace, control \
                         characters or braces",
                    ));
                }
                Ok(text)
            })
            .collect()
    };
    if !any {
        return Ok(vec![scope_list(value)?]);
    }
    let syn::Expr::Array(alternatives) = value else {
        return Err(syn::Error::new_spanned(
            value,
            "any_scopes must be an array of nonempty scope arrays",
        ));
    };
    if alternatives.elems.is_empty() {
        return Err(syn::Error::new_spanned(
            alternatives,
            "any_scopes must contain at least one nonempty alternative",
        ));
    }
    alternatives.elems.iter().map(scope_list).collect()
}

/// Every item of a target's `#[graphql(...)]` attributes, list forms included.
fn graphql_metas(attrs: &[syn::Attribute]) -> syn::Result<Vec<Meta>> {
    let mut metas = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("graphql")) {
        match &attr.meta {
            Meta::Path(_) => {}
            Meta::List(list) => {
                metas.extend(list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?)
            }
            Meta::NameValue(other) => {
                return Err(syn::Error::new_spanned(other, "expected `#[graphql(...)]`"));
            }
        }
    }
    Ok(metas)
}

/// Add a generated `desc` to an argument's async-graphql options as one
/// `#[graphql(...)]` attribute. Existing options (`name`, `default`,
/// validators, ...) keep their order and the merged attribute takes the place
/// of the first `graphql` attribute; a second, description-only attribute is
/// never added beside them.
fn merge_graphql_argument_description(
    attrs: &mut Vec<syn::Attribute>,
    description: syn::LitStr,
) -> syn::Result<()> {
    let mut metas = graphql_metas(attrs)?;
    metas.push(syn::parse_quote!(desc = #description));
    let position = attrs
        .iter()
        .position(|attr| attr.path().is_ident("graphql"))
        .unwrap_or(attrs.len());
    attrs.retain(|attr| !attr.path().is_ident("graphql"));
    let metas = metas.into_iter().collect::<Punctuated<Meta, Token![,]>>();
    attrs.insert(
        position.min(attrs.len()),
        syn::parse_quote!(#[graphql(#metas)]),
    );
    Ok(())
}

/// An expression without the invisible groups a `macro_rules!` fragment adds.
fn ungrouped(mut expression: &syn::Expr) -> &syn::Expr {
    while let syn::Expr::Group(group) = expression {
        expression = &group.expr;
    }
    expression
}

fn graphql_attribute_value(attrs: &[syn::Attribute], key: &str) -> syn::Result<Option<String>> {
    let mut result = None;
    for meta in graphql_metas(attrs)? {
        let Meta::NameValue(value) = meta else {
            continue;
        };
        if !value.path.is_ident(key) {
            continue;
        }
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(lit),
            ..
        }) = ungrouped(&value.value)
        else {
            return Err(syn::Error::new_spanned(
                &value.value,
                format!("graphql {key} must be a string"),
            ));
        };
        if result.replace(lit.value()).is_some() {
            return Err(syn::Error::new_spanned(
                &value.path,
                format!("duplicate graphql {key}"),
            ));
        }
    }
    Ok(result)
}

/// The source text of a `guard` value, as async-graphql reads it: a string
/// literal holds the expression, any other value is the expression itself.
fn guard_source(value: &syn::Expr) -> String {
    match value {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(lit),
            ..
        }) => lit.value(),
        expression => expression.to_token_stream().to_string(),
    }
}

/// The async-graphql root attribute a path names, when it is one this macro
/// recognises: `Object` or `Subscription`, bare (without a leading `::`) or
/// under `async_graphql::` or `graphql_orm::async_graphql::` (with or without
/// a leading `::`).
fn root_attribute_kind(path: &syn::Path) -> Option<&'static str> {
    let segments = path
        .segments
        .iter()
        .map(|segment| {
            segment
                .arguments
                .is_none()
                .then(|| segment.ident.to_string())
        })
        .collect::<Option<Vec<_>>>()?;
    let (last, prefix) = segments.split_last()?;
    let kind = match last.as_str() {
        "Object" => "Object",
        "Subscription" => "Subscription",
        _ => return None,
    };
    let prefix = prefix.iter().map(String::as_str).collect::<Vec<_>>();
    match prefix.as_slice() {
        [] if path.leading_colon.is_none() => Some(kind),
        ["async_graphql"] | ["graphql_orm", "async_graphql"] => Some(kind),
        _ => None,
    }
}

/// Check the root's own `#[Object(...)]` or `#[Subscription(...)]` attribute
/// and return its impl-level guard, if any.
///
/// The root attribute must be exactly one recognised path of the declared
/// kind, so its guard and `rename_fields` are always seen: an alias or a
/// missing attribute fails to compile rather than hiding them.
///
/// async-graphql applies an impl-level guard only to resolvers without their
/// own guard. Every described resolver receives a guard, so the impl-level one
/// is composed into it explicitly. `rename_fields` would rename the exposed
/// fields away from their catalogued coordinates, so it is rejected.
fn root_attribute_guard(item: &ItemImpl, kind: &str) -> syn::Result<Option<String>> {
    let expected = if kind == "subscription" {
        "Subscription"
    } else {
        "Object"
    };
    let mut roots = item
        .attrs
        .iter()
        .filter_map(|attr| root_attribute_kind(attr.path()).map(|found| (attr, found)));
    let Some((attr, found)) = roots.next() else {
        return Err(syn::Error::new(
            item.impl_token.span,
            format!(
                "`#[graphql_orm_custom_operations]` must be followed by \
                 `#[async_graphql::{expected}]` (or `#[{expected}]`, \
                 `#[graphql_orm::async_graphql::{expected}]`); renamed imports are not \
                 recognised"
            ),
        ));
    };
    if let Some((extra, _)) = roots.next() {
        return Err(syn::Error::new_spanned(
            extra,
            "a described custom root takes a single async-graphql root attribute",
        ));
    }
    if found != expected {
        return Err(syn::Error::new_spanned(
            attr,
            format!("`kind = \"{kind}\"` requires `#[{expected}]`, not `#[{found}]`"),
        ));
    }
    let mut guard = None;
    let Meta::List(list) = &attr.meta else {
        return Ok(None);
    };
    for meta in list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)? {
        if meta.path().is_ident("rename_fields") {
            return Err(syn::Error::new_spanned(
                meta,
                "described custom roots do not support `rename_fields`; name renamed \
                 resolvers with `#[graphql(name = \"...\")]` so the catalogued and \
                 exposed coordinates agree",
            ));
        }
        if meta.path().is_ident("rename_args") {
            return Err(syn::Error::new_spanned(
                meta,
                "described custom roots do not support `rename_args`; name renamed \
                 arguments with `#[graphql(name = \"...\")]` so the catalogued and \
                 exposed argument names agree",
            ));
        }
        if let Meta::NameValue(value) = &meta
            && value.path.is_ident("guard")
        {
            guard = Some(guard_source(&value.value));
        }
    }
    Ok(guard)
}

/// Reject resolver flags that expose fields under coordinates other than the
/// catalogued one, or serve fields without the resolver's guard.
fn reject_unmodelled_resolver_flags(attrs: &[syn::Attribute]) -> syn::Result<()> {
    for meta in graphql_metas(attrs)? {
        let path = meta.path();
        if path.is_ident("flatten") {
            return Err(syn::Error::new_spanned(
                meta,
                "described custom roots do not support `#[graphql(flatten)]`: the flattened \
                 fields would be served without this resolver's assurance guard",
            ));
        }
        if path.is_ident("derived") {
            return Err(syn::Error::new_spanned(
                meta,
                "described custom roots do not support `#[graphql(derived(...))]`: a derived \
                 field is exposed under an uncatalogued name; describe it as its own resolver",
            ));
        }
    }
    Ok(())
}

/// Install the generated guards on one described resolver.
///
/// async-graphql reads only the first `#[graphql(...)]` attribute of a
/// resolver, so a described root must have at most one. `generated` holds the
/// declared-authorization guard (when the root declares a requirement)
/// followed by `DeclaredAssuranceGuard::new(<root kind>)`. A host guard is
/// kept and composed after them with `GuardExt::and`, so it cannot replace,
/// weaken, or misclassify either check, and both run before the resolver body.
///
/// Returns the guard expression the resolver ends up with.
fn install_generated_guards(
    method: &mut syn::ImplItemFn,
    generated: &[String],
    root_guard: Option<&str>,
) -> syn::Result<String> {
    let compose = |host: Option<&str>| {
        let mut guards = generated.iter().map(String::as_str).chain(host).rev();
        let last = guards
            .next()
            .expect("a described root always has an assurance guard")
            .to_owned();
        guards.fold(last, |inner, outer| {
            format!("::graphql_orm::async_graphql::GuardExt::and({outer}, {inner})")
        })
    };
    // Without its own guard, async-graphql would have applied the root's.
    let default_guard = compose(root_guard);
    // `single_graphql_attribute` has already rejected any later attribute.
    let Some(attr) = method
        .attrs
        .iter_mut()
        .find(|attr| attr.path().is_ident("graphql"))
    else {
        let guard = syn::LitStr::new(&default_guard, method.sig.ident.span());
        method
            .attrs
            .push(syn::parse_quote!(#[graphql(guard = #guard)]));
        return Ok(default_guard);
    };
    let span = attr
        .path()
        .get_ident()
        .map_or_else(proc_macro2::Span::call_site, proc_macro2::Ident::span);
    let mut items = match &attr.meta {
        Meta::Path(_) => Punctuated::<Meta, Token![,]>::new(),
        Meta::List(list) => {
            list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?
        }
        Meta::NameValue(other) => {
            return Err(syn::Error::new_spanned(
                other,
                "expected `#[graphql(...)]` on a described custom root",
            ));
        }
    };
    let mut guards = items
        .iter_mut()
        .filter_map(|item| match item {
            Meta::NameValue(value) if value.path.is_ident("guard") => Some(value),
            _ => None,
        })
        .collect::<Vec<_>>();
    if guards.len() > 1 {
        return Err(syn::Error::new_spanned(
            &guards[1].path,
            "duplicate graphql guard",
        ));
    }
    let installed = if let Some(guard) = guards.pop() {
        let composed = compose(Some(&guard_source(&guard.value)));
        guard.value = syn::Expr::Lit(syn::ExprLit {
            attrs: Vec::new(),
            lit: syn::Lit::Str(syn::LitStr::new(&composed, span)),
        });
        composed
    } else {
        let guard = syn::LitStr::new(&default_guard, span);
        items.push(syn::parse_quote!(guard = #guard));
        default_guard
    };
    attr.meta = syn::parse_quote!(graphql(#items));
    Ok(installed)
}

/// Keep a described subscription's guards checked while its stream is open,
/// as generated `Changed` subscriptions keep their authority checked.
///
/// async-graphql evaluates the resolver's guard once, when the stream is
/// created. The resolver's own body moves into a hidden inherent method,
/// returned here, and the resolver becomes a wrapper that passes that
/// method's stream through `watch_declared_subscription`, which evaluates
/// the same composed guard again before each item is released and on a
/// timer, and ends the stream on the first refusal. The composed guard is the
/// declared authorization, operation assurance and the host guard, so a host
/// guard that consults live authority (for example
/// `AgqlCurrentAuthorityGuard`) revokes an open stream. A host guard that
/// reads a resolver argument is evaluated once only: the arguments belong to
/// the resolver body by then, so only the generated guards are re-evaluated.
fn watch_subscription_stream(
    method: &mut syn::ImplItemFn,
    generated_guards: &[String],
    installed_guard: &str,
) -> syn::Result<syn::ImplItemFn> {
    let inner_name = syn::Ident::new(
        &format!("__gom_described_{}", method.sig.ident.unraw()),
        method.sig.ident.span(),
    );
    let mut inner = method.clone();
    inner.sig.ident = inner_name.clone();
    inner.vis = syn::Visibility::Inherited;
    // The inner method is plain Rust: drop what only the root macro reads.
    inner.attrs.retain(|attr| {
        attr.path().is_ident("cfg")
            || attr.path().is_ident("allow")
            || attr.path().is_ident("expect")
    });
    inner.attrs.push(syn::parse_quote!(#[doc(hidden)]));
    inner
        .attrs
        .push(syn::parse_quote!(#[allow(clippy::too_many_arguments)]));
    for argument in &mut inner.sig.inputs {
        if let FnArg::Typed(argument) = argument {
            argument
                .attrs
                .retain(|attr| !attr.path().is_ident("graphql"));
        }
    }

    let context = syn::Ident::new("__gom_context", method.sig.ident.span());
    let mut has_context = false;
    let mut forwarded = Vec::new();
    for argument in &mut method.sig.inputs {
        let FnArg::Typed(argument) = argument else {
            continue;
        };
        if is_context(&argument.ty) {
            has_context = true;
            *argument.pat = syn::parse_quote!(#context);
            forwarded.push(context.clone());
            continue;
        }
        let Pat::Ident(pat) = argument.pat.as_mut() else {
            return Err(syn::Error::new_spanned(
                &argument.pat,
                "semantic resolver arguments require identifiers",
            ));
        };
        pat.mutability = None;
        pat.by_ref = None;
        forwarded.push(pat.ident.clone());
    }
    if !has_context {
        let position = usize::from(method.sig.receiver().is_some());
        method.sig.inputs.insert(
            position,
            syn::parse_quote!(#context: &::graphql_orm::async_graphql::Context<'_>),
        );
    }
    let returns_result = match &method.sig.output {
        ReturnType::Type(_, ty) => !std::ptr::eq(unwrap_result_type(ty), ty.as_ref()),
        ReturnType::Default => false,
    };
    let generated_only = generated_guards
        .iter()
        .rev()
        .cloned()
        .reduce(|inner, outer| {
            format!("::graphql_orm::async_graphql::GuardExt::and({outer}, {inner})")
        })
        .expect("a described root always has an assurance guard");
    let installed: proc_macro2::TokenStream = installed_guard.parse().map_err(|_| {
        syn::Error::new_spanned(
            &method.sig.ident,
            "the resolver's guard is not an expression",
        )
    })?;
    // The guard is built before the body takes the arguments. One that names
    // an argument (or `self`) cannot be rebuilt afterwards.
    let reads_resolver_state =
        installed
            .clone()
            .into_iter()
            .flat_map(flatten_tokens)
            .any(|token| {
                matches!(&token, proc_macro2::TokenTree::Ident(ident)
            if ident == "self" || forwarded.iter().any(|argument| ident == argument))
            });
    let watched: proc_macro2::TokenStream = if reads_resolver_state {
        generated_only
            .parse()
            .expect("generated guards are valid expressions")
    } else {
        installed
    };
    let authorize = quote! {
        {
            let __gom_guard = ::std::sync::Arc::new(#watched);
            move || {
                let __gom_guard = ::std::sync::Arc::clone(&__gom_guard);
                async move {
                    ::graphql_orm::async_graphql::Guard::check(&*__gom_guard, #context).await
                }
            }
        }
    };
    let receiver = if method.sig.receiver().is_some() {
        quote! { self.#inner_name }
    } else {
        quote! { Self::#inner_name }
    };
    method.block = if returns_result {
        syn::parse_quote! {{
            #receiver(#(#forwarded),*).await.map(|__gom_stream| {
                ::graphql_orm::graphql::subscriptions::watch_declared_subscription(
                    __gom_stream,
                    #authorize,
                )
            })
        }}
    } else {
        syn::parse_quote! {{
            ::graphql_orm::graphql::subscriptions::watch_declared_subscription(
                #receiver(#(#forwarded),*).await,
                #authorize,
            )
        }}
    };
    Ok(inner)
}

/// Enforce the declared authorization and the operation-assurance requirement
/// inside a `#[graphql(entity)]` resolver.
///
/// async-graphql applies no guard to an entity resolver, so the checks are
/// the first statements of its body: authentication and scopes, then the
/// assurance requirement registered for the resolver's own catalogued
/// coordinate, then any registered for `Query._entities`, as generated entity
/// resolvers check them. That needs the resolver's `Context` and a `Result`
/// return type; a resolver without them is refused instead of being described
/// and classified as protected while running unchecked.
fn enforce_entity_resolver_checks(
    method: &mut syn::ImplItemFn,
    authorization: &DeclaredAuthorization,
    kind_variant: &str,
    field_name: &str,
) -> syn::Result<()> {
    let context = method
        .sig
        .inputs
        .iter()
        .find_map(|argument| match argument {
            FnArg::Typed(argument) if is_context(&argument.ty) => match argument.pat.as_ref() {
                Pat::Ident(pat) => Some(pat.ident.clone()),
                _ => None,
            },
            _ => None,
        });
    let returns_result = match &method.sig.output {
        ReturnType::Type(_, ty) => !std::ptr::eq(unwrap_result_type(ty), ty.as_ref()),
        ReturnType::Default => false,
    };
    let (Some(context), true) = (context, returns_result) else {
        return Err(syn::Error::new_spanned(
            &method.sig,
            if authorization.is_required() {
                "this `#[graphql(entity)]` resolver declares authorization, which is enforced in \
                 its body: give it a named `&Context<'_>` argument and a `Result` return type, or \
                 declare it public with `#[graphql_orm(authorization = false)]`"
            } else {
                "a described `#[graphql(entity)]` resolver enforces the operation-assurance \
                 requirement registered for its coordinate in its body: give it a named \
                 `&Context<'_>` argument and a `Result` return type"
            },
        ));
    };
    let enforcement = if authorization.is_required() {
        authorization.enforcement_tokens(&context)
    } else {
        quote! {}
    };
    let kind = quote::format_ident!("{}", kind_variant);
    let block = &method.block;
    method.block = syn::parse_quote! {{
        #enforcement
        // The context field is `_entities`: enforce this resolver's own
        // coordinate, and any classification registered for `_entities`.
        ::graphql_orm::graphql::assurance::enforce_operation_assurance(
            #context,
            ::graphql_orm::graphql::orm::GraphqlOperationKind::#kind,
            #field_name,
        )?;
        ::graphql_orm::graphql::assurance::enforce_resolver_assurance(
            #context,
            ::graphql_orm::graphql::orm::GraphqlOperationKind::#kind,
        )?;
        #block
    }};
    Ok(())
}

/// Every token of a tree, groups included.
fn flatten_tokens(token: proc_macro2::TokenTree) -> Vec<proc_macro2::TokenTree> {
    match token {
        proc_macro2::TokenTree::Group(group) => group
            .stream()
            .into_iter()
            .flat_map(flatten_tokens)
            .collect(),
        token => vec![token],
    }
}

/// Reject a resolver with more than one `#[graphql(...)]` attribute.
///
/// async-graphql reads only the first, so a flag such as `skip` or `entity`, a
/// name or a guard in a later attribute would mean one thing to this macro and
/// another to the schema. This runs before any other attribute is read.
fn single_graphql_attribute(attrs: &[syn::Attribute]) -> syn::Result<()> {
    // A `graphql` attribute applied through `cfg_attr` is expanded after this
    // macro runs, so its `name`, `skip`, `entity` or `guard` would be unseen:
    // the catalogued and checked field name could differ from the served one.
    if let Some(conditional) = attrs.iter().find(|attr| {
        attr.path().is_ident("cfg_attr")
            && attr
                .meta
                .to_token_stream()
                .into_iter()
                .flat_map(flatten_tokens)
                .any(|token| matches!(&token, proc_macro2::TokenTree::Ident(ident) if ident == "graphql"))
    }) {
        return Err(syn::Error::new_spanned(
            conditional,
            "a `graphql` attribute applied through `cfg_attr` is not visible to this macro, so \
             the resolver's name, guard and flags could not be checked; write \
             `#[graphql(...)]` unconditionally and put the whole resolver under `#[cfg(...)]` \
             if it is conditional",
        ));
    }
    if let Some(extra) = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("graphql"))
        .nth(1)
    {
        return Err(syn::Error::new_spanned(
            extra,
            "a described custom root takes a single `#[graphql(...)]` attribute; \
             async-graphql reads only the first",
        ));
    }
    Ok(())
}

/// A boolean resolver flag as async-graphql reads it: `key` or `key = <bool>`.
fn has_graphql_flag(attrs: &[syn::Attribute], key: &str) -> syn::Result<bool> {
    let mut found = false;
    for meta in graphql_metas(attrs)? {
        if !meta.path().is_ident(key) {
            continue;
        }
        found = match &meta {
            Meta::Path(_) => true,
            Meta::NameValue(syn::MetaNameValue {
                value:
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Bool(value),
                        ..
                    }),
                ..
            }) => value.value,
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    format!("graphql {key} must be a flag or a boolean"),
                ));
            }
        };
    }
    Ok(found)
}

#[derive(Default)]
struct MethodSemanticOptions {
    description: Option<String>,
    result_classification: Option<String>,
    result_export: Option<String>,
    result_maximum_items: Option<u32>,
    result_type_kind: Option<String>,
    /// The resolver's own declaration, applied to its root's.
    authorization: Option<ResolverAuthorization>,
}

fn take_semantic_options(attrs: &mut Vec<syn::Attribute>) -> syn::Result<MethodSemanticOptions> {
    let mut options = MethodSemanticOptions::default();
    let mut authorization = None;
    let mut scopes = None;
    let mut replacement = None;
    let mut authorization_span = None;
    let mut retained = Vec::with_capacity(attrs.len());
    for attr in attrs.drain(..) {
        if attr.path().is_ident("graphql_orm") {
            attr.parse_nested_meta(|nested| {
                if nested.path.is_ident("description") {
                    let lit: syn::LitStr = nested.value()?.parse()?;
                    if options.description.replace(lit.value()).is_some() {
                        return Err(nested.error("duplicate semantic description"));
                    }
                    Ok(())
                } else if nested.path.is_ident("result_classification") {
                    let lit: syn::LitStr = nested.value()?.parse()?;
                    validate_semantic_classification(&lit.value(), lit.span())?;
                    if options.result_classification.replace(lit.value()).is_some() {
                        return Err(nested.error("duplicate result classification"));
                    }
                    Ok(())
                } else if nested.path.is_ident("result_export") {
                    let lit: syn::LitStr = nested.value()?.parse()?;
                    if !matches!(lit.value().as_str(), "exportable" | "never_export") {
                        return Err(
                            nested.error("result_export must be exportable or never_export")
                        );
                    }
                    if options.result_export.replace(lit.value()).is_some() {
                        return Err(nested.error("duplicate result export"));
                    }
                    Ok(())
                } else if nested.path.is_ident("result_maximum_items") {
                    let lit: syn::LitInt = nested.value()?.parse()?;
                    let maximum = lit.base10_parse::<u32>()?;
                    if maximum == 0 {
                        return Err(nested.error("result_maximum_items must be positive"));
                    }
                    if options.result_maximum_items.replace(maximum).is_some() {
                        return Err(nested.error("duplicate result maximum"));
                    }
                    Ok(())
                } else if nested.path.is_ident("result_type_kind") {
                    let lit: syn::LitStr = nested.value()?.parse()?;
                    if !matches!(lit.value().as_str(), "scalar" | "enum" | "object") {
                        return Err(
                            nested.error("result_type_kind must be scalar, enum, or object")
                        );
                    }
                    if options.result_type_kind.replace(lit.value()).is_some() {
                        return Err(nested.error("duplicate result type kind"));
                    }
                    Ok(())
                } else if nested.path.is_ident("authorization")
                    && nested.input.peek(syn::token::Paren)
                {
                    // `authorization(replace)` or
                    // `authorization(replace, all_scopes = [...])`: the only
                    // way a resolver can carry a different requirement than
                    // its root's without being public.
                    authorization_span.get_or_insert(nested.path.span());
                    let mut replace = false;
                    let mut replacing = None;
                    nested.parse_nested_meta(|option| {
                        if option.path.is_ident("replace") {
                            replace = true;
                            Ok(())
                        } else if option.path.is_ident("all_scopes")
                            || option.path.is_ident("any_scopes")
                        {
                            let any = option.path.is_ident("any_scopes");
                            let value: syn::Expr = option.value()?.parse()?;
                            if replacing
                                .replace(parse_scope_alternatives(&value, any)?)
                                .is_some()
                            {
                                return Err(
                                    option.error("declare one of all_scopes or any_scopes, once")
                                );
                            }
                            Ok(())
                        } else {
                            Err(option.error(
                                "expected `replace`, `all_scopes = [...]` or `any_scopes = [[...]]`",
                            ))
                        }
                    })?;
                    if !replace {
                        return Err(nested.error(
                            "`authorization(...)` replaces the root's requirement and must say \
                             so: write `authorization(replace, ...)`. To add scopes to the \
                             root's requirement, write `all_scopes = [...]` or \
                             `any_scopes = [[...]]` directly",
                        ));
                    }
                    if replacement.replace(replacing).is_some() {
                        return Err(nested.error("duplicate authorization"));
                    }
                    Ok(())
                } else if nested.path.is_ident("authorization") {
                    let lit: syn::LitBool = nested.value()?.parse()?;
                    authorization_span.get_or_insert(lit.span);
                    if authorization.replace(lit.value).is_some() {
                        return Err(nested.error("duplicate authorization"));
                    }
                    Ok(())
                } else if nested.path.is_ident("all_scopes") || nested.path.is_ident("any_scopes") {
                    let any = nested.path.is_ident("any_scopes");
                    let value: syn::Expr = nested.value()?.parse()?;
                    authorization_span.get_or_insert(nested.path.span());
                    if scopes
                        .replace(parse_scope_alternatives(&value, any)?)
                        .is_some()
                    {
                        return Err(nested.error("declare one of all_scopes or any_scopes, once"));
                    }
                    Ok(())
                } else {
                    Err(nested.error("unsupported custom-operation semantic attribute"))
                }
            })?;
        } else {
            retained.push(attr);
        }
    }
    *attrs = retained;
    let span = authorization_span.unwrap_or_else(proc_macro2::Span::mixed_site);
    options.authorization = match (replacement, authorization, scopes) {
        (None, None, None) => None,
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
            return Err(syn::Error::new(
                span,
                "`authorization(replace, ...)` states the resolver's whole requirement and \
                 cannot be combined with `authorization = ...`, `all_scopes` or `any_scopes`",
            ));
        }
        (Some(replacing), None, None) => Some(ResolverAuthorization::Replace(replacing)),
        (None, Some(false), Some(_)) => {
            return Err(syn::Error::new(
                span,
                "`authorization = false` declares a public resolver and cannot be combined \
                 with all_scopes or any_scopes",
            ));
        }
        (None, Some(false), None) => Some(ResolverAuthorization::Public),
        (None, _, scopes) => Some(ResolverAuthorization::Narrow(scopes)),
    };
    if options.result_classification.is_some() != options.result_export.is_some() {
        return Err(syn::Error::new(
            proc_macro2::Span::mixed_site(),
            "result_classification and result_export must be declared together",
        ));
    }
    if options.result_classification.as_deref() == Some("secret")
        && options.result_export.as_deref() == Some("exportable")
    {
        return Err(syn::Error::new(
            proc_macro2::Span::mixed_site(),
            "secret custom-operation results cannot be exportable",
        ));
    }
    Ok(options)
}

fn humanize(value: &str) -> String {
    let mut output = String::new();
    for (index, character) in value.chars().enumerate() {
        if index > 0 && character.is_uppercase() {
            output.push(' ');
        }
        if index == 0 {
            output.extend(character.to_uppercase());
        } else {
            output.push(character);
        }
    }
    output
}

fn unwrap_result_type(ty: &Type) -> &Type {
    let Type::Path(path) = ty else { return ty };
    let Some(segment) = path.path.segments.last() else {
        return ty;
    };
    if segment.ident != "Result" {
        return ty;
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return ty;
    };
    arguments
        .args
        .iter()
        .find_map(|argument| match argument {
            GenericArgument::Type(ty) => Some(ty),
            _ => None,
        })
        .unwrap_or(ty)
}

fn generic_type<'a>(ty: &'a Type, wrapper: &str) -> Option<&'a Type> {
    let Type::Path(path) = ty else { return None };
    let segment = path.path.segments.last()?;
    if segment.ident != wrapper {
        return None;
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    arguments.args.iter().find_map(|argument| match argument {
        GenericArgument::Type(ty) => Some(ty),
        _ => None,
    })
}

fn result_leaf_type(ty: &Type) -> &Type {
    generic_type(ty, "Option")
        .or_else(|| generic_type(ty, "Box"))
        .or_else(|| generic_type(ty, "Vec"))
        .map_or(ty, result_leaf_type)
}

fn result_is_list(ty: &Type) -> bool {
    if let Some(inner) = generic_type(ty, "Option").or_else(|| generic_type(ty, "Box")) {
        return result_is_list(inner);
    }
    generic_type(ty, "Vec").is_some()
}

fn is_known_scalar(ty: &Type) -> bool {
    let Type::Path(path) = ty else { return false };
    path.path.segments.last().is_some_and(|segment| {
        matches!(
            segment.ident.to_string().as_str(),
            "String"
                | "str"
                | "bool"
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
                | "f32"
                | "f64"
                | "Uuid"
                | "Decimal"
                | "Date"
                | "Time"
                | "DateTime"
                | "OffsetDateTime"
                | "Value"
        )
    })
}

fn subscription_item_type(ty: &Type) -> Option<&Type> {
    let Type::ImplTrait(implementation) = ty else {
        return None;
    };
    implementation.bounds.iter().find_map(|bound| {
        let syn::TypeParamBound::Trait(bound) = bound else {
            return None;
        };
        let segment = bound.path.segments.last()?;
        if segment.ident != "Stream" {
            return None;
        }
        let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
            return None;
        };
        arguments.args.iter().find_map(|argument| match argument {
            GenericArgument::AssocType(item) if item.ident == "Item" => Some(&item.ty),
            _ => None,
        })
    })
}

fn is_context(ty: &Type) -> bool {
    ty.to_token_stream().to_string().contains("Context")
}

pub(crate) fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = match syn::parse::<CustomRootArgs>(args) {
        Ok(args) => args,
        Err(error) => return error.to_compile_error().into(),
    };
    let mut item = match syn::parse::<ItemImpl>(input) {
        Ok(item) => item,
        Err(error) => return error.to_compile_error().into(),
    };
    let kind_variant = match args.kind.as_str() {
        "query" => "Query",
        "mutation" => "Mutation",
        _ => "Subscription",
    };
    // Every described root enforces its own assurance coordinate first.
    let assurance_guard = format!(
        "::graphql_orm::graphql::assurance::DeclaredAssuranceGuard::new(\
         ::graphql_orm::graphql::orm::GraphqlOperationKind::{kind_variant})"
    );
    let kind = match args.kind.as_str() {
        "query" => quote! { ::graphql_orm::graphql::orm::GraphqlOperationKind::Query },
        "mutation" => quote! { ::graphql_orm::graphql::orm::GraphqlOperationKind::Mutation },
        "subscription" => {
            quote! { ::graphql_orm::graphql::orm::GraphqlOperationKind::Subscription }
        }
        _ => unreachable!(),
    };
    let observation = args.observation.as_deref().map(|mode| {
        let mode = if mode == "best_effort" {
            quote! { ::graphql_orm::graphql::orm::GraphqlSubscriptionReplayMode::BestEffort }
        } else {
            quote! { ::graphql_orm::graphql::orm::GraphqlSubscriptionReplayMode::ReplayThenLive }
        };
        let maximum_duration_seconds = args.maximum_duration_seconds;
        let maximum_events = args.maximum_events;
        quote! {
            .with_subscription_observation(
                ::graphql_orm::graphql::orm::GraphqlSubscriptionObservationDescriptor {
                    replay_mode: #mode,
                    maximum_duration_seconds: Some(#maximum_duration_seconds),
                    maximum_events: Some(#maximum_events),
                    condition_fields: Vec::new(),
                }
            ).expect("custom subscription observation metadata must validate")
        }
    });
    let ai_execution = args.ai_execution.as_deref().map(|policy| {
        let policy = match policy {
            "automatic" => quote! {
                ::graphql_orm::graphql::orm::AiMutationExecutionPolicy::Automatic
            },
            "approval_required" => quote! {
                ::graphql_orm::graphql::orm::AiMutationExecutionPolicy::ApprovalRequired
            },
            "prohibited" => quote! {
                ::graphql_orm::graphql::orm::AiMutationExecutionPolicy::Prohibited
            },
            _ => unreachable!(),
        };
        quote! {
            .with_ai_mutation_execution(#policy)
            .expect("custom mutation AI execution metadata must validate")
        }
    });
    let root_guard = match root_attribute_guard(&item, &args.kind) {
        Ok(guard) => guard,
        Err(error) => return error.to_compile_error().into(),
    };
    let self_ty = item.self_ty.clone();
    let mut descriptors = Vec::new();
    let mut watched_streams = Vec::new();
    let mut root_fields = Vec::new();
    let mut field_names = std::collections::BTreeSet::new();
    let mut result_type_names = std::collections::BTreeSet::new();
    let mut result_types = Vec::<Type>::new();

    for impl_item in &mut item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        if let Err(error) = single_graphql_attribute(&method.attrs)
            .and_then(|()| reject_unmodelled_resolver_flags(&method.attrs))
        {
            return error.to_compile_error().into();
        }
        match has_graphql_flag(&method.attrs, "skip") {
            Ok(true) => continue,
            Ok(false) => {}
            Err(error) => return error.to_compile_error().into(),
        }
        let semantic_options = match take_semantic_options(&mut method.attrs) {
            Ok(options) => options,
            Err(error) => return error.to_compile_error().into(),
        };
        let entity = match has_graphql_flag(&method.attrs, "entity") {
            Ok(entity) => entity,
            Err(error) => return error.to_compile_error().into(),
        };
        // A resolver narrows its root's declaration unless it says it
        // replaces it or is public.
        let authorization = ResolverAuthorization::apply(
            semantic_options.authorization.as_ref(),
            &args.authorization,
        );
        // Authentication and scopes first, then assurance, as generated
        // operations check them.
        let generated_guards = authorization
            .guard_sources()
            .into_iter()
            .chain([assurance_guard.clone()])
            .collect::<Vec<_>>();
        let method_name = method.sig.ident.unraw().to_string();
        let field_name = match graphql_attribute_value(&method.attrs, "name") {
            Ok(Some(name)) => name,
            Ok(None) => crate::naming::async_graphql_rename(&method_name, None),
            Err(error) => return error.to_compile_error().into(),
        };
        let installed = if entity {
            enforce_entity_resolver_checks(method, &authorization, kind_variant, &field_name)
                .map(|()| String::new())
        } else {
            install_generated_guards(method, &generated_guards, root_guard.as_deref())
        };
        let installed_guard = match installed {
            Ok(guard) => guard,
            Err(error) => return error.to_compile_error().into(),
        };
        let explicit = semantic_options.description.clone();
        if !field_names.insert(field_name.clone()) {
            return syn::Error::new_spanned(
                &method.sig.ident,
                "duplicate custom semantic GraphQL root field name",
            )
            .to_compile_error()
            .into();
        }
        // An entity resolver is served below `_entities`, not as a root field.
        if !entity {
            root_fields.push(RootField {
                cfg: cfg_attributes(&method.attrs),
                name: field_name.clone(),
            });
        }
        let documented = match semantic_doc_description(&method.attrs) {
            Ok(description) => description,
            Err(error) => return error.to_compile_error().into(),
        };
        let explicit_description = explicit.is_some();
        let description = explicit
            .or(documented)
            .unwrap_or_else(|| humanize(&method_name));
        if let Err(error) = validate_semantic_description(&description, method.sig.ident.span()) {
            return error.to_compile_error().into();
        }
        if explicit_description {
            method
                .attrs
                .retain(|attribute| !attribute.path().is_ident("doc"));
        }
        if explicit_description
            || semantic_doc_description(&method.attrs)
                .ok()
                .flatten()
                .is_none()
        {
            let doc = syn::LitStr::new(&description, method.sig.ident.span());
            method.attrs.push(syn::parse_quote!(#[doc = #doc]));
        }

        let mut arguments = Vec::new();
        for argument in &mut method.sig.inputs {
            let FnArg::Typed(argument) = argument else {
                continue;
            };
            if is_context(&argument.ty) {
                continue;
            }
            let Pat::Ident(pat) = argument.pat.as_ref() else {
                return syn::Error::new_spanned(
                    &argument.pat,
                    "semantic resolver arguments require identifiers",
                )
                .to_compile_error()
                .into();
            };
            let rust_name = pat.ident.unraw().to_string();
            let graphql_name = match graphql_attribute_value(&argument.attrs, "name") {
                Ok(Some(name)) => name,
                Ok(None) => crate::naming::async_graphql_rename(&rust_name, None),
                Err(error) => return error.to_compile_error().into(),
            };
            let (argument_description, explicit_argument_description) =
                match graphql_attribute_value(&argument.attrs, "desc") {
                    Ok(Some(description)) => (description, true),
                    Ok(None) => (humanize(&rust_name), false),
                    Err(error) => return error.to_compile_error().into(),
                };
            if let Err(error) =
                validate_semantic_description(&argument_description, pat.ident.span())
            {
                return error.to_compile_error().into();
            }
            if !explicit_argument_description {
                let description = syn::LitStr::new(&argument_description, pat.ident.span());
                if let Err(error) =
                    merge_graphql_argument_description(&mut argument.attrs, description)
                {
                    return error.to_compile_error().into();
                }
            }
            let argument_type = &argument.ty;
            arguments.push(quote! {
                ::graphql_orm::graphql::orm::GraphqlSemanticArgumentDescriptor {
                    graphql_name: #graphql_name.to_owned(),
                    description: #argument_description.to_owned(),
                    type_ref: ::graphql_orm::graphql::orm::parse_graphql_type(
                        &<#argument_type as ::graphql_orm::async_graphql::InputType>::qualified_type_name(),
                    ).expect("custom resolver argument type must be valid GraphQL"),
                }
            });
        }
        let output = match &method.sig.output {
            ReturnType::Type(_, ty) => unwrap_result_type(ty),
            ReturnType::Default => {
                return syn::Error::new_spanned(
                    &method.sig,
                    "semantic resolvers require an output type",
                )
                .to_compile_error()
                .into();
            }
        };
        let output = if args.kind == "subscription" {
            let Some(output) = subscription_item_type(output) else {
                return syn::Error::new_spanned(
                    output,
                    "semantic subscription resolvers require `impl Stream<Item = T>` so the event type is explicit",
                )
                .to_compile_error()
                .into();
            };
            output
        } else {
            output
        };
        let leaf_type = result_leaf_type(output);
        let result_kind = semantic_options
            .result_type_kind
            .as_deref()
            .unwrap_or_else(|| {
                if is_known_scalar(leaf_type) {
                    "scalar"
                } else {
                    "object"
                }
            });
        let scalar_or_enum = matches!(result_kind, "scalar" | "enum");
        let list_result = result_is_list(output);
        if semantic_options.result_maximum_items.is_some() && (!scalar_or_enum || !list_result) {
            return syn::Error::new_spanned(
                output,
                "result_maximum_items is valid only for a scalar or enum list result",
            )
            .to_compile_error()
            .into();
        }
        if scalar_or_enum
            && list_result
            && semantic_options.result_export.as_deref() == Some("exportable")
            && semantic_options.result_maximum_items.is_none()
        {
            return syn::Error::new_spanned(
                output,
                "exportable scalar or enum list results require result_maximum_items",
            )
            .to_compile_error()
            .into();
        }
        if result_kind == "object" {
            let identity = leaf_type.to_token_stream().to_string();
            if result_type_names.insert(identity) {
                result_types.push(leaf_type.clone());
            }
        }
        let result_kind_override = semantic_options
            .result_type_kind
            .as_deref()
            .and_then(|kind| {
                let kind = match kind {
                    "scalar" => quote! {
                        ::graphql_orm::graphql::orm::GraphqlSemanticTypeKind::Scalar
                    },
                    "enum" => quote! {
                        ::graphql_orm::graphql::orm::GraphqlSemanticTypeKind::Enum
                    },
                    "object" => return None,
                    _ => unreachable!(),
                };
                Some(quote! {
                    .with_custom_result_leaf_kind(#kind)
                    .expect("custom resolver result kind must validate")
                })
            });
        let result_disclosure = if semantic_options.result_classification.is_some()
            || semantic_options.result_maximum_items.is_some()
        {
            let classification = match semantic_options.result_classification.as_deref() {
                Some(classification) => {
                    match semantic_classification_tokens(classification, method.sig.ident.span()) {
                        Ok(tokens) => tokens,
                        Err(error) => return error.to_compile_error().into(),
                    }
                }
                None => quote! {
                    ::graphql_orm::graphql::orm::GraphqlSemanticClassification::Secret
                },
            };
            let export = match semantic_options.result_export.as_deref() {
                Some("exportable") => quote! {
                    ::graphql_orm::graphql::orm::GraphqlSemanticExport::Exportable
                },
                Some("never_export") | None => quote! {
                    ::graphql_orm::graphql::orm::GraphqlSemanticExport::NeverExport
                },
                Some(_) => unreachable!(),
            };
            let maximum = semantic_options.result_maximum_items.map(|maximum| {
                quote! { .with_maximum_items(#maximum) }
            });
            Some(quote! {
                .with_result_disclosure(
                    ::graphql_orm::graphql::orm::GraphqlSemanticResultDisclosure::new(
                        #classification,
                        #export,
                    )
                    #maximum
                )
                .expect("custom resolver result disclosure must validate")
            })
        } else {
            None
        };
        let has_authorization_policy = authorization.is_required();
        descriptors.push(quote! {
            ::graphql_orm::graphql::orm::GraphqlSemanticOperationDescriptor::custom(
                #kind,
                #field_name,
                #description,
                vec![#(#arguments),*],
                ::graphql_orm::graphql::orm::parse_graphql_type(
                    &<#output as ::graphql_orm::async_graphql::OutputType>::qualified_type_name(),
                ).expect("custom resolver result type must be valid GraphQL"),
                #has_authorization_policy,
            ).expect("custom resolver semantic metadata must validate")
            #result_kind_override
            #result_disclosure
            #ai_execution
            #observation
        });
        if args.kind == "subscription" && !entity {
            match watch_subscription_stream(method, &generated_guards, &installed_guard) {
                Ok(inner) => watched_streams.push(inner),
                Err(error) => return error.to_compile_error().into(),
            }
        }
    }

    let watched_streams = if watched_streams.is_empty() {
        quote! {}
    } else {
        let self_ty = &item.self_ty;
        let (impl_generics, _, where_clause) = item.generics.split_for_impl();
        quote! {
            impl #impl_generics #self_ty #where_clause {
                #(#watched_streams)*
            }
        }
    };
    let root_field_names = root_field_names_impl(&item, &root_fields);
    quote! {
        #item

        #root_field_names

        #watched_streams

        impl ::graphql_orm::graphql::orm::GraphqlCustomOperationMetadata for #self_ty {
            fn graphql_custom_operations(
            ) -> &'static [::graphql_orm::graphql::orm::GraphqlSemanticOperationDescriptor] {
                static OPERATIONS: ::std::sync::OnceLock<
                    Box<[::graphql_orm::graphql::orm::GraphqlSemanticOperationDescriptor]>
                > = ::std::sync::OnceLock::new();
                OPERATIONS.get_or_init(|| vec![#(#descriptors),*].into_boxed_slice())
            }

            fn graphql_custom_result_types(
            ) -> &'static [::graphql_orm::graphql::orm::GraphqlEntitySemanticMetadata] {
                static TYPES: ::std::sync::OnceLock<
                    Box<[::graphql_orm::graphql::orm::GraphqlEntitySemanticMetadata]>
                > = ::std::sync::OnceLock::new();
                TYPES.get_or_init(|| vec![
                    #(
                        <#result_types as
                            ::graphql_orm::graphql::orm::GraphqlSemanticResultTypeMetadata>
                            ::graphql_semantic_result_type()
                            .clone()
                    ),*
                ].into_boxed_slice())
            }
        }
    }
    .into()
}

/// One root field a handwritten root exposes, under the `cfg` of its resolver.
struct RootField {
    cfg: Vec<syn::Attribute>,
    name: String,
}

fn cfg_attributes(attrs: &[syn::Attribute]) -> Vec<syn::Attribute> {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .cloned()
        .collect()
}

/// Declare a handwritten root's field names for the `schema_roots!` collision
/// check.
fn root_field_names_impl(item: &ItemImpl, fields: &[RootField]) -> proc_macro2::TokenStream {
    let self_ty = &item.self_ty;
    let (impl_generics, _, where_clause) = item.generics.split_for_impl();
    let names = fields.iter().map(|field| {
        let cfg = &field.cfg;
        let name = &field.name;
        quote! { #(#cfg)* #name }
    });
    quote! {
        impl #impl_generics ::graphql_orm::graphql::orm::GraphqlRootFieldNames
            for #self_ty #where_clause
        {
            const ROOT_FIELD_NAMES: &'static [&'static str] = &[#(#names),*];
        }
    }
}

/// The `rename_fields` rule of a root's `#[Object(...)]` or
/// `#[Subscription(...)]` attribute.
fn root_rename_rule(item: &ItemImpl) -> syn::Result<Option<String>> {
    let mut roots = item
        .attrs
        .iter()
        .filter(|attr| root_attribute_kind(attr.path()).is_some());
    let Some(attr) = roots.next() else {
        return Err(syn::Error::new(
            item.impl_token.span,
            "`#[graphql_orm_root_fields]` must be followed by `#[async_graphql::Object]` or \
             `#[async_graphql::Subscription]` (or `#[Object]`, `#[Subscription]`, \
             `#[graphql_orm::async_graphql::Object]`); renamed imports are not recognised",
        ));
    };
    if let Some(extra) = roots.next() {
        return Err(syn::Error::new_spanned(
            extra,
            "a root takes a single async-graphql root attribute",
        ));
    }
    let Meta::List(list) = &attr.meta else {
        return Ok(None);
    };
    for meta in list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)? {
        if let Meta::NameValue(value) = &meta
            && value.path.is_ident("rename_fields")
        {
            let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(rule),
                ..
            }) = ungrouped(&value.value)
            else {
                return Err(syn::Error::new_spanned(
                    &value.value,
                    "rename_fields must be a string",
                ));
            };
            return Ok(Some(rule.value()));
        }
    }
    Ok(None)
}

/// Names a `derived(name = "...")` flag adds beside its resolver.
fn derived_field_names(attrs: &[syn::Attribute]) -> syn::Result<Vec<String>> {
    let mut names = Vec::new();
    for meta in graphql_metas(attrs)? {
        let Meta::List(list) = &meta else { continue };
        if !list.path.is_ident("derived") {
            continue;
        }
        for nested in list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)? {
            if let Meta::NameValue(value) = &nested
                && value.path.is_ident("name")
                && let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(name),
                    ..
                }) = &value.value
            {
                names.push(name.value());
            }
        }
    }
    Ok(names)
}

/// `#[graphql_orm_root_fields]`: record the root field names of a handwritten
/// `Object` or `Subscription` impl without describing or guarding it.
///
/// Names follow async-graphql: an explicit `#[graphql(name = "...")]`, else
/// the root's `rename_fields` rule (camel case by default) applied to the
/// method name. `skip` and `entity` resolvers contribute no root field, and
/// `derived(name = "...")` contributes its extra name. `flatten` is refused
/// because the flattened object's field names are not visible here.
pub(crate) fn expand_root_fields(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "`#[graphql_orm_root_fields]` takes no arguments",
        )
        .to_compile_error()
        .into();
    }
    let item = match syn::parse::<ItemImpl>(input) {
        Ok(item) => item,
        Err(error) => return error.to_compile_error().into(),
    };
    let fields = (|| -> syn::Result<Vec<RootField>> {
        let rule = root_rename_rule(&item)?;
        let mut fields = Vec::new();
        for impl_item in &item.items {
            let ImplItem::Fn(method) = impl_item else {
                continue;
            };
            single_graphql_attribute(&method.attrs)?;
            if has_graphql_flag(&method.attrs, "skip")?
                || has_graphql_flag(&method.attrs, "entity")?
            {
                continue;
            }
            if has_graphql_flag(&method.attrs, "flatten")? {
                return Err(syn::Error::new_spanned(
                    &method.sig.ident,
                    "`#[graphql_orm_root_fields]` cannot name the fields of a \
                     `#[graphql(flatten)]` resolver; declare them as resolvers of this root",
                ));
            }
            let method_name = method.sig.ident.unraw().to_string();
            let name = match graphql_attribute_value(&method.attrs, "name")? {
                Some(name) => name,
                None => crate::naming::async_graphql_rename(&method_name, rule.as_deref()),
            };
            let cfg = cfg_attributes(&method.attrs);
            for name in std::iter::once(name).chain(derived_field_names(&method.attrs)?) {
                fields.push(RootField {
                    cfg: cfg.clone(),
                    name,
                });
            }
        }
        Ok(fields)
    })();
    let fields = match fields {
        Ok(fields) => fields,
        Err(error) => return error.to_compile_error().into(),
    };
    let root_field_names = root_field_names_impl(&item, &fields);
    quote! {
        #item

        #root_field_names
    }
    .into()
}
