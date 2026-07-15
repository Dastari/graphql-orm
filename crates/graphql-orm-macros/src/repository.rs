use std::collections::{BTreeSet, HashSet};

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::visit_mut::{self, VisitMut};
use syn::{DeriveInput, Fields, Item, ItemStruct, parse_quote};

use crate::backend::BackendKind;
use crate::backend::{backend_marker_tokens, resolve_backend};
use crate::entity::{has_graphql_entity_attribute, parse_entity_metadata, parse_field_metadata};
use crate::naming::graphql_field_name;

pub(crate) fn ensure_repository_declaration(input: &DeriveInput) -> syn::Result<()> {
    if has_graphql_entity_attribute(&input.attrs) {
        return Err(syn::Error::new_spanned(
            input,
            "RepositoryEntity requires #[repository_entity(...)], not #[graphql_entity(...)]",
        ));
    }
    if !input
        .attrs
        .iter()
        .any(|attribute| attribute.path().is_ident("repository_entity"))
    {
        return Err(syn::Error::new_spanned(
            input,
            "RepositoryEntity requires #[repository_entity(...)]",
        ));
    }
    let metadata = parse_entity_metadata(&input.attrs)?;
    if metadata.schema_only {
        return Err(syn::Error::new_spanned(
            input,
            "RepositoryEntity cannot use schema_only; use GraphQLSchemaEntity for metadata-only declarations",
        ));
    }
    if metadata.auth.is_some() {
        return Err(syn::Error::new_spanned(
            input,
            "repository-only entities do not generate GraphQL resolvers and cannot declare resolver auth; use read_policy/write_policy and repository authorization",
        ));
    }
    let backend = resolve_backend(
        metadata.backend.as_deref(),
        input.ident.span(),
        "repository_entity",
    )?;
    if backend == BackendKind::Mssql
        && (metadata.upsert.is_some()
            || metadata.repository_mutations
            || metadata.append_only
            || metadata.retention_policy.is_some()
            || metadata.notify_handler.is_some())
    {
        return Err(syn::Error::new_spanned(
            input,
            "repository-only MSSQL entities are read-only; write, append-only, upsert, retention, and mutation-hook options are unsupported",
        ));
    }
    Ok(())
}

pub(crate) fn strip_entity_graphql_surface(
    input: &DeriveInput,
    tokens: TokenStream,
) -> syn::Result<TokenStream> {
    let entity_name = input.ident.to_string();
    let where_name = format!("{entity_name}WhereInput");
    let order_name = format!("{entity_name}OrderByInput");
    let sensitive = sensitive_fields(input)?;
    let mut file = syn::parse2::<syn::File>(tokens)?;
    let mut debug_impls = Vec::new();
    let (create_authorizer, update_authorizer) = repository_write_authorizers(input)?;

    file.items.retain_mut(|item| match item {
        Item::Impl(item_impl) if has_async_graphql_impl_attribute(&item_impl.attrs) => false,
        Item::Struct(item_struct) if item_struct.ident == where_name => {
            sanitize_plain_struct(item_struct, true, true);
            debug_impls.push(debug_impl(item_struct, &sensitive));
            true
        }
        Item::Struct(item_struct) if item_struct.ident == order_name => {
            sanitize_plain_struct(item_struct, true, false);
            true
        }
        Item::Impl(item_impl)
            if item_impl
                .trait_
                .as_ref()
                .and_then(|(_, path, _)| path.segments.last())
                .is_some_and(|segment| segment.ident == "Entity") =>
        {
            if let Some(authorizer) = create_authorizer.clone() {
                item_impl.items.push(authorizer);
            }
            if let Some(authorizer) = update_authorizer.clone() {
                item_impl.items.push(authorizer);
            }
            true
        }
        _ => true,
    });
    file.items.extend(debug_impls);
    let output = file.into_token_stream();
    if contains_generated_graphql_impl(&output) {
        return Err(syn::Error::new_spanned(
            input,
            "internal RepositoryEntity generation error: an async-graphql implementation remained in emitted entity code",
        ));
    }
    Ok(output)
}

struct RepositorySensitiveEventRedactor<'a> {
    changed_event: syn::Ident,
    fields: &'a BTreeSet<String>,
    redact_identity: bool,
}

impl VisitMut for RepositorySensitiveEventRedactor<'_> {
    fn visit_item_struct_mut(&mut self, item: &mut ItemStruct) {
        if item.ident == self.changed_event {
            if let Fields::Named(fields) = &mut item.fields {
                fields.named = std::mem::take(&mut fields.named)
                    .into_iter()
                    .filter(|field| {
                        field.ident.as_ref().is_none_or(|ident| {
                            ident != "entity"
                                && (!self.redact_identity || (ident != "id" && ident != "key"))
                        })
                    })
                    .collect();
            }
        }
        visit_mut::visit_item_struct_mut(self, item);
    }

    fn visit_impl_item_fn_mut(&mut self, function: &mut syn::ImplItemFn) {
        if function.sig.ident == "__gom_capture_entity_state" {
            let sensitive = self.fields.iter().collect::<Vec<_>>();
            function.block = parse_quote!({
                ::graphql_orm::graphql::orm::entity_state_redacted(
                    entity,
                    &[#(#sensitive),*],
                )
            });
        }
        visit_mut::visit_impl_item_fn_mut(self, function);
    }

    fn visit_expr_struct_mut(&mut self, expression: &mut syn::ExprStruct) {
        let type_name = expression
            .path
            .segments
            .last()
            .map(|segment| &segment.ident);
        if type_name == Some(&self.changed_event) {
            expression.fields = std::mem::take(&mut expression.fields)
                .into_iter()
                .filter(|field| {
                    !matches!(&field.member, syn::Member::Named(ident)
                        if ident == "entity"
                            || (self.redact_identity && (ident == "id" || ident == "key")))
                })
                .collect();
        } else if type_name.is_some_and(|ident| ident == "MutationEvent") && self.redact_identity {
            for field in &mut expression.fields {
                if matches!(&field.member, syn::Member::Named(ident) if ident == "id") {
                    field.expr = parse_quote!("[redacted]".to_string());
                }
            }
        } else if type_name.is_some_and(|ident| ident == "MutationFieldValue") {
            let sensitive = expression.fields.iter().any(|field| {
                if !matches!(&field.member, syn::Member::Named(ident) if ident == "field") {
                    return false;
                }
                let syn::Expr::Lit(literal) = &field.expr else {
                    return false;
                };
                let syn::Lit::Str(value) = &literal.lit else {
                    return false;
                };
                self.fields.contains(&value.value())
            });
            if sensitive {
                for field in &mut expression.fields {
                    if matches!(&field.member, syn::Member::Named(ident) if ident == "value") {
                        field.expr = parse_quote!(::graphql_orm::graphql::orm::SqlValue::String(
                            "[redacted]".to_string()
                        ));
                    }
                }
            }
        }
        visit_mut::visit_expr_struct_mut(self, expression);
    }
}

fn repository_write_authorizers(
    input: &DeriveInput,
) -> syn::Result<(Option<syn::ImplItem>, Option<syn::ImplItem>)> {
    let metadata = parse_entity_metadata(&input.attrs)?;
    let backend = resolve_backend(
        metadata.backend.as_deref(),
        input.ident.span(),
        "repository_entity",
    )?;
    let writable_backend = backend != BackendKind::Mssql
        && metadata.schema_policy.as_deref() != Some("external_read_only");
    let syn::Data::Struct(data) = &input.data else {
        return Ok((None, None));
    };
    let Fields::Named(fields) = &data.fields else {
        return Ok((None, None));
    };
    let primary_keys = fields
        .named
        .iter()
        .filter(|field| parse_field_metadata(field).is_ok_and(|meta| meta.is_primary_key))
        .collect::<Vec<_>>();
    let composite = primary_keys.len() > 1;
    let create_available = writable_backend && (!composite || metadata.repository_mutations);
    let update_available = create_available && !metadata.append_only;
    if !create_available {
        return Ok((None, None));
    }
    let first_primary = primary_keys.first().copied().or_else(|| {
        fields
            .named
            .iter()
            .find(|field| field.ident.as_ref().is_some_and(|ident| ident == "id"))
    });
    let auto_generated_pk = first_primary
        .and_then(|field| {
            parse_field_metadata(field).ok().map(|meta| {
                meta.auto_generated
                    .unwrap_or_else(|| field.ident.as_ref().is_some_and(|ident| ident == "id"))
            })
        })
        .unwrap_or(false);
    let entity = &input.ident;
    let create_input = syn::Ident::new(&format!("Create{entity}Input"), entity.span());
    let update_input = syn::Ident::new(&format!("Update{entity}Input"), entity.span());
    let graphql_rename_fields = metadata.graphql_rename_fields.as_deref();
    let serde_rename_all = metadata.serde_rename_all.as_deref();
    let entity_name = entity.to_string();
    let mut create_checks = Vec::new();
    let mut update_checks = Vec::new();
    for field in &fields.named {
        let Some(ident) = field.ident.as_ref() else {
            continue;
        };
        let field_meta = parse_field_metadata(field)?;
        if field_meta.is_relation || field_meta.skip_db {
            continue;
        }
        let rust_name = ident.to_string();
        let api_name = graphql_field_name(
            &field_meta,
            &rust_name,
            graphql_rename_fields,
            serde_rename_all,
        );
        let policy = field_meta
            .write_policy
            .as_deref()
            .map(|policy| quote! { Some(#policy) })
            .unwrap_or_else(|| quote! { None });
        let timestamp = rust_name == "created_at" || rust_name == "updated_at";
        let include_create =
            (!field_meta.is_primary_key || !auto_generated_pk) && !timestamp && field_meta.write;
        if include_create {
            create_checks.push(quote! {
                db.ensure_repository_writable_field(
                    None,
                    #entity_name,
                    #api_name,
                    #policy,
                    None,
                    Some(&input.#ident as &(dyn ::std::any::Any + Send + Sync)),
                ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
            });
        }
        if !field_meta.is_primary_key && !timestamp && field_meta.write {
            update_checks.push(quote! {
                if let Some(value) = input.#ident.as_ref() {
                    db.ensure_repository_writable_field(
                        None,
                        #entity_name,
                        #api_name,
                        #policy,
                        existing,
                        Some(value as &(dyn ::std::any::Any + Send + Sync)),
                    ).await.map_err(::graphql_orm::graphql::errors::sqlx_error_from_public)?;
                }
            });
        }
    }
    let create_authorizer = parse_quote! {
        fn authorize_repository_create_fields<'a, B: ::graphql_orm::graphql::orm::OrmBackend>(
            db: &'a ::graphql_orm::db::Database<B>,
            input: &'a (dyn ::std::any::Any + Send + Sync),
        ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<()>> {
            Box::pin(async move {
                let input = input.downcast_ref::<#create_input>().ok_or_else(|| {
                    ::graphql_orm::sqlx::Error::Protocol(
                        concat!("repository create input type mismatch for ", stringify!(#entity)).to_string()
                    )
                })?;
                #(#create_checks)*
                Ok(())
            })
        }
    };
    let update_authorizer = update_available.then(|| parse_quote! {
        fn authorize_repository_update_fields<'a, B: ::graphql_orm::graphql::orm::OrmBackend>(
            db: &'a ::graphql_orm::db::Database<B>,
            existing: Option<&'a (dyn ::std::any::Any + Send + Sync)>,
            input: &'a (dyn ::std::any::Any + Send + Sync),
        ) -> ::graphql_orm::futures::future::BoxFuture<'a, ::graphql_orm::Result<()>> {
            Box::pin(async move {
                let input = input.downcast_ref::<#update_input>().ok_or_else(|| {
                    ::graphql_orm::sqlx::Error::Protocol(
                        concat!("repository update input type mismatch for ", stringify!(#entity)).to_string()
                    )
                })?;
                #(#update_checks)*
                Ok(())
            })
        }
    });
    Ok((Some(create_authorizer), update_authorizer))
}

pub(crate) fn strip_operations_graphql_surface(
    input: &DeriveInput,
    tokens: TokenStream,
) -> syn::Result<TokenStream> {
    let entity_name = input.ident.to_string();
    let metadata = parse_entity_metadata(&input.attrs)?;
    let plural_name = metadata
        .plural_name
        .clone()
        .unwrap_or_else(|| format!("{entity_name}s"));
    let removed = HashSet::from([
        format!("{entity_name}Edge"),
        format!("{entity_name}Connection"),
        format!("{entity_name}SearchEdge"),
        format!("{entity_name}SearchConnection"),
        format!("GraphQLCreate{entity_name}Input"),
        format!("GraphQLUpdate{entity_name}Input"),
        format!("{entity_name}Result"),
        format!("Upsert{entity_name}Result"),
        format!("{entity_name}Queries"),
        format!("{entity_name}Mutations"),
        format!("{entity_name}Subscriptions"),
        format!("Update{plural_name}Result"),
        format!("Delete{plural_name}Result"),
    ]);
    let create_name = format!("Create{entity_name}Input");
    let update_name = format!("Update{entity_name}Input");
    let key_name = format!("{entity_name}Key");
    let changed_event_name = format!("{entity_name}ChangedEvent");
    let sensitive = sensitive_fields(input)?;
    let backend = resolve_backend(
        metadata.backend.as_deref(),
        input.ident.span(),
        "repository_entity",
    )?;
    let backend_marker = backend_marker_tokens(backend);
    let entity_ident = &input.ident;
    let where_ident = syn::Ident::new(&format!("{entity_name}WhereInput"), input.ident.span());
    let order_ident = syn::Ident::new(&format!("{entity_name}OrderByInput"), input.ident.span());
    let mut file = syn::parse2::<syn::File>(tokens)?;
    if !sensitive.is_empty() {
        RepositorySensitiveEventRedactor {
            changed_event: syn::Ident::new(&changed_event_name, input.ident.span()),
            fields: &sensitive,
            redact_identity: has_sensitive_primary_key(input)?,
        }
        .visit_file_mut(&mut file);
    }
    let mut debug_impls = Vec::new();

    file.items.retain_mut(|item| match item {
        Item::Struct(item_struct) if removed.contains(&item_struct.ident.to_string()) => false,
        Item::Struct(item_struct) if item_struct.ident == changed_event_name => {
            sanitize_plain_struct(item_struct, false, false);
            true
        }
        Item::Struct(item_struct)
            if item_struct.ident == create_name
                || item_struct.ident == update_name
                || item_struct.ident == key_name =>
        {
            let default = item_struct.ident == update_name;
            sanitize_plain_struct(item_struct, default, true);
            debug_impls.push(debug_impl(item_struct, &sensitive));
            true
        }
        Item::Impl(item_impl) if has_async_graphql_impl_attribute(&item_impl.attrs) => false,
        Item::Impl(item_impl)
            if token_stream_mentions_any(&item_impl.to_token_stream(), &removed) =>
        {
            false
        }
        Item::Impl(item_impl)
            if impl_self_ident(item_impl).is_some_and(|ident| ident == entity_name) =>
        {
            remove_pool_bound_public_methods(item_impl);
            rewrite_repository_search_method(item_impl, &where_ident, &backend_marker);
            true
        }
        _ => true,
    });
    file.items.extend(debug_impls);
    file.items.push(parse_quote! {
        impl #entity_ident {
            /// Start a bounded, policy-aware repository query without exposing a raw pool.
            pub fn query(
                db: &::graphql_orm::db::Database<#backend_marker>,
            ) -> ::graphql_orm::graphql::orm::RepositoryQuery<'_, Self, #where_ident, #order_ident, #backend_marker> {
                ::graphql_orm::graphql::orm::RepositoryQuery::new(db)
            }
        }
    });
    let output = file.into_token_stream();
    if contains_generated_graphql_impl(&output) {
        return Err(syn::Error::new_spanned(
            input,
            "internal RepositoryEntity generation error: an async-graphql implementation remained in emitted repository code",
        ));
    }
    Ok(output)
}

fn rewrite_repository_search_method(
    item: &mut syn::ItemImpl,
    where_input: &syn::Ident,
    backend_marker: &TokenStream,
) {
    for member in &mut item.items {
        let syn::ImplItem::Fn(function) = member else {
            continue;
        };
        if function.sig.ident != "search_db" {
            continue;
        }
        let attributes = std::mem::take(&mut function.attrs);
        let mut replacement: syn::ImplItemFn = parse_quote! {
            pub fn search_db<'a>(
                db: &'a ::graphql_orm::db::Database<#backend_marker>,
                search: ::graphql_orm::graphql::filters::SearchInput,
            ) -> ::graphql_orm::graphql::orm::RepositorySearchQuery<'a, Self, #where_input, #backend_marker> {
                ::graphql_orm::graphql::orm::RepositorySearchQuery::new(db, search)
            }
        };
        replacement.attrs = attributes;
        *function = replacement;
    }
}

fn impl_self_ident(item: &syn::ItemImpl) -> Option<String> {
    let syn::Type::Path(path) = item.self_ty.as_ref() else {
        return None;
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

fn remove_pool_bound_public_methods(item: &mut syn::ItemImpl) {
    item.items.retain_mut(|member| {
        let syn::ImplItem::Fn(function) = member else {
            return true;
        };
        if !matches!(function.vis, syn::Visibility::Public(_)) {
            return true;
        }
        let pool_bound = function.sig.inputs.iter().any(|argument| {
            let syn::FnArg::Typed(argument) = argument else {
                return false;
            };
            matches!(argument.pat.as_ref(), syn::Pat::Ident(ident) if ident.ident == "pool")
        });
        if pool_bound {
            return false;
        }

        // Repository-only entities route list reads through the Database-bound,
        // policy-aware query builder. The GraphQL generator's compatibility
        // helpers otherwise call EntityQuery directly and would not enforce the
        // repository row/field policy boundary.
        if function.sig.ident == "find_all" {
            function.block = parse_quote!({ Self::query(db).fetch_all().await });
        } else if function.sig.ident == "find_many" {
            function.block =
                parse_quote!({ Self::query(db).filter(where_input).fetch_all().await });
        }
        true
    });
}

fn sensitive_fields(input: &DeriveInput) -> syn::Result<BTreeSet<String>> {
    let entity_metadata = parse_entity_metadata(&input.attrs)?;
    let syn::Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "RepositoryEntity can only be derived for structs",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            input,
            "RepositoryEntity requires named fields",
        ));
    };
    let mut sensitive = BTreeSet::new();
    for field in &fields.named {
        let Some(ident) = field.ident.as_ref() else {
            continue;
        };
        let metadata = parse_field_metadata(field)?;
        if !metadata.sensitive {
            continue;
        }
        let rust_name = ident.to_string();
        sensitive.insert(rust_name.clone());
        sensitive.insert(graphql_field_name(
            &metadata,
            &rust_name,
            entity_metadata.graphql_rename_fields.as_deref(),
            entity_metadata.serde_rename_all.as_deref(),
        ));
        if let Some(column) = metadata.db_column {
            sensitive.insert(column);
        }
    }
    Ok(sensitive)
}

fn has_sensitive_primary_key(input: &DeriveInput) -> syn::Result<bool> {
    let syn::Data::Struct(data) = &input.data else {
        return Ok(false);
    };
    for field in &data.fields {
        let metadata = parse_field_metadata(field)?;
        if metadata.is_primary_key && metadata.sensitive {
            return Ok(true);
        }
    }
    Ok(false)
}

fn sanitize_plain_struct(item: &mut ItemStruct, default: bool, manual_debug: bool) {
    item.attrs.retain(|attribute| {
        !attribute.path().is_ident("derive") && !attribute.path().is_ident("graphql")
    });
    item.attrs.push(if default {
        if manual_debug {
            parse_quote!(#[derive(Clone, Default)])
        } else {
            parse_quote!(#[derive(Clone, Debug, Default)])
        }
    } else if manual_debug {
        parse_quote!(#[derive(Clone)])
    } else {
        parse_quote!(#[derive(Clone)])
    });
    if let Fields::Named(fields) = &mut item.fields {
        for field in &mut fields.named {
            field
                .attrs
                .retain(|attribute| !attribute.path().is_ident("graphql"));
        }
    }
}

fn debug_impl(item: &ItemStruct, sensitive: &BTreeSet<String>) -> Item {
    let ident = &item.ident;
    let name = ident.to_string();
    let fields = match &item.fields {
        Fields::Named(fields) => fields
            .named
            .iter()
            .filter_map(|field| field.ident.as_ref())
            .map(|field| {
                let label = field.to_string();
                if sensitive.contains(&label) {
                    quote! { debug.field(#label, &"[redacted]"); }
                } else {
                    quote! { debug.field(#label, &self.#field); }
                }
            })
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    parse_quote! {
        impl ::std::fmt::Debug for #ident {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                let mut debug = formatter.debug_struct(#name);
                #(#fields)*
                debug.finish()
            }
        }
    }
}

fn has_async_graphql_impl_attribute(attributes: &[syn::Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        let path = attribute.path();
        path.segments.iter().any(|segment| {
            matches!(
                segment.ident.to_string().as_str(),
                "Object" | "Subscription" | "ComplexObject"
            )
        }) && path
            .segments
            .iter()
            .any(|segment| segment.ident == "async_graphql")
    })
}

fn token_stream_mentions_any(tokens: &TokenStream, names: &HashSet<String>) -> bool {
    tokens.clone().into_iter().any(|token| match token {
        proc_macro2::TokenTree::Ident(ident) => names.contains(&ident.to_string()),
        proc_macro2::TokenTree::Group(group) => token_stream_mentions_any(&group.stream(), names),
        _ => false,
    })
}

fn contains_generated_graphql_impl(tokens: &TokenStream) -> bool {
    let rendered = tokens.to_string();
    [
        "async_graphql :: Object",
        "async_graphql :: InputObject",
        "async_graphql :: SimpleObject",
        "async_graphql :: Subscription",
    ]
    .iter()
    .any(|needle| rendered.contains(needle))
}
