use convert_case::{Case, Casing};
use proc_macro::TokenStream;
use quote::{ToTokens, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{
    FnArg, GenericArgument, ImplItem, ItemImpl, Meta, Pat, PathArguments, ReturnType, Token, Type,
};

use crate::entity::{
    semantic_classification_tokens, semantic_doc_description, validate_semantic_classification,
    validate_semantic_description,
};

struct CustomRootArgs {
    kind: String,
    authorization: bool,
    ai_execution: Option<String>,
    observation: Option<String>,
    maximum_duration_seconds: Option<u32>,
    maximum_events: Option<u32>,
}

impl Parse for CustomRootArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let items = Punctuated::<Meta, Token![,]>::parse_terminated(input)?;
        let mut kind = None;
        let mut authorization = true;
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
                authorization = lit.value;
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

fn graphql_attribute_value(attrs: &[syn::Attribute], key: &str) -> syn::Result<Option<String>> {
    let mut result = None;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("graphql")) {
        attr.parse_nested_meta(|nested| {
            if nested.path.is_ident(key) {
                let lit: syn::LitStr = nested.value()?.parse()?;
                if result.replace(lit.value()).is_some() {
                    return Err(nested.error(format!("duplicate graphql {key}")));
                }
            } else if nested.input.peek(Token![=]) {
                let _: syn::Expr = nested.value()?.parse()?;
            }
            Ok(())
        })?;
    }
    Ok(result)
}

fn has_graphql_flag(attrs: &[syn::Attribute], key: &str) -> syn::Result<bool> {
    let mut found = false;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("graphql")) {
        attr.parse_nested_meta(|nested| {
            if nested.path.is_ident(key) {
                found = true;
            } else if nested.input.peek(Token![=]) {
                let _: syn::Expr = nested.value()?.parse()?;
            }
            Ok(())
        })?;
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
}

fn take_semantic_options(attrs: &mut Vec<syn::Attribute>) -> syn::Result<MethodSemanticOptions> {
    let mut options = MethodSemanticOptions::default();
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
                } else {
                    Err(nested.error("unsupported custom-operation semantic attribute"))
                }
            })?;
        } else {
            retained.push(attr);
        }
    }
    *attrs = retained;
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
    let self_ty = item.self_ty.clone();
    let mut descriptors = Vec::new();
    let mut field_names = std::collections::BTreeSet::new();
    let mut result_type_names = std::collections::BTreeSet::new();
    let mut result_types = Vec::<Type>::new();

    for impl_item in &mut item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        match has_graphql_flag(&method.attrs, "skip") {
            Ok(true) => continue,
            Ok(false) => {}
            Err(error) => return error.to_compile_error().into(),
        }
        let semantic_options = match take_semantic_options(&mut method.attrs) {
            Ok(options) => options,
            Err(error) => return error.to_compile_error().into(),
        };
        let explicit = semantic_options.description.clone();
        let method_name = method.sig.ident.to_string();
        let field_name = match graphql_attribute_value(&method.attrs, "name") {
            Ok(Some(name)) => name,
            Ok(None) => method_name.to_case(Case::Camel),
            Err(error) => return error.to_compile_error().into(),
        };
        if !field_names.insert(field_name.clone()) {
            return syn::Error::new_spanned(
                &method.sig.ident,
                "duplicate custom semantic GraphQL root field name",
            )
            .to_compile_error()
            .into();
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
            let rust_name = pat.ident.to_string();
            let graphql_name = match graphql_attribute_value(&argument.attrs, "name") {
                Ok(Some(name)) => name,
                Ok(None) => rust_name.to_case(Case::Camel),
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
                argument
                    .attrs
                    .push(syn::parse_quote!(#[graphql(desc = #description)]));
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
        let authorization = args.authorization;
        descriptors.push(quote! {
            ::graphql_orm::graphql::orm::GraphqlSemanticOperationDescriptor::custom(
                #kind,
                #field_name,
                #description,
                vec![#(#arguments),*],
                ::graphql_orm::graphql::orm::parse_graphql_type(
                    &<#output as ::graphql_orm::async_graphql::OutputType>::qualified_type_name(),
                ).expect("custom resolver result type must be valid GraphQL"),
                #authorization,
            ).expect("custom resolver semantic metadata must validate")
            #result_kind_override
            #result_disclosure
            #ai_execution
            #observation
        });
    }

    quote! {
        #item

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
