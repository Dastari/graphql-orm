use quote::quote;
use syn::{Data, DeriveInput, Field, Fields, GenericArgument, PathArguments, Type};

use crate::entity::{
    humanize_identifier, semantic_classification_rank, semantic_classification_tokens,
    semantic_doc_description, validate_semantic_classification, validate_semantic_description,
};
use crate::naming::apply_rename_rule;

#[derive(Default)]
struct ObjectOptions {
    name: Option<String>,
    rename_fields: Option<String>,
    description: Option<String>,
    classification: Option<String>,
}

#[derive(Default)]
struct FieldOptions {
    name: Option<String>,
    description: Option<String>,
    classification: Option<String>,
    non_exportable: bool,
    sensitive: bool,
    skip: bool,
    maximum_items: Option<u32>,
    kind: Option<String>,
}

fn parse_object_options(input: &DeriveInput) -> syn::Result<ObjectOptions> {
    let mut options = ObjectOptions::default();
    for attribute in &input.attrs {
        if attribute.path().is_ident("graphql") {
            attribute.parse_nested_meta(|nested| {
                if nested.path.is_ident("name") {
                    options.name = Some(nested.value()?.parse::<syn::LitStr>()?.value());
                } else if nested.path.is_ident("desc") {
                    let lit: syn::LitStr = nested.value()?.parse()?;
                    validate_semantic_description(&lit.value(), lit.span())?;
                    options.description = Some(lit.value());
                } else if nested.path.is_ident("rename_fields") {
                    options.rename_fields = Some(nested.value()?.parse::<syn::LitStr>()?.value());
                } else if nested.path.is_ident("complex") {
                    return Err(nested.error(
                        "GraphQLSemanticObject does not admit undeclared ComplexObject fields",
                    ));
                } else if nested.input.peek(syn::Token![=]) {
                    let _: syn::Expr = nested.value()?.parse()?;
                }
                Ok(())
            })?;
        } else if attribute.path().is_ident("graphql_orm") {
            attribute.parse_nested_meta(|nested| {
                if nested.path.is_ident("description") {
                    return Err(nested.error(
                        "GraphQLSemanticObject descriptions must use Rust documentation or #[graphql(desc = \"...\")] so SDL and semantic metadata stay identical",
                    ));
                } else if nested.path.is_ident("classification") {
                    let lit: syn::LitStr = nested.value()?.parse()?;
                    validate_semantic_classification(&lit.value(), lit.span())?;
                    options.classification = Some(lit.value());
                } else {
                    return Err(nested.error("unsupported semantic-object option"));
                }
                Ok(())
            })?;
        }
    }
    Ok(options)
}

fn parse_field_options(field: &Field) -> syn::Result<FieldOptions> {
    let mut options = FieldOptions::default();
    for attribute in &field.attrs {
        if attribute.path().is_ident("graphql") {
            attribute.parse_nested_meta(|nested| {
                if nested.path.is_ident("name") {
                    options.name = Some(nested.value()?.parse::<syn::LitStr>()?.value());
                } else if nested.path.is_ident("desc") {
                    let lit: syn::LitStr = nested.value()?.parse()?;
                    validate_semantic_description(&lit.value(), lit.span())?;
                    options.description = Some(lit.value());
                } else if nested.path.is_ident("skip") {
                    options.skip = true;
                } else if nested.path.is_ident("flatten") {
                    return Err(nested.error(
                        "GraphQLSemanticObject requires flattened fields to be declared explicitly",
                    ));
                } else if nested.input.peek(syn::Token![=]) {
                    let _: syn::Expr = nested.value()?.parse()?;
                }
                Ok(())
            })?;
        } else if attribute.path().is_ident("graphql_orm") {
            attribute.parse_nested_meta(|nested| {
                if nested.path.is_ident("description") {
                    return Err(nested.error(
                        "GraphQLSemanticObject field descriptions must use Rust documentation or #[graphql(desc = \"...\")] so SDL and semantic metadata stay identical",
                    ));
                } else if nested.path.is_ident("classification") {
                    let lit: syn::LitStr = nested.value()?.parse()?;
                    validate_semantic_classification(&lit.value(), lit.span())?;
                    options.classification = Some(lit.value());
                } else if nested.path.is_ident("non_exportable") {
                    options.non_exportable = true;
                } else if nested.path.is_ident("sensitive") {
                    options.sensitive = true;
                } else if nested.path.is_ident("maximum_items") {
                    let lit: syn::LitInt = nested.value()?.parse()?;
                    let limit = lit.base10_parse::<u32>()?;
                    if limit == 0 {
                        return Err(nested.error("maximum_items must be positive"));
                    }
                    options.maximum_items = Some(limit);
                } else if nested.path.is_ident("type_kind") {
                    let lit: syn::LitStr = nested.value()?.parse()?;
                    if !matches!(lit.value().as_str(), "scalar" | "enum" | "object") {
                        return Err(nested.error("type_kind must be scalar, enum, or object"));
                    }
                    options.kind = Some(lit.value());
                } else {
                    return Err(nested.error("unsupported semantic-object field option"));
                }
                Ok(())
            })?;
        }
    }
    if options.sensitive {
        if options
            .classification
            .as_deref()
            .is_some_and(|value| value != "secret")
        {
            return Err(syn::Error::new_spanned(
                field,
                "sensitive fields cannot declare a classification below secret",
            ));
        }
        options.classification = Some("secret".to_owned());
        options.non_exportable = true;
    }
    Ok(options)
}

fn type_ident(ty: &Type) -> Option<&syn::Ident> {
    let Type::Path(path) = ty else { return None };
    path.path.segments.last().map(|segment| &segment.ident)
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

fn is_known_scalar(ty: &Type) -> bool {
    let ty = generic_type(ty, "Option")
        .or_else(|| generic_type(ty, "Box"))
        .unwrap_or(ty);
    type_ident(ty).is_some_and(|ident| {
        matches!(
            ident.to_string().as_str(),
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

fn named_type_tokens(ty: &Type, nullable: bool, kind: &str) -> proc_macro2::TokenStream {
    let kind = match kind {
        "scalar" => quote! { ::graphql_orm::graphql::orm::GraphqlSemanticTypeKind::Scalar },
        "enum" => quote! { ::graphql_orm::graphql::orm::GraphqlSemanticTypeKind::Enum },
        "object" => quote! { ::graphql_orm::graphql::orm::GraphqlSemanticTypeKind::Object },
        _ => unreachable!(),
    };
    quote! {
        ::graphql_orm::graphql::orm::GraphqlSemanticTypeRef::named(
            <#ty as ::graphql_orm::async_graphql::OutputType>::type_name().into_owned(),
            #kind,
            #nullable,
        )
    }
}

fn type_tokens(
    ty: &Type,
    kind: Option<&str>,
    maximum_items: Option<u32>,
    nullable: bool,
) -> syn::Result<(
    proc_macro2::TokenStream,
    Option<(proc_macro2::TokenStream, bool)>,
)> {
    if let Some(inner) = generic_type(ty, "Option") {
        return type_tokens(inner, kind, maximum_items, true);
    }
    if let Some(inner) = generic_type(ty, "Box") {
        return type_tokens(inner, kind, maximum_items, nullable);
    }
    if let Some(inner) = generic_type(ty, "Vec") {
        let limit = maximum_items.ok_or_else(|| {
            syn::Error::new_spanned(
                ty,
                "public semantic-object lists require #[graphql_orm(maximum_items = ...)]",
            )
        })?;
        let (item, relationship) = type_tokens(inner, kind, None, false)?;
        return Ok((
            quote! {
                ::graphql_orm::graphql::orm::GraphqlSemanticTypeRef::list(
                    #nullable,
                    Some(#limit),
                    #item,
                )
            },
            relationship.map(|(target, _)| (target, true)),
        ));
    }
    let inferred = if is_known_scalar(ty) {
        "scalar"
    } else {
        "object"
    };
    let kind = kind.unwrap_or(inferred);
    let relationship = (kind == "object").then(|| {
        (
            quote! {
                <#ty as ::graphql_orm::async_graphql::OutputType>::type_name().into_owned()
            },
            false,
        )
    });
    Ok((named_type_tokens(ty, nullable, kind), relationship))
}

pub(crate) fn expand(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "GraphQLSemanticObject does not support generic result objects",
        ));
    }
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "GraphQLSemanticObject can only be derived for structs",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            input,
            "GraphQLSemanticObject requires named fields",
        ));
    };
    let options = parse_object_options(input)?;
    let object_name = options.name.unwrap_or_else(|| input.ident.to_string());
    let description = options
        .description
        .or(semantic_doc_description(&input.attrs)?)
        .unwrap_or_else(|| humanize_identifier(&input.ident.to_string()));
    validate_semantic_description(&description, input.ident.span())?;
    let classification = options
        .classification
        .unwrap_or_else(|| "internal".to_owned());
    let classification_tokens =
        semantic_classification_tokens(&classification, input.ident.span())?;
    let rename_fields = options
        .rename_fields
        .unwrap_or_else(|| "camelCase".to_owned());
    let mut field_tokens = Vec::new();
    for field in &fields.named {
        let field_ident = field.ident.as_ref().expect("named field");
        let field_options = parse_field_options(field)?;
        if field_options.skip {
            continue;
        }
        let graphql_name = field_options
            .name
            .unwrap_or_else(|| apply_rename_rule(&field_ident.to_string(), &rename_fields));
        let field_description = field_options
            .description
            .or(semantic_doc_description(&field.attrs)?)
            .unwrap_or_else(|| humanize_identifier(&field_ident.to_string()));
        validate_semantic_description(&field_description, field_ident.span())?;
        let field_classification = field_options
            .classification
            .as_deref()
            .unwrap_or(&classification);
        if semantic_classification_rank(field_classification)
            < semantic_classification_rank(&classification)
        {
            return Err(syn::Error::new_spanned(
                field,
                "field classification cannot weaken its object classification",
            ));
        }
        let field_classification_tokens =
            semantic_classification_tokens(field_classification, field_ident.span())?;
        let export = if field_options.non_exportable || field_classification == "secret" {
            quote! { ::graphql_orm::graphql::orm::GraphqlSemanticExport::NeverExport }
        } else {
            quote! { ::graphql_orm::graphql::orm::GraphqlSemanticExport::Exportable }
        };
        let (type_ref, relationship) = type_tokens(
            &field.ty,
            field_options.kind.as_deref(),
            field_options.maximum_items,
            false,
        )?;
        let relationship = if let Some((target, multiple)) = relationship {
            let cardinality = if multiple {
                quote! { ::graphql_orm::graphql::orm::GraphqlSemanticRelationshipCardinality::Many }
            } else {
                quote! { ::graphql_orm::graphql::orm::GraphqlSemanticRelationshipCardinality::One }
            };
            quote! {
                Some(::graphql_orm::graphql::orm::GraphqlSemanticRelationshipDescriptor {
                    target: #target,
                    cardinality: #cardinality,
                    arguments: Vec::new(),
                })
            }
        } else {
            quote! { None }
        };
        field_tokens.push(quote! {
            ::graphql_orm::graphql::orm::GraphqlSemanticFieldMetadata {
                field_name: #graphql_name.to_owned(),
                description: #field_description.to_owned(),
                type_ref: #type_ref,
                selectable: true,
                filter_operators: Vec::new(),
                sortable: false,
                groupable: false,
                aggregate_operators: Vec::new(),
                aggregate_value_kind: None,
                relationship: #relationship,
                classification: #field_classification_tokens,
                export: #export,
                has_field_policy: false,
            }
        });
    }
    let object = &input.ident;
    Ok(quote! {
        impl ::graphql_orm::graphql::orm::GraphqlSemanticObjectMetadata for #object {
            fn graphql_semantic_object(
            ) -> &'static ::graphql_orm::graphql::orm::GraphqlEntitySemanticMetadata {
                static METADATA: ::std::sync::OnceLock<
                    ::graphql_orm::graphql::orm::GraphqlEntitySemanticMetadata
                > = ::std::sync::OnceLock::new();
                METADATA.get_or_init(|| ::graphql_orm::graphql::orm::GraphqlEntitySemanticMetadata {
                    entity_name: #object_name.to_owned(),
                    description: #description.to_owned(),
                    default_classification: #classification_tokens,
                    fields: vec![#(#field_tokens),*].into_boxed_slice(),
                })
            }
        }
    })
}
