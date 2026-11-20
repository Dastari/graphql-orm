//! Handwritten resolver fields on a generated entity object.
//!
//! `GraphQLEntity` emits one closed `#[Object] impl` per entity, and a second
//! macro cannot add methods to it. With `#[graphql_orm(object_fields)]` the
//! derive therefore emits a hidden `macro_rules!` whose body holds that impl,
//! generated methods included, with a hole for more methods.
//! `#[graphql_object_fields]` hands the handwritten impl to that macro, which
//! passes it to [`wrap`] and receives the wrapped methods back for the hole.
//!
//! [`wrap`] never sees the generated methods and has no input it leaves
//! unwrapped: every resolver it is given gets the field check a generated
//! getter runs in front of its body, has its name checked against the
//! generated ones, and is described to the entity's metadata.

use crate::composed_identity::{flag, string_literal};
use crate::entity::{humanize_identifier, semantic_doc_description};
use crate::naming::derived_field_name;
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use std::collections::BTreeSet;
use syn::ext::IdentExt as _;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned as _;
use syn::visit_mut::VisitMut;
use syn::{FnArg, ImplItem, ItemImpl, LitStr, Meta, Pat, ReturnType, Token, Type};

fn callback_ident(entity: &syn::Ident) -> syn::Ident {
    format_ident!(
        "__gom_object_fields_{}",
        entity.unraw(),
        span = entity.span()
    )
}

/// What `GraphQLEntity` emits in place of its `#[Object] impl` for an entity
/// that hosts handwritten fields.
#[allow(clippy::too_many_arguments)]
pub(crate) fn host_tokens(
    struct_name: &syn::Ident,
    description: &str,
    entity_name: &str,
    backend_marker: &TokenStream,
    graphql_rename_fields: Option<&str>,
    serde_rename_all: Option<&str>,
    generated_names: &[String],
    generated_methods: &[TokenStream],
) -> TokenStream {
    let callback = callback_ident(struct_name);
    let graphql_rename_fields = graphql_rename_fields.into_iter();
    let serde_rename_all = serde_rename_all.into_iter();
    quote! {
        #[doc(hidden)]
        #[allow(unused_macros)]
        macro_rules! #callback {
            // The entity's one GraphQL object. `__graphql_object_fields_wrap!`
            // expands to this arm, with the methods it wrapped.
            (
                @object
                { $($entity:tt)* }
                { $($attributes:tt)* }
                { $($definitions:tt)* }
                $($wrapped:tt)*
            ) => {
                #[doc = #description]
                $($attributes)*
                #[::graphql_orm::async_graphql::Object]
                impl $($entity)* {
                    #(#generated_methods)*
                    $($wrapped)*
                }

                impl ::graphql_orm::graphql::orm::HandwrittenObjectFields for $($entity)* {
                    const FIELDS: &'static [::graphql_orm::graphql::orm::ComputedObjectFieldDef] =
                        &[$($definitions)*];
                }
            };
            // From `#[graphql_object_fields]`: this macro's path, then the impl.
            ({ $($callback:tt)* } $($handwritten:tt)*) => {
                ::graphql_orm::__graphql_object_fields_wrap! {
                    #entity_name
                    { #backend_marker }
                    { #(#graphql_rename_fields)* }
                    { #(#serde_rename_all)* }
                    [ #(#generated_names),* ]
                    { $($callback)* }
                    $($handwritten)*
                }
            };
        }
        // Lets the handwritten impl precede the entity, or name it by path.
        #[doc(hidden)]
        #[allow(unused_imports)]
        pub(crate) use #callback;

        impl ::graphql_orm::graphql::orm::GeneratedObjectFieldsHost for #struct_name {}
    }
}

/// Refuse every attribute of the handwritten impl that is not inert.
///
/// The impl's attributes are placed on the generated `#[Object] impl`, ahead
/// of `#[Object]`. An attribute macro there would run on the finished impl,
/// after every resolver was wrapped, and could add a resolver without the
/// field check. Only lint levels and a whole-impl `cfg` are forwarded; a doc
/// comment is dropped, and `cfg_attr` is refused because what it expands to
/// is not known here.
fn inert_impl_attributes(item_impl: &ItemImpl) -> syn::Result<()> {
    for attribute in &item_impl.attrs {
        let path = attribute.path();
        if ["doc", "allow", "warn", "deny", "forbid", "expect", "cfg"]
            .iter()
            .any(|inert| path.is_ident(inert))
        {
            continue;
        }
        return Err(syn::Error::new_spanned(
            attribute,
            "graphql_object_fields accepts only `allow`, `warn`, `deny`, `forbid`, `expect` and `cfg` beside it on the impl: any other attribute would run on the generated object after its resolvers were given their field check",
        ));
    }
    Ok(())
}

/// `#[graphql_object_fields]`: hand the impl to the entity's callback macro.
pub(crate) fn attribute(args: TokenStream, mut item_impl: ItemImpl) -> syn::Result<TokenStream> {
    if item_impl.trait_.is_some() {
        return Err(syn::Error::new_spanned(
            &item_impl,
            "graphql_object_fields requires an inherent impl",
        ));
    }
    if !item_impl.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_impl.generics,
            "graphql_object_fields does not support generic entity impls",
        ));
    }
    let self_ty = item_impl.self_ty.clone();
    let Type::Path(path) = self_ty.as_ref() else {
        return Err(syn::Error::new_spanned(
            self_ty,
            "graphql_object_fields requires the entity type, named by identifier or path",
        ));
    };
    if path.qself.is_some() {
        return Err(syn::Error::new_spanned(
            self_ty,
            "graphql_object_fields requires the entity type, named by identifier or path",
        ));
    }
    // The callback sits beside the entity, so it is reached by the same path.
    let mut callback = path.path.clone();
    let last = callback
        .segments
        .last_mut()
        .expect("a type path has a segment");
    if !last.arguments.is_empty() {
        return Err(syn::Error::new_spanned(
            self_ty,
            "graphql_object_fields does not support generic entity impls",
        ));
    }
    last.ident = callback_ident(&last.ident);
    // Reported beside an object without handwritten fields, so that the one
    // mistake is the only error.
    let refusal = if !args.is_empty() {
        Some(syn::Error::new_spanned(
            args,
            "graphql_object_fields takes no arguments; the generated object keeps the entity's own naming and description",
        ))
    } else {
        inert_impl_attributes(&item_impl).err()
    };
    let unsupported_arguments = match refusal {
        Some(error) => {
            item_impl.items.clear();
            item_impl.attrs.clear();
            error.to_compile_error()
        }
        None => quote! {},
    };
    let self_ty = &item_impl.self_ty;
    Ok(quote! {
        #unsupported_arguments
        const _: () = {
            const fn assert_object_fields_host<
                T: ::graphql_orm::graphql::orm::GeneratedObjectFieldsHost,
            >() {}
            assert_object_fields_host::<#self_ty>();
        };
        #callback! { { #callback } #item_impl }
    })
}

struct Wrapping {
    entity_name: LitStr,
    backend: TokenStream,
    graphql_rename_fields: Option<LitStr>,
    serde_rename_all: Option<LitStr>,
    generated_names: Vec<LitStr>,
    callback: syn::Path,
    handwritten: ItemImpl,
}

impl Parse for Wrapping {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let entity_name = input.parse()?;
        let backend;
        syn::braced!(backend in input);
        let graphql_rename_fields;
        syn::braced!(graphql_rename_fields in input);
        let serde_rename_all;
        syn::braced!(serde_rename_all in input);
        let generated_names;
        syn::bracketed!(generated_names in input);
        let callback;
        syn::braced!(callback in input);
        Ok(Self {
            entity_name,
            backend: backend.parse()?,
            graphql_rename_fields: graphql_rename_fields.parse()?,
            serde_rename_all: serde_rename_all.parse()?,
            generated_names: Punctuated::<LitStr, Token![,]>::parse_terminated(&generated_names)?
                .into_iter()
                .collect(),
            callback: callback.parse()?,
            // One impl and nothing else: there is no position for a method
            // that would reach the object without the field check.
            handwritten: input.parse()?,
        })
    }
}

/// What a handwritten method's attributes declare.
#[derive(Default)]
struct FieldOptions {
    name: Option<String>,
    skipped: bool,
    read_policy: Option<String>,
    subscribe: bool,
}

/// Read a method's `#[graphql(...)]` and `#[graphql_orm(...)]` attributes, and
/// remove the latter: only this macro understands them.
fn field_options(method: &mut syn::ImplItemFn) -> syn::Result<FieldOptions> {
    let mut options = FieldOptions::default();
    for attribute in &method.attrs {
        if attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr") {
            return Err(syn::Error::new_spanned(
                attribute,
                "graphql_object_fields does not support conditional compilation of a single field; put the `cfg` on the whole impl",
            ));
        }
        if attribute.path().is_ident("graphql") {
            for option in
                attribute.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?
            {
                options.skipped |= flag(&option, "skip")?;
                for unsupported in ["flatten", "entity", "derived"] {
                    if option.path().is_ident(unsupported) {
                        return Err(syn::Error::new_spanned(
                            &option,
                            format!(
                                "graphql_object_fields does not support `{unsupported}`: every handwritten field is one named resolver with its own field read decision"
                            ),
                        ));
                    }
                }
                if let Meta::NameValue(value) = &option
                    && value.path.is_ident("name")
                {
                    options.name = Some(string_literal(&value.value, "name")?);
                }
            }
        } else if attribute.path().is_ident("graphql_orm") {
            attribute.parse_nested_meta(|meta| {
                if meta.path.is_ident("read_policy") {
                    let key: LitStr = meta.value()?.parse()?;
                    if key.value().is_empty() {
                        return Err(syn::Error::new_spanned(
                            key,
                            "read_policy requires a policy key",
                        ));
                    }
                    options.read_policy = Some(key.value());
                    Ok(())
                } else if meta.path.is_ident("subscribe") {
                    options.subscribe = if meta.input.peek(Token![=]) {
                        meta.value()?.parse::<syn::LitBool>()?.value
                    } else {
                        true
                    };
                    Ok(())
                } else {
                    Err(meta.error(
                        "unsupported handwritten object field option; expected read_policy or subscribe",
                    ))
                }
            })?;
        }
    }
    method
        .attrs
        .retain(|attribute| !attribute.path().is_ident("graphql_orm"));
    Ok(options)
}

/// The value type of a resolver that returns `Result<T>` or `FieldResult<T>`.
fn resolved_type(method: &syn::ImplItemFn) -> syn::Result<Type> {
    let refusal = || {
        syn::Error::new(
            match &method.sig.output {
                ReturnType::Type(_, ty) => ty.span(),
                ReturnType::Default => method.sig.ident.span(),
            },
            "a handwritten object field must return `async_graphql::Result<T>`, so that a refused read is a field error",
        )
    };
    let ReturnType::Type(_, ty) = &method.sig.output else {
        return Err(refusal());
    };
    let Type::Path(path) = ty.as_ref() else {
        return Err(refusal());
    };
    let last = path.path.segments.last().ok_or_else(refusal)?;
    if last.ident != "Result" && last.ident != "FieldResult" {
        return Err(refusal());
    }
    let syn::PathArguments::AngleBracketed(arguments) = &last.arguments else {
        return Err(refusal());
    };
    let Some(syn::GenericArgument::Type(value)) = arguments.args.first() else {
        return Err(refusal());
    };
    // The type is named again in a constant, where no lifetime may be elided.
    struct Static;
    impl VisitMut for Static {
        fn visit_lifetime_mut(&mut self, lifetime: &mut syn::Lifetime) {
            *lifetime = syn::Lifetime::new("'static", lifetime.span());
        }
        fn visit_type_reference_mut(&mut self, reference: &mut syn::TypeReference) {
            if reference.lifetime.is_none() {
                reference.lifetime = Some(syn::Lifetime::new("'static", reference.span()));
            }
            syn::visit_mut::visit_type_reference_mut(self, reference);
        }
    }
    let mut value = value.clone();
    Static.visit_type_mut(&mut value);
    Ok(value)
}

/// The identifier of the resolver's `&Context<'_>` parameter, added if absent.
fn context_ident(method: &mut syn::ImplItemFn) -> syn::Result<syn::Ident> {
    let generated = syn::Ident::new("__gom_ctx", Span::call_site());
    for argument in method.sig.inputs.iter_mut() {
        let FnArg::Typed(argument) = argument else {
            continue;
        };
        let Type::Reference(reference) = argument.ty.as_ref() else {
            continue;
        };
        let Type::Path(path) = reference.elem.as_ref() else {
            continue;
        };
        if !path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Context")
        {
            continue;
        }
        return match argument.pat.as_mut() {
            Pat::Ident(pattern) => Ok(pattern.ident.clone()),
            pattern @ Pat::Wild(_) => {
                *pattern = syn::parse_quote!(#generated);
                Ok(generated)
            }
            other => Err(syn::Error::new_spanned(
                other,
                "name the `Context` parameter of a handwritten object field with an identifier",
            )),
        };
    }
    method.sig.inputs.insert(
        1,
        syn::parse_quote!(#generated: &::graphql_orm::async_graphql::Context<'_>),
    );
    Ok(generated)
}

/// `__graphql_object_fields_wrap!`: wrap every handwritten resolver and hand
/// the result to the entity's callback macro, which emits the object.
pub(crate) fn wrap(input: TokenStream) -> syn::Result<TokenStream> {
    let Wrapping {
        entity_name,
        backend,
        graphql_rename_fields,
        serde_rename_all,
        generated_names,
        callback,
        mut handwritten,
    } = syn::parse2(input)?;
    let entity = entity_name.value();
    let graphql_rename_fields = graphql_rename_fields.map(|rule| rule.value());
    let serde_rename_all = serde_rename_all.map(|rule| rule.value());
    let generated_names: BTreeSet<String> = generated_names.iter().map(LitStr::value).collect();
    let self_ty = handwritten.self_ty.clone();

    // A refused handwritten field is reported beside the generated object
    // alone, so that the one mistake is the only error.
    // The attribute checks the impl's attributes first; a direct call of this
    // macro is held to the same rule.
    let (definitions, refusal) = match inert_impl_attributes(&handwritten).and_then(|()| {
        handwritten_fields(
            &mut handwritten,
            &entity,
            &backend,
            graphql_rename_fields.as_deref(),
            serde_rename_all.as_deref(),
            &generated_names,
        )
    }) {
        Ok(definitions) => (definitions, quote! {}),
        Err(error) => {
            handwritten.items.clear();
            handwritten.attrs.clear();
            (Vec::new(), error.to_compile_error())
        }
    };

    // The type description is the entity's; a doc comment here would replace it.
    let attributes = handwritten
        .attrs
        .iter()
        .filter(|attribute| !attribute.path().is_ident("doc"));
    let items = &handwritten.items;
    Ok(quote! {
        #refusal

        #callback! {
            @object
            { #self_ty }
            { #(#attributes)* }
            { #(#definitions),* }
            #(#items)*
        }
    })
}

/// Put the field check in front of every handwritten resolver, and
/// return the field definitions in declaration order.
fn handwritten_fields(
    handwritten: &mut ItemImpl,
    entity: &str,
    backend: &TokenStream,
    graphql_rename_fields: Option<&str>,
    serde_rename_all: Option<&str>,
    generated_names: &BTreeSet<String>,
) -> syn::Result<Vec<TokenStream>> {
    let mut handwritten_names = BTreeSet::new();
    let mut definitions = Vec::new();
    for item in handwritten.items.iter_mut() {
        let ImplItem::Fn(method) = item else {
            continue;
        };
        let options = field_options(method)?;
        if options.skipped {
            if options.read_policy.is_some() || options.subscribe {
                return Err(syn::Error::new_spanned(
                    &method.sig.ident,
                    "a skipped method is not a field, so it has no read policy or subscription availability",
                ));
            }
            continue;
        }
        if method.sig.asyncness.is_none() {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "a handwritten object field must be an `async fn`; mark a helper method `#[graphql(skip)]`",
            ));
        }
        // The handwritten `self`, so the inserted check names the same value.
        let record = match method.sig.inputs.first() {
            Some(FnArg::Receiver(receiver))
                if receiver.reference.is_some() && receiver.mutability.is_none() =>
            {
                receiver.self_token
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    &method.sig,
                    "a handwritten object field must take `&self`: its field read decision is asked for the loaded row before it resolves",
                ));
            }
        };
        let rust_name = method.sig.ident.unraw().to_string();
        let api_name = options.name.clone().unwrap_or_else(|| {
            derived_field_name(&rust_name, graphql_rename_fields, serde_rename_all)
        });
        if generated_names.contains(&api_name) {
            return Err(syn::Error::new_spanned(
                &method.sig.ident,
                format!(
                    "handwritten field `{api_name}` collides with the generated field `{api_name}` of `{entity}`; rename one of them"
                ),
            ));
        }
        if !handwritten_names.insert(api_name.clone()) {
            return Err(syn::Error::new_spanned(
                &method.sig.ident,
                format!("`{entity}` declares the handwritten field `{api_name}` more than once"),
            ));
        }
        let value_type = resolved_type(method)?;
        let field_description = match semantic_doc_description(&method.attrs)? {
            Some(description) => description,
            None => {
                // Served and described alike, as for a generated field.
                let description = humanize_identifier(&rust_name);
                method.attrs.push(syn::parse_quote!(#[doc = #description]));
                description
            }
        };
        if options.name.is_none() {
            method
                .attrs
                .push(syn::parse_quote!(#[graphql(name = #api_name)]));
        }
        let context = context_ident(method)?;
        let index = definitions.len();
        let body = &method.block;
        // A refused `Option<T>` field is an explicit null beside its error.
        // Any other field can only return the error.
        let nullable = matches!(
            &value_type,
            Type::Path(path) if path.qself.is_none()
                && path.path.segments.last().is_some_and(|segment| segment.ident == "Option")
        );
        let refuse = if nullable {
            quote! {
                ::graphql_orm::graphql::orm::report_computed_object_field_refusal(
                    #context,
                    __gom_denied,
                );
                return ::std::result::Result::Ok(::std::option::Option::None);
            }
        } else {
            quote! {
                return ::std::result::Result::Err(::std::convert::Into::into(__gom_denied));
            }
        };
        // Nothing of the handwritten body runs before the field decision.
        method.block = syn::parse_quote!({
            if let ::std::result::Result::Err(__gom_denied) =
                ::graphql_orm::graphql::orm::authorize_computed_object_field::<Self, #backend>(
                    #context,
                    #record,
                    &<Self as ::graphql_orm::graphql::orm::HandwrittenObjectFields>::FIELDS[#index],
                )
                .await
            {
                #refuse
            }
            #body
        });
        let read_policy = match &options.read_policy {
            Some(key) => quote! { ::std::option::Option::Some(#key) },
            None => quote! { ::std::option::Option::None },
        };
        let subscribe = options.subscribe;
        // An event is released only once its selected fields are proved, and
        // only a leaf value is proved by its own field decision.
        let leaf = if subscribe {
            quote! {
                fn leaf<Kind, T: ?Sized + ::graphql_orm::graphql::orm::SubscribableFieldOutput<Kind>>() {}
                leaf::<_, #value_type>();
            }
        } else {
            quote! {}
        };
        definitions.push(quote! {
            ::graphql_orm::graphql::orm::ComputedObjectFieldDef::new(
                #rust_name,
                #api_name,
                #field_description,
                #read_policy,
                #subscribe,
                || {
                    #leaf
                    <#value_type as ::graphql_orm::async_graphql::OutputType>::qualified_type_name()
                },
            )
        });
    }
    Ok(definitions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composed(rename_fields: &str, handwritten: TokenStream) -> syn::Result<String> {
        let rename_fields: TokenStream = rename_fields.parse().unwrap();
        wrap(quote! {
            "Record" { ::graphql_orm::SqliteBackend } { #rename_fields } {}
            ["id", "RecordVersion"]
            { records::__gom_object_fields_Record }
            #handwritten
        })
        .map(|tokens| tokens.to_string())
    }

    fn field() -> TokenStream {
        quote! {
            impl Record {
                /// Words in the title.
                async fn word_count(&self) -> async_graphql::Result<i32> {
                    Ok(1)
                }
            }
        }
    }

    #[test]
    fn handwritten_fields_follow_the_entity_naming_rule() {
        // No rule: the build's field case, camel case by default.
        let default = derived_field_name("word_count", None, None);
        assert!(
            composed("", field())
                .unwrap()
                .contains(&format!("name = \"{default}\""))
        );
        for (rule, name) in [
            ("camelCase", "wordCount"),
            ("PascalCase", "WordCount"),
            ("snake_case", "word_count"),
            ("SCREAMING_SNAKE_CASE", "WORD_COUNT"),
            ("lowercase", "wordcount"),
            ("UPPERCASE", "WORDCOUNT"),
        ] {
            let expanded = composed(&format!("\"{rule}\""), field()).unwrap();
            assert!(
                expanded.contains(&format!("graphql (name = \"{name}\")")),
                "{rule}: {expanded}"
            );
            assert!(
                expanded.contains(&format!(
                    "\"word_count\" , \"{name}\" , \"Words in the title.\""
                )),
                "{rule}: {expanded}"
            );
        }
    }

    #[test]
    fn the_field_check_precedes_the_handwritten_body() {
        let expanded = composed("", field()).unwrap();
        let callback = expanded
            .find("records :: __gom_object_fields_Record ! { @ object { Record }")
            .unwrap();
        let handwritten = expanded.find("async fn word_count").unwrap();
        let check = expanded.find("authorize_computed_object_field").unwrap();
        let body = expanded.find("Ok (1)").unwrap();
        assert!(
            callback < handwritten && handwritten < check && check < body,
            "{expanded}"
        );
        // A resolver without a context parameter is given one for the check.
        assert!(expanded.contains("__gom_ctx : & :: graphql_orm :: async_graphql :: Context"));
    }

    #[test]
    fn every_resolver_the_wrapper_receives_is_checked() {
        let expanded = composed(
            "",
            quote! {
                impl Record {
                    async fn one(&self) -> async_graphql::Result<i32> { Ok(1) }
                    #[graphql(name = "second")]
                    async fn two(&self, ctx: &Context<'_>) -> FieldResult<i32> { Ok(2) }
                    #[graphql_orm(read_policy = "three.read", subscribe)]
                    async fn three(&self, _: &Context<'_>) -> Result<Option<i32>> { Ok(None) }
                }
            },
        )
        .unwrap();
        // Only the field declared `Option<T>` can answer a refusal with a null.
        assert_eq!(
            expanded
                .matches("report_computed_object_field_refusal")
                .count(),
            1,
            "{expanded}"
        );
        assert_eq!(
            expanded.matches("Into :: into (__gom_denied)").count(),
            2,
            "{expanded}"
        );
        assert_eq!(expanded.matches("async fn").count(), 3, "{expanded}");
        assert_eq!(
            expanded.matches("authorize_computed_object_field").count(),
            3,
            "{expanded}"
        );
        assert!(expanded.contains("SubscribableFieldOutput"));
    }

    #[test]
    fn the_wrapper_takes_one_impl_and_no_other_methods() {
        // The shape that would carry an unwrapped method: a block of methods
        // beside the handwritten impl.
        let smuggled = wrap(quote! {
            "Record" { ::graphql_orm::SqliteBackend } {} {} ["id"]
            { __gom_object_fields_Record }
            { async fn unchecked(&self) -> async_graphql::Result<i32> { Ok(1) } }
            impl Record {}
        });
        assert!(smuggled.is_err());
        let trailing = wrap(quote! {
            "Record" { ::graphql_orm::SqliteBackend } {} {} ["id"]
            { __gom_object_fields_Record }
            impl Record {}
            async fn unchecked(&self) -> async_graphql::Result<i32> { Ok(1) }
        });
        assert!(trailing.is_err());
    }

    #[test]
    fn the_generated_methods_precede_the_wrapped_ones_in_the_callback() {
        let host = host_tokens(
            &syn::Ident::new("Record", Span::call_site()),
            "A record.",
            "Record",
            &quote! { ::graphql_orm::SqliteBackend },
            Some("PascalCase"),
            None,
            &["Id".to_owned()],
            &[quote! { async fn id(&self) -> i32 { 1 } }],
        )
        .to_string();
        let object = host.find("async_graphql :: Object").unwrap();
        let generated = host.find("async fn id").unwrap();
        let hole = host.find("$ ($ wrapped) *").unwrap();
        let wrapper = host.find("__graphql_object_fields_wrap").unwrap();
        assert!(
            object < generated && generated < hole && hole < wrapper,
            "{host}"
        );
        // The generated methods are not passed to the wrapper.
        assert_eq!(host.matches("async fn id").count(), 1, "{host}");
        assert!(host.contains("# [doc = \"A record.\"]"));
    }

    #[test]
    fn a_refused_field_keeps_the_generated_object() {
        for (handwritten, message) in [
            (
                quote! { impl Record { async fn id(&self) -> async_graphql::Result<i32> { Ok(1) } } },
                "collides with the generated field `id`",
            ),
            (
                quote! { impl Record {
                    #[graphql(name = "RecordVersion")]
                    async fn version(&self) -> async_graphql::Result<i32> { Ok(1) }
                } },
                "collides with the generated field `RecordVersion`",
            ),
            (
                quote! { impl Record { async fn plain(&self) -> i32 { 1 } } },
                "must return `async_graphql::Result<T>`",
            ),
            (
                quote! { impl Record { async fn unbound() -> async_graphql::Result<i32> { Ok(1) } } },
                "must take `&self`",
            ),
            (
                quote! { impl Record { fn sync(&self) -> async_graphql::Result<i32> { Ok(1) } } },
                "must be an `async fn`",
            ),
            (
                quote! { impl Record {
                    #[graphql(flatten)]
                    async fn more(&self) -> async_graphql::Result<i32> { Ok(1) }
                } },
                "does not support `flatten`",
            ),
            (
                quote! { impl Record {
                    #[graphql_orm(write_policy = "x")]
                    async fn more(&self) -> async_graphql::Result<i32> { Ok(1) }
                } },
                "expected read_policy or subscribe",
            ),
            (
                quote! { impl Record {
                    async fn twice(&self) -> async_graphql::Result<i32> { Ok(1) }
                    #[graphql(name = "twice")]
                    async fn again(&self) -> async_graphql::Result<i32> { Ok(1) }
                } },
                "more than once",
            ),
        ] {
            let expanded = composed("", handwritten).unwrap();
            assert!(expanded.contains(message), "{message}: {expanded}");
            assert!(expanded.contains("compile_error"), "{expanded}");
            assert!(
                expanded.contains("@ object { Record } { } { }"),
                "{expanded}"
            );
            assert!(
                !expanded.contains("authorize_computed_object_field"),
                "{expanded}"
            );
        }
    }

    #[test]
    fn only_inert_impl_attributes_reach_the_generated_object() {
        let forwarded = composed(
            "",
            quote! {
                /// Dropped: the type description is the entity's.
                #[allow(clippy::unused_async)]
                #[cfg(feature = "extra")]
                impl Record {
                    async fn one(&self) -> async_graphql::Result<i32> { Ok(1) }
                }
            },
        )
        .unwrap();
        assert!(
            forwarded
                .contains("{ # [allow (clippy :: unused_async)] # [cfg (feature = \"extra\")] }"),
            "{forwarded}"
        );
        for attribute in [
            quote! { #[appends_a_resolver] },
            quote! { #[some_crate::appends_a_resolver(argument)] },
            quote! { #[cfg_attr(feature = "extra", appends_a_resolver)] },
            quote! { #[cfg_attr(all(), allow(unused))] },
            quote! { #[derive(Debug)] },
        ] {
            let refused = composed(
                "",
                quote! {
                    #[allow(unused)]
                    #attribute
                    impl Record {
                        async fn one(&self) -> async_graphql::Result<i32> { Ok(1) }
                    }
                },
            )
            .unwrap();
            assert!(refused.contains("compile_error"), "{attribute}: {refused}");
            assert!(refused.contains("accepts only"), "{attribute}: {refused}");
            // Nothing of the impl is forwarded: no attribute and no resolver.
            assert!(
                refused.contains("@ object { Record } { } { } }"),
                "{attribute}: {refused}"
            );
            assert!(
                !refused.contains("appends_a_resolver ]"),
                "{attribute}: {refused}"
            );
        }
    }

    #[test]
    fn a_skipped_method_is_left_alone() {
        let expanded = composed(
            "",
            quote! { impl Record { #[graphql(skip)] fn helper(&self) -> i32 { 1 } } },
        )
        .unwrap();
        assert!(!expanded.contains("authorize_computed_object_field"));
        // Not a field: no definition, and async-graphql leaves it out of the object.
        assert!(
            expanded.contains("@ object { Record } { } { } # [graphql (skip)] fn helper"),
            "{expanded}"
        );
    }
}
