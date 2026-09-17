//! Apollo Federation v2 entity keys for generated ORM entities.
//!
//! async-graphql produces `@key(fields: "...", resolvable: true)` from exactly
//! one construct: an `#[graphql(entity)]` method inside an `#[Object] impl`.
//! The emitted key string is the resolver's *argument* names joined by spaces,
//! so each key argument must be renamed to the entity's exported GraphQL field
//! name. The ORM configures argument case independently of field case, so
//! relying on the default argument name would produce a `@key` naming a field
//! that does not exist.
//!
//! Entity resolvers are not schema fields. `MergedObject` forwards
//! `find_entity` to each member, and `schema_roots!` composes `QueryRoot` from
//! the per-entity `{Entity}Queries` objects, so a resolver generated there
//! participates in `_entities` without changing the operation catalogue or the
//! router protocol descriptor.

use crate::backend::{BackendKind, backend_quote_identifier_path};
use crate::entity::{EntityMetadata, FieldMetadata, parse_field_metadata};
use crate::naming::graphql_field_name;
use crate::relations::{
    RelationValueKind, classify_relation_value_type, relation_key_part_kind_tokens,
};
use quote::quote;
use syn::spanned::Spanned;

/// One resolver argument standing for one field of one `@key`.
pub(crate) struct FederationKeyArgument {
    /// Exported GraphQL field name; this is what the `@key` string contains.
    pub(crate) graphql_name: String,
    pub(crate) rust_name: String,
    pub(crate) argument_ident: syn::Ident,
    pub(crate) rust_type: syn::Type,
    pub(crate) quoted_column: String,
    pub(crate) kind: RelationValueKind,
    pub(crate) is_primary_key: bool,
}

/// One validated `@key` declaration.
pub(crate) struct ResolvedFederationKey {
    pub(crate) arguments: Vec<FederationKeyArgument>,
    /// Stable batching identity for the shared relation loader.
    pub(crate) loader_identity: String,
    pub(crate) method_ident: syn::Ident,
}

/// Everything about a field that key validation needs.
struct CandidateField {
    rust_name: String,
    graphql_name: String,
    column: String,
    ty: syn::Type,
    meta: FieldMetadata,
    span: proc_macro2::Span,
}

fn collect_candidate_fields(
    fields: &syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
    graphql_rename_fields: Option<&str>,
    serde_rename_all: Option<&str>,
) -> syn::Result<Vec<CandidateField>> {
    let mut candidates = Vec::new();
    for field in fields {
        let Some(ident) = field.ident.as_ref() else {
            continue;
        };
        let meta = parse_field_metadata(field)?;
        let rust_name = ident.to_string();
        let graphql_name =
            graphql_field_name(&meta, &rust_name, graphql_rename_fields, serde_rename_all);
        let column = meta.db_column.clone().unwrap_or_else(|| rust_name.clone());
        candidates.push(CandidateField {
            rust_name,
            graphql_name,
            column,
            ty: field.ty.clone(),
            meta,
            span: field.span(),
        });
    }
    Ok(candidates)
}

/// Declared uniqueness sources, each as a set of Rust field names.
///
/// Index declarations name Rust fields while `unique_composite` names database
/// columns, so both spellings are accepted when normalising.
fn declared_unique_field_sets(
    entity_meta: &EntityMetadata,
    candidates: &[CandidateField],
) -> Vec<Vec<String>> {
    let normalise = |name: &str| -> Option<String> {
        candidates
            .iter()
            .find(|candidate| candidate.rust_name == name || candidate.column == name)
            .map(|candidate| candidate.rust_name.clone())
    };

    let mut sets = Vec::new();
    for candidate in candidates {
        if candidate.meta.unique {
            sets.push(vec![candidate.rust_name.clone()]);
        }
    }
    for index in &entity_meta.indexes {
        if !index.unique {
            continue;
        }
        if let Some(set) = index.columns.iter().map(|name| normalise(name)).collect() {
            sets.push(set);
        }
    }
    for columns in &entity_meta.unique_composite {
        if let Some(set) = columns.iter().map(|name| normalise(name)).collect() {
            sets.push(set);
        }
    }
    sets
}

fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names.dedup();
    names
}

/// Validate every declared key and produce the data the resolvers need.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_federation_keys(
    struct_name: &syn::Ident,
    entity_meta: &EntityMetadata,
    fields: &syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
    graphql_rename_fields: Option<&str>,
    serde_rename_all: Option<&str>,
    backend: BackendKind,
    primary_key_rust_names: &[String],
) -> syn::Result<Vec<ResolvedFederationKey>> {
    if entity_meta.federation_keys.is_empty() {
        return Ok(Vec::new());
    }

    let candidates = collect_candidate_fields(fields, graphql_rename_fields, serde_rename_all)?;
    let unique_sets = declared_unique_field_sets(entity_meta, &candidates);
    let primary_key_set = sorted(primary_key_rust_names.to_vec());

    let mut resolved: Vec<ResolvedFederationKey> = Vec::new();
    let mut seen_sets: Vec<Vec<String>> = Vec::new();

    for (index, declaration) in entity_meta.federation_keys.iter().enumerate() {
        let requested: Vec<String> = match declaration.fields.as_ref() {
            Some(names) => names.clone(),
            None => primary_key_rust_names
                .iter()
                .map(|rust_name| {
                    candidates
                        .iter()
                        .find(|candidate| &candidate.rust_name == rust_name)
                        .map(|candidate| candidate.graphql_name.clone())
                        .unwrap_or_else(|| rust_name.clone())
                })
                .collect(),
        };

        let mut arguments = Vec::new();
        let mut rust_names = Vec::new();
        for (position, requested_name) in requested.iter().enumerate() {
            let candidate = candidates
                .iter()
                .find(|candidate| &candidate.graphql_name == requested_name)
                .ok_or_else(|| {
                    syn::Error::new(
                        declaration.span,
                        format!(
                            "federation_key references unknown GraphQL field `{requested_name}` on `{struct_name}`; use the exported field name"
                        ),
                    )
                })?;

            if candidate.meta.is_relation || candidate.meta.skip_db {
                return Err(syn::Error::new(
                    candidate.span,
                    format!(
                        "federation_key field `{requested_name}` must be a persisted scalar column; relations and skipped fields cannot form a key"
                    ),
                ));
            }
            if candidate.meta.is_private || !candidate.meta.read || candidate.meta.input_only {
                return Err(syn::Error::new(
                    candidate.span,
                    format!(
                        "federation_key field `{requested_name}` is not exported in the GraphQL schema; a @key may only name readable fields"
                    ),
                ));
            }
            if candidate.meta.read_policy.is_some() {
                return Err(syn::Error::new(
                    candidate.span,
                    format!(
                        "federation_key field `{requested_name}` has a field-level read policy; a @key field is disclosed to every subgraph that joins this entity"
                    ),
                ));
            }
            let Some((kind, is_option)) = classify_relation_value_type(&candidate.ty) else {
                return Err(syn::Error::new(
                    candidate.span,
                    format!(
                        "federation_key field `{requested_name}` has an unsupported key type; keys must be String, UUID, integer, float, or boolean columns"
                    ),
                ));
            };
            if is_option {
                return Err(syn::Error::new(
                    candidate.span,
                    format!(
                        "federation_key field `{requested_name}` is nullable; a @key field must be non-null so every row has exactly one representation"
                    ),
                ));
            }

            if rust_names.contains(&candidate.rust_name) {
                return Err(syn::Error::new(
                    declaration.span,
                    format!("federation_key names field `{requested_name}` more than once"),
                ));
            }
            rust_names.push(candidate.rust_name.clone());
            arguments.push(FederationKeyArgument {
                graphql_name: candidate.graphql_name.clone(),
                rust_name: candidate.rust_name.clone(),
                argument_ident: quote::format_ident!("__gom_federation_key_{index}_{position}"),
                rust_type: candidate.ty.clone(),
                quoted_column: backend_quote_identifier_path(backend, &candidate.column),
                kind,
                is_primary_key: candidate.meta.is_primary_key
                    || primary_key_rust_names.contains(&candidate.rust_name),
            });
        }

        let key_set = sorted(rust_names.clone());
        let is_unique = key_set == primary_key_set
            || unique_sets.iter().any(|set| sorted(set.clone()) == key_set);
        if !is_unique && !declaration.assume_unique {
            return Err(syn::Error::new(
                declaration.span,
                format!(
                    "federation_key on `{struct_name}` selects `{}`, which is neither the primary key nor a declared unique constraint; \
                     a @key must identify at most one row. Declare `unique`, `unique_index`, or `unique_composite` for these columns, \
                     or set `assume_unique = true` when the uniqueness is enforced by an externally managed schema the ORM does not declare",
                    rust_names.join(", ")
                ),
            ));
        }
        if is_unique && declaration.assume_unique {
            return Err(syn::Error::new(
                declaration.span,
                format!(
                    "federation_key on `{struct_name}` already selects a declared unique column set; remove `assume_unique = true`"
                ),
            ));
        }

        if seen_sets.iter().any(|existing| existing == &key_set) {
            return Err(syn::Error::new(
                declaration.span,
                format!(
                    "duplicate federation_key on `{struct_name}`; each @key must select a distinct field set"
                ),
            ));
        }
        seen_sets.push(key_set);

        let graphql_names = arguments
            .iter()
            .map(|argument| argument.graphql_name.clone())
            .collect::<Vec<_>>();
        resolved.push(ResolvedFederationKey {
            loader_identity: format!("{struct_name}::@key({})", graphql_names.join(" ")),
            method_ident: quote::format_ident!("__gom_federation_entity_{index}"),
            arguments,
        });
    }

    Ok(resolved)
}

/// Scope-template argument aliases contributed by one key's resolver.
///
/// A key argument is addressable by the exported field name it carries. When
/// the field also belongs to the primary key, the single-read argument name is
/// accepted as an alias so an existing `single_read` scope template keeps
/// resolving under a field-case configuration that renames the field.
pub(crate) fn scope_template_aliases(
    key: &ResolvedFederationKey,
    single_read_argument_names: &[(String, String)],
) -> Vec<(String, syn::Ident, syn::Type)> {
    let mut aliases = Vec::new();
    for argument in &key.arguments {
        aliases.push((
            argument.graphql_name.clone(),
            argument.argument_ident.clone(),
            argument.rust_type.clone(),
        ));
        if argument.is_primary_key {
            for (rust_name, argument_name) in single_read_argument_names {
                if rust_name == &argument.rust_name && argument_name != &argument.graphql_name {
                    aliases.push((
                        argument_name.clone(),
                        argument.argument_ident.clone(),
                        argument.rust_type.clone(),
                    ));
                }
            }
        }
    }
    aliases
}

/// Emit one `#[graphql(entity)]` resolver per declared key.
///
/// The body reproduces the generated single-row read authorization chain:
/// resolver auth, single-read scope enforcement, assurance, entity access on
/// the `GraphqlQuery` surface, an authorized fetch, and a row-policy check that
/// yields null rather than an error. Batching is delegated to the per-entity
/// relation `DataLoader` that `schema_roots!` already registers, so every
/// representation in one `_entities` fetch collapses into a single statement.
#[allow(clippy::too_many_arguments)]
pub(crate) fn federation_entity_resolver_tokens(
    struct_name: &syn::Ident,
    entity_name_lit: &str,
    backend_marker: &proc_macro2::TokenStream,
    resolver_auth_mode: &proc_macro2::TokenStream,
    relation_preload_single: &proc_macro2::TokenStream,
    keys: &[ResolvedFederationKey],
    scope_enforcements: &[proc_macro2::TokenStream],
) -> proc_macro2::TokenStream {
    let resolvers = keys
        .iter()
        .zip(scope_enforcements.iter())
        .map(|(key, scope_enforcement)| {
            let method_ident = &key.method_ident;
            let loader_identity = &key.loader_identity;
            let description = format!(
                "Resolve one {entity_name_lit} from a Federation entity representation."
            );

            let resolver_arguments = key.arguments.iter().map(|argument| {
                let ident = &argument.argument_ident;
                let ty = &argument.rust_type;
                let name = &argument.graphql_name;
                quote! { #[graphql(name = #name)] #ident: #ty, }
            });
            let value_bindings = key.arguments.iter().map(|argument| {
                let ident = &argument.argument_ident;
                let (sql_value, key_part) = match argument.kind {
                    RelationValueKind::String => (
                        quote! { ::graphql_orm::graphql::orm::SqlValue::String(#ident.clone()) },
                        quote! { #ident.clone() },
                    ),
                    RelationValueKind::Uuid => (
                        quote! { ::graphql_orm::graphql::orm::SqlValue::Uuid(#ident) },
                        quote! { #ident.to_string() },
                    ),
                    RelationValueKind::Int => (
                        quote! { ::graphql_orm::graphql::orm::SqlValue::Int(#ident as i64) },
                        quote! { #ident.to_string() },
                    ),
                    RelationValueKind::Float => (
                        quote! { ::graphql_orm::graphql::orm::SqlValue::Float(#ident.into()) },
                        quote! { #ident.to_string() },
                    ),
                    RelationValueKind::Bool => (
                        quote! { ::graphql_orm::graphql::orm::SqlValue::Bool(#ident) },
                        quote! { #ident.to_string() },
                    ),
                };
                quote! {
                    __gom_federation_values.push(#sql_value);
                    __gom_federation_parts.push(#key_part);
                }
            });
            let key_columns = key
                .arguments
                .iter()
                .map(|argument| {
                    let column = &argument.quoted_column;
                    quote! { #column }
                })
                .collect::<Vec<_>>();
            let key_part_kinds = key
                .arguments
                .iter()
                .map(|argument| relation_key_part_kind_tokens(argument.kind))
                .collect::<Vec<_>>();

            quote! {
                #[doc = #description]
                #[graphql(entity)]
                async fn #method_ident(
                    &self,
                    ctx: &::graphql_orm::async_graphql::Context<'_>,
                    #(#resolver_arguments)*
                ) -> ::graphql_orm::async_graphql::Result<Option<#struct_name>> {
                    let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(
                        ctx,
                        #resolver_auth_mode,
                    )?;
                    #scope_enforcement
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
                    ).await?;

                    let mut __gom_federation_values = Vec::new();
                    let mut __gom_federation_parts = Vec::new();
                    #(#value_bindings)*

                    let __gom_federation_loader = ctx.data_unchecked::<
                        ::graphql_orm::async_graphql::dataloader::DataLoader<
                            ::graphql_orm::graphql::loaders::FederationKeyLoader<
                                #struct_name,
                                #backend_marker,
                            >,
                        >,
                    >();
                    let __gom_federation_loaded = __gom_federation_loader
                        .load_one(::graphql_orm::graphql::loaders::FederationEntityKey {
                            key: #loader_identity,
                            columns: vec![#(#key_columns),*],
                            key_part_kinds: vec![#(#key_part_kinds),*],
                            values: __gom_federation_values,
                            representation: ::graphql_orm::graphql::loaders::RelationKey::new(
                                __gom_federation_parts,
                            ),
                            auth_context: auth_context.clone(),
                        })
                        .await
                        .map_err(|error| {
                            ::graphql_orm::async_graphql::Error::new(error.to_string())
                        })?;

                    let mut entity = __gom_federation_loaded.flatten();

                    if let Some(ref loaded) = entity {
                        if !db.can_read_row(
                            Some(ctx),
                            #entity_name_lit,
                            <#struct_name as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                            ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery,
                            loaded as &(dyn ::std::any::Any + Send + Sync),
                        ).await? {
                            return Ok(None);
                        }
                    }

                    let _ = pool;
                    #relation_preload_single

                    Ok(entity)
                }
            }
        })
        .collect::<Vec<_>>();

    quote! { #(#resolvers)* }
}

/// Record the declared key count so `GraphQLEntity` can require this derive.
///
/// This is emitted whether or not the rest of the operations derive succeeds.
/// Otherwise a key that fails validation would report its own precise error
/// *and* a second, misleading "add `GraphQLOperations`" error.
pub(crate) fn federation_key_witness_impl(
    attrs: &[syn::Attribute],
    struct_name: &syn::Ident,
) -> syn::Result<proc_macro2::TokenStream> {
    let metadata = crate::entity::parse_entity_metadata(attrs)?;
    if metadata.federation_keys.is_empty() {
        return Ok(quote! {});
    }
    let count = metadata.federation_keys.len();
    Ok(quote! {
        impl ::graphql_orm::graphql::federation::GeneratedFederationEntityKeys for #struct_name {
            const FEDERATION_KEY_COUNT: usize = #count;
        }
    })
}
