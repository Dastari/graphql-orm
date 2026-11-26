use super::*;
use crate::backend::{backend_marker_tokens, backend_render_identifier_path, resolve_backend};
use crate::entity::{
    RelationConditionMetadata, RelationConditionValue, collect_parsed_fields,
    graphql_relation_target_checks, has_graphql_complex, parse_entity_metadata,
    relation_change_propagation_tokens, relation_delete_policy_tokens,
    relation_server_only_columns, relation_server_only_key_mask, resolver_auth_mode_tokens,
    validate_graphql_relation_keys,
};
use crate::naming::graphql_field_name;
use crate::relationship_contract::GeneratedRelationshipArgumentContract;
use syn::spanned::Spanned;

struct RelationDef {
    field_name: syn::Ident,
    graphql_name: String,
    read_policy: Option<String>,
    description: String,
    target_type_str: String,
    source_columns: Vec<String>,
    fk_columns: Vec<String>,
    is_multiple: bool,
    source_field_idents: Vec<syn::Ident>,
    source_kinds: Vec<RelationValueKind>,
    source_optional: Vec<bool>,
    emit_foreign_key: bool,
    on_delete: proc_macro2::TokenStream,
    propagate_change: proc_macro2::TokenStream,
    source_condition: Option<ResolvedSourceCondition>,
    target_condition: Option<RelationConditionMetadata>,
    /// A server-only key: batched rows associate by database match identity.
    associate_by_match: bool,
}

struct ResolvedSourceCondition {
    field_name: syn::Ident,
    kind: RelationValueKind,
    optional: bool,
    value: RelationConditionValue,
}

#[derive(Copy, Clone)]
pub(crate) enum RelationValueKind {
    String,
    Uuid,
    Int,
    Float,
    Bool,
}

pub(crate) fn classify_relation_value_type(ty: &syn::Type) -> Option<(RelationValueKind, bool)> {
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
                    "String" => RelationValueKind::String,
                    "Uuid" => RelationValueKind::Uuid,
                    "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64"
                    | "usize" => RelationValueKind::Int,
                    "f32" | "f64" => RelationValueKind::Float,
                    "bool" => RelationValueKind::Bool,
                    _ => return None,
                };
                return Some((kind, is_option));
            }
            _ => return None,
        }
    }
}

pub(crate) fn relation_key_part_kind_tokens(kind: RelationValueKind) -> proc_macro2::TokenStream {
    match kind {
        RelationValueKind::String => {
            quote! { ::graphql_orm::graphql::loaders::RelationKeyPartKind::String }
        }
        RelationValueKind::Uuid => {
            quote! { ::graphql_orm::graphql::loaders::RelationKeyPartKind::Uuid }
        }
        RelationValueKind::Int => {
            quote! { ::graphql_orm::graphql::loaders::RelationKeyPartKind::Int }
        }
        RelationValueKind::Float => {
            quote! { ::graphql_orm::graphql::loaders::RelationKeyPartKind::Float }
        }
        RelationValueKind::Bool => {
            quote! { ::graphql_orm::graphql::loaders::RelationKeyPartKind::Bool }
        }
    }
}

fn condition_value_matches_kind(value: &RelationConditionValue, kind: RelationValueKind) -> bool {
    matches!(
        (value, kind),
        (RelationConditionValue::String(_), RelationValueKind::String)
            | (RelationConditionValue::Integer(_), RelationValueKind::Int)
            | (RelationConditionValue::Float(_), RelationValueKind::Float)
            | (RelationConditionValue::Boolean(_), RelationValueKind::Bool)
    )
}

fn condition_sql_value_tokens(value: &RelationConditionValue) -> proc_macro2::TokenStream {
    match value {
        RelationConditionValue::String(value) => {
            quote! { ::graphql_orm::graphql::orm::SqlValue::String(#value.to_owned()) }
        }
        RelationConditionValue::Integer(value) => {
            quote! { ::graphql_orm::graphql::orm::SqlValue::Int(#value) }
        }
        RelationConditionValue::Float(value) => {
            quote! { ::graphql_orm::graphql::orm::SqlValue::Float(#value) }
        }
        RelationConditionValue::Boolean(value) => {
            quote! { ::graphql_orm::graphql::orm::SqlValue::Bool(#value) }
        }
    }
}

fn source_condition_tokens(
    condition: Option<&ResolvedSourceCondition>,
    receiver: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let Some(condition) = condition else {
        return quote! { true };
    };
    let field_name = &condition.field_name;
    match (&condition.value, condition.kind, condition.optional) {
        (RelationConditionValue::String(value), RelationValueKind::String, false) => {
            quote! { #receiver.#field_name == #value }
        }
        (RelationConditionValue::String(value), RelationValueKind::String, true) => {
            quote! { #receiver.#field_name.as_deref() == Some(#value) }
        }
        (RelationConditionValue::Integer(value), RelationValueKind::Int, false) => {
            quote! { (#receiver.#field_name as i64) == #value }
        }
        (RelationConditionValue::Integer(value), RelationValueKind::Int, true) => {
            quote! { #receiver.#field_name.is_some_and(|value| (value as i64) == #value) }
        }
        (RelationConditionValue::Float(value), RelationValueKind::Float, false) => {
            quote! { (#receiver.#field_name as f64) == #value }
        }
        (RelationConditionValue::Float(value), RelationValueKind::Float, true) => {
            quote! { #receiver.#field_name.is_some_and(|value| (value as f64) == #value) }
        }
        (RelationConditionValue::Boolean(value), RelationValueKind::Bool, false) => {
            quote! { #receiver.#field_name == #value }
        }
        (RelationConditionValue::Boolean(value), RelationValueKind::Bool, true) => {
            quote! { #receiver.#field_name == Some(#value) }
        }
        _ => quote! { false },
    }
}

/// Whether `GraphQLEntity` owns the composed complex object of this entity.
///
/// A `compose_complex_object` entity without readable relation fields has no
/// relation resolvers, so the entity derive mounts the generated `NodeId` and
/// `RecordVersion` itself and an optional `GraphQLRelations` derive stands down.
pub(crate) fn entity_owns_composed_object(
    legacy_graphql_complex: bool,
    entity_meta: &crate::entity::EntityMetadata,
    parsed_fields: &[crate::entity::ParsedField],
) -> bool {
    legacy_graphql_complex
        && entity_meta.compose_complex_object
        && !entity_meta.schema_only
        && !parsed_fields
            .iter()
            .any(|field| field.meta.is_relation && field.meta.graphql_readable())
}

/// The flattened object that `#[graphql_complex_object]` mounts.
///
/// `identity_names` are the generated identity fields among `fields`, which
/// handwritten composed resolvers must not shadow.
pub(crate) fn composed_object_tokens(
    input: &DeriveInput,
    fields: proc_macro2::TokenStream,
    identity_names: &[&'static str],
) -> proc_macro2::TokenStream {
    let struct_name = &input.ident;
    let entity_visibility = &input.vis;
    let relation_object = syn::Ident::new(
        &format!("{struct_name}GeneratedRelations"),
        struct_name.span(),
    );
    let relation_object_name = relation_object.to_string();
    quote! {
        #[doc(hidden)]
        #entity_visibility struct #relation_object<'a>(&'a #struct_name);

        impl<'a> ::std::ops::Deref for #relation_object<'a> {
            type Target = #struct_name;

            fn deref(&self) -> &Self::Target {
                self.0
            }
        }

        #[::graphql_orm::async_graphql::Object(name = #relation_object_name)]
        impl<'a> #relation_object<'a> {
            #fields
        }

        impl ::graphql_orm::graphql::orm::GeneratedRelationsObject for #struct_name {
            type Object<'a> = #relation_object<'a> where Self: 'a;

            const GENERATED_IDENTITY_FIELDS: &'static [&'static str] = &[#(#identity_names),*];

            fn generated_relations_object(&self) -> Self::Object<'_> {
                #relation_object(self)
            }
        }
    }
}

pub(crate) fn generate_graphql_relations(
    input: &DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;
    let entity_meta = parse_entity_metadata(&input.attrs)?;
    let resolver_auth_mode =
        resolver_auth_mode_tokens(entity_meta.auth.as_deref(), struct_name.span())?;
    let backend = resolve_backend(
        entity_meta.backend.as_deref(),
        struct_name.span(),
        "graphql_entity",
    )?;
    let backend_marker = backend_marker_tokens(backend);

    let data = match &input.data {
        Data::Struct(data) => data,
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "GraphQLRelations can only be derived for structs",
            ));
        }
    };

    let fields = match &data.fields {
        Fields::Named(fields) => &fields.named,
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "GraphQLRelations requires named fields",
            ));
        }
    };

    let graphql_rename_fields = entity_meta.graphql_rename_fields.as_deref();
    let serde_rename_all = entity_meta.serde_rename_all.as_deref();
    let legacy_graphql_complex = has_graphql_complex(&input.attrs)?;
    let parsed_fields = collect_parsed_fields(fields.iter())?;

    // Find primary key field
    let mut pk_field_name: Option<syn::Ident> = None;
    for parsed_field in &parsed_fields {
        if parsed_field.meta.is_primary_key {
            pk_field_name = Some(parsed_field.field.ident.clone().unwrap());
            break;
        }
    }
    let pk_field = pk_field_name.unwrap_or_else(|| syn::Ident::new("id", struct_name.span()));

    // Collect relations
    let mut relations: Vec<RelationDef> = Vec::new();
    let mut relation_target_checks = Vec::new();

    for parsed_field in &parsed_fields {
        let field = &parsed_field.field;
        let meta = &parsed_field.meta;
        if !meta.is_relation || !meta.graphql_readable() {
            continue;
        }

        let field_name = field.ident.clone().unwrap();
        let rust_name = field_name.to_string();
        let graphql_name =
            graphql_field_name(meta, &rust_name, graphql_rename_fields, serde_rename_all);

        let target_type = meta
            .relation_target
            .clone()
            .unwrap_or_else(|| "Unknown".to_string());
        let to_cols = meta.relation_to_fields.clone().unwrap_or_else(|| {
            vec![
                meta.relation_to
                    .clone()
                    .unwrap_or_else(|| "unknown_id".to_string()),
            ]
        });
        let from_cols = meta.relation_from_fields.clone().unwrap_or_else(|| {
            vec![
                meta.relation_from
                    .clone()
                    .unwrap_or_else(|| pk_field.to_string()),
            ]
        });
        if from_cols.len() != to_cols.len() {
            return Err(syn::Error::new_spanned(
                field,
                format!(
                    "Relation '{}' has {} source key part(s) but {} target key part(s)",
                    rust_name,
                    from_cols.len(),
                    to_cols.len()
                ),
            ));
        }
        if from_cols.is_empty() {
            return Err(syn::Error::new_spanned(
                field,
                format!("Relation '{}' must define at least one key part", rust_name),
            ));
        }

        let server_only_keys =
            relation_server_only_key_mask(meta, &parsed_fields, &from_cols, &to_cols)?;
        validate_graphql_relation_keys(
            backend,
            struct_name,
            field,
            meta,
            &parsed_fields,
            &from_cols,
            &to_cols,
            &server_only_keys,
            // A relation object is always a SimpleObject the host derives.
            &relation_server_only_columns(
                backend,
                entity_meta.table_name.as_deref().unwrap_or("unknown"),
                &parsed_fields,
                true,
            ),
        )?;
        relation_target_checks.push(graphql_relation_target_checks(
            backend,
            struct_name,
            field,
            meta,
            &to_cols,
            &server_only_keys,
        )?);
        let mut source_field_idents = Vec::with_capacity(from_cols.len());
        let mut source_kinds = Vec::with_capacity(from_cols.len());
        let mut source_optional = Vec::with_capacity(from_cols.len());
        for from_col in &from_cols {
            let source_field = parsed_fields
                .iter()
                .find(|parsed| {
                    parsed
                        .field
                        .ident
                        .as_ref()
                        .map(|ident| ident == &syn::Ident::new(from_col, ident.span()))
                        .unwrap_or(false)
                })
                .ok_or_else(|| {
                    syn::Error::new_spanned(
                        field,
                        format!(
                            "Relation '{}' references unknown source field '{}' on '{}'",
                            rust_name, from_col, struct_name
                        ),
                    )
                })?;
            let source_field_ty = source_field.field.ty.clone();
            let (source_kind, source_is_option) =
                classify_relation_value_type(&source_field_ty).ok_or_else(|| {
                    syn::Error::new_spanned(
                        &source_field.field.ty,
                        format!(
                            "Unsupported relation source field type for '{}.{}': expected String/uuid/int/float/bool (optionals allowed)",
                            struct_name, from_col
                        ),
                    )
                })?;
            source_field_idents.push(source_field.field.ident.clone().unwrap());
            source_kinds.push(source_kind);
            source_optional.push(source_is_option);
        }
        let is_multiple = meta.relation_multiple;
        let emits_foreign_key = meta.relation_emit_foreign_key.unwrap_or(!is_multiple);
        if (meta.relation_source_condition.is_some() || meta.relation_target_condition.is_some())
            && emits_foreign_key
        {
            return Err(syn::Error::new_spanned(
                field,
                "conditional relations require `emit_fk = false` because they do not describe an unconditional physical foreign key",
            ));
        }
        let source_condition = if let Some(condition) = &meta.relation_source_condition {
            let source_field = parsed_fields
                .iter()
                .find(|parsed| {
                    parsed
                        .field
                        .ident
                        .as_ref()
                        .is_some_and(|ident| ident == condition.member.as_str())
                })
                .ok_or_else(|| {
                    syn::Error::new_spanned(
                        field,
                        format!(
                            "Relation '{}' source condition references unknown field '{}' on '{}'",
                            rust_name, condition.member, struct_name
                        ),
                    )
                })?;
            if source_field.meta.is_relation || source_field.meta.skip_db {
                return Err(syn::Error::new_spanned(
                    &source_field.field,
                    "relation source conditions require a persisted scalar field",
                ));
            }
            let (kind, optional) = classify_relation_value_type(&source_field.field.ty).ok_or_else(
                || {
                    syn::Error::new_spanned(
                        &source_field.field.ty,
                        "relation source condition fields must be String/int/float/bool (optionals allowed)",
                    )
                },
            )?;
            if !condition_value_matches_kind(&condition.value, kind) {
                return Err(syn::Error::new_spanned(
                    field,
                    "relation source condition value does not match the source field type",
                ));
            }
            Some(ResolvedSourceCondition {
                field_name: source_field.field.ident.clone().expect("named field"),
                kind,
                optional,
                value: condition.value.clone(),
            })
        } else {
            None
        };
        let on_delete =
            relation_delete_policy_tokens(meta.relation_on_delete.as_deref(), field.span())?;
        let propagate_change = relation_change_propagation_tokens(
            meta.relation_propagate_change.as_deref(),
            field.span(),
        )?;

        relations.push(RelationDef {
            field_name,
            read_policy: meta.read_policy.clone(),
            graphql_name,
            description: meta
                .description
                .clone()
                .expect("relation descriptions are resolved during parsing"),
            target_type_str: target_type,
            source_columns: from_cols,
            fk_columns: to_cols,
            is_multiple,
            source_field_idents,
            source_kinds,
            source_optional,
            emit_foreign_key: emits_foreign_key,
            on_delete,
            propagate_change,
            source_condition,
            target_condition: meta.relation_target_condition.clone(),
            associate_by_match: server_only_keys.iter().any(|server_only| *server_only),
        });
    }

    // Generate relation metadata
    let relation_metadata: Vec<_> = relations
        .iter()
        .map(|r| {
            let graphql_name = &r.graphql_name;
            let target_type = &r.target_type_str;
            let source_column = r
                .source_columns
                .first()
                .expect("relation has source column");
            let target_column = r.fk_columns.first().expect("relation has target column");
            let source_columns = r.source_columns.iter().collect::<Vec<_>>();
            let target_columns = r.fk_columns.iter().collect::<Vec<_>>();
            let is_multiple = r.is_multiple;
            let emit_foreign_key = r.emit_foreign_key;
            let on_delete = &r.on_delete;
            let propagate_change = &r.propagate_change;
            let is_readable = r.read_policy.is_none();
            let has_source_condition = r.source_condition.is_some();
            let has_target_condition = r.target_condition.is_some();
            quote! {
                ::graphql_orm::graphql::orm::RelationMetadata {
                    has_source_condition: #has_source_condition,
                    has_target_condition: #has_target_condition,
                    is_readable: #is_readable && _generated_output.contains(&#graphql_name),
                    field_name: #graphql_name,
                    target_type: #target_type,
                    source_column: #source_column,
                    target_column: #target_column,
                    source_columns: &[#(#source_columns),*],
                    target_columns: &[#(#target_columns),*],
                    is_multiple: #is_multiple,
                    emit_foreign_key: #emit_foreign_key,
                    on_delete: #on_delete,
                    propagate_change: #propagate_change,
                    search_fields: None,
                }
            }
        })
        .collect();

    // Generate ComplexObject resolver methods for relations with filtering/sorting/pagination
    //
    // Strategy:
    // - When NO filter/sort/pagination args provided: Use DataLoader for batching (N+1 free)
    // - When args ARE provided: Use direct database query (supports full SQL filtering)
    //
    // This gives optimal performance for simple relation traversal while keeping
    // full filter/sort/pagination support for complex queries.
    let relation_resolvers: Vec<_> = relations
        .iter()
        .map(|r| -> syn::Result<proc_macro2::TokenStream> {
        let field_name = &r.field_name;
        let graphql_name = &r.graphql_name;
        // The relation field's read decision is asked with its declared key
        // or `None`, as for every other generated field.
        let relation_read_check = {
            let record = if legacy_graphql_complex && entity_meta.compose_complex_object {
                quote! { &**self }
            } else {
                quote! { self }
            };
            let policy = match &r.read_policy {
                Some(policy) => quote! { Some(#policy) },
                None => quote! { None },
            };
            quote! {
                db.ensure_readable_field(ctx, stringify!(#struct_name), #graphql_name,
                    #policy, Some(#record)).await
                    .map_err(::graphql_orm::graphql::loaders::relation_authorization_error)?;
            }
        };
        let relation_loader_identity = format!("{struct_name}::{graphql_name}");
        let description = &r.description;
        let fk_columns_sql = r
            .fk_columns
            .iter()
            .map(|column| backend_render_identifier_path(backend, column))
            .collect::<Vec<_>>();
        let source_fields = &r.source_field_idents;
        let source_kinds = &r.source_kinds;
        let source_optional = &r.source_optional;
        let source_condition = source_condition_tokens(r.source_condition.as_ref(), quote! { self });
        let key_part_kind_tokens = source_kinds
            .iter()
            .copied()
            .map(relation_key_part_kind_tokens)
            .collect::<Vec<_>>();

        let source_value_bindings = source_fields
            .iter()
            .zip(source_kinds.iter())
            .zip(source_optional.iter())
            .map(|((source_field, source_kind), source_is_option)| {
                let sql_value_expr = match source_kind {
                    RelationValueKind::String => quote! { ::graphql_orm::graphql::orm::SqlValue::String(value.clone()) },
                    RelationValueKind::Uuid => quote! { ::graphql_orm::graphql::orm::SqlValue::Uuid(*value) },
                    RelationValueKind::Int => quote! { ::graphql_orm::graphql::orm::SqlValue::Int(*value as i64) },
                    RelationValueKind::Float => quote! { ::graphql_orm::graphql::orm::SqlValue::Float((*value).into()) },
                    RelationValueKind::Bool => quote! { ::graphql_orm::graphql::orm::SqlValue::Bool(*value) },
                };
                let key_part_expr = match source_kind {
                    RelationValueKind::String => quote! { value.clone() },
                    RelationValueKind::Uuid => quote! { value.to_string() },
                    _ => quote! { value.to_string() },
                };
                if *source_is_option {
                    quote! {
                        let value = self.#source_field.as_ref()?;
                        relation_sql_values.push(#sql_value_expr);
                        relation_key_parts.push(#key_part_expr);
                    }
                } else {
                    quote! {
                        let value = &self.#source_field;
                        relation_sql_values.push(#sql_value_expr);
                        relation_key_parts.push(#key_part_expr);
                    }
                }
            })
            .collect::<Vec<_>>();

        // Generate type name strings for use in fully-qualified paths
        let target_type_str = &r.target_type_str;
        let connection_type_str = format!("{}Connection", r.target_type_str);
        let edge_type_str = format!("{}Edge", r.target_type_str);

        // Create idents for local use in the macro
        let target_type = syn::Ident::new(target_type_str, struct_name.span());
        let argument_contract = GeneratedRelationshipArgumentContract::to_many(
            target_type_str,
            struct_name.span(),
        );
        let where_arg_name = argument_contract.where_name();
        let order_by_arg_name = argument_contract.order_by_name();
        let page_arg_name = argument_contract.page_name();
        let where_rust_type = argument_contract.where_rust_type();
        let order_by_rust_type = argument_contract.order_by_rust_type();
        let page_rust_type = argument_contract.page_rust_type();
        let connection_type = syn::Ident::new(&connection_type_str, struct_name.span());
        let edge_type = syn::Ident::new(&edge_type_str, struct_name.span());
        let list_relation_complexity = "{ let requested = page.as_ref().and_then(|page| page.limit).unwrap_or(::graphql_orm::graphql::orm::PaginationConfig::DEFAULT_LIMIT); let capped = requested.max(0).min(::graphql_orm::graphql::orm::page_complexity_maximum()); 2 + (capped as usize).saturating_mul(child_complexity) }";
        let single_relation_complexity = "2 + child_complexity";
        let source_condition_multiple_guard = quote! {
            if !(#source_condition) {
                let page_info = ::graphql_orm::graphql::pagination::PageInfo {
                    has_next_page: false,
                    has_previous_page: false,
                    start_cursor: None,
                    end_cursor: None,
                    total_count: Some(0),
                };
                return Ok(#connection_type { edges: Vec::new(), page_info });
            }
        };
        let source_condition_single_guard = quote! {
            if !(#source_condition) {
                return Ok(None);
            }
        };

        let (target_condition_filter, fallback_target_predicate, fallback_target_value) =
            if let Some(condition) = &r.target_condition {
                let target_column_sql =
                    backend_render_identifier_path(backend, &condition.member);
                let target_value = condition_sql_value_tokens(&condition.value);
                let placeholder_index = r.fk_columns.len() + 1;
                (
                    quote! {
                        Some::<::graphql_orm::graphql::orm::FilterExpression>(
                            ::graphql_orm::graphql::orm::FilterExpression::trusted_fragment(
                            format!("{} = ?", #target_column_sql),
                            vec![#target_value],
                        ))
                    },
                    quote! {
                        Some::<String>(format!(
                            "{} = {}",
                            #target_column_sql,
                            <#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::placeholder(#placeholder_index),
                        ))
                    },
                    quote! { Some::<::graphql_orm::graphql::orm::SqlValue>(#target_value) },
                )
            } else {
                (
                    quote! { None::<::graphql_orm::graphql::orm::FilterExpression> },
                    quote! { None::<String> },
                    quote! { None::<::graphql_orm::graphql::orm::SqlValue> },
                )
            };

        let source_binding_multiple = if source_optional.iter().any(|is_option| *is_option) {
            quote! {
                let Some((relation_loader_key, relation_sql_values)) = (|| {
                    let mut relation_sql_values = Vec::new();
                    let mut relation_key_parts = Vec::new();
                    #(#source_value_bindings)*
                    Some((
                        ::graphql_orm::graphql::loaders::RelationKey::from_projected_parts(relation_key_parts, &[#(#key_part_kind_tokens),*]),
                        relation_sql_values,
                    ))
                })() else {
                    let page_info = ::graphql_orm::graphql::pagination::PageInfo {
                        has_next_page: false,
                        has_previous_page: false,
                        start_cursor: None,
                        end_cursor: None,
                        total_count: Some(0),
                    };
                    return Ok(#connection_type { edges: Vec::new(), page_info });
                };
            }
        } else {
            quote! {
                let (relation_loader_key, relation_sql_values) = {
                    let mut relation_sql_values = Vec::new();
                    let mut relation_key_parts = Vec::new();
                    #(#source_value_bindings)*
                    (
                        ::graphql_orm::graphql::loaders::RelationKey::from_projected_parts(relation_key_parts, &[#(#key_part_kind_tokens),*]),
                        relation_sql_values,
                    )
                };
            }
        };

        let source_binding_single = if source_optional.iter().any(|is_option| *is_option) {
            quote! {
                let Some((relation_loader_key, relation_sql_values)) = (|| {
                    let mut relation_sql_values = Vec::new();
                    let mut relation_key_parts = Vec::new();
                    #(#source_value_bindings)*
                    Some((
                        ::graphql_orm::graphql::loaders::RelationKey::from_projected_parts(relation_key_parts, &[#(#key_part_kind_tokens),*]),
                        relation_sql_values,
                    ))
                })() else {
                    return Ok(None);
                };
            }
        } else {
            quote! {
                let (relation_loader_key, relation_sql_values) = {
                    let mut relation_sql_values = Vec::new();
                    let mut relation_key_parts = Vec::new();
                    #(#source_value_bindings)*
                    (
                        ::graphql_orm::graphql::loaders::RelationKey::from_projected_parts(relation_key_parts, &[#(#key_part_kind_tokens),*]),
                        relation_sql_values,
                    )
                };
            }
        };

        // Relations with a server-only key associate batched rows by the
        // database's match of the complete tuple, never by key text.
        let association = if r.associate_by_match {
            quote! { .__associate_by_database_match() }
        } else {
            quote! {}
        };
        let relation_query_key = quote! {{
            let __gom_relation_page =
                db.client_pagination_config().resolve_client_page(page.as_ref(), true).map_err(|error| error.into_graphql_error())?;
            let __gom_relation_pagination =
                if __gom_relation_page.limit.is_some() || __gom_relation_page.offset > 0 {
                    Some(__gom_relation_page.clone())
                } else {
                    None
                };
            ::graphql_orm::graphql::loaders::CompositeRelationQueryKey::__generated(
                #relation_loader_identity,
                relation_loader_key.clone(),
                relation_sql_values.clone(),
                vec![#(#fk_columns_sql),*],
                vec![#(#key_part_kind_tokens),*],
                where_input
                    .as_ref()
                    .and_then(|filter| filter.to_filter_expression().map(|expr| format!("{expr:?}"))),
                order_by
                    .as_ref()
                    .and_then(|order| order.to_sort_expression().map(|expr| expr.clause)),
                __gom_relation_pagination
                    .as_ref()
                    .map(|page| format!("limit={:?};offset={}", page.limit, page.offset)),
                {
                    let supplied = where_input
                        .as_ref()
                        .and_then(|filter| filter.to_filter_expression());
                    match (#target_condition_filter, supplied) {
                        (Some(fixed), Some(supplied)) => Some(
                            ::graphql_orm::graphql::orm::FilterExpression::And(vec![fixed, supplied]),
                        ),
                        (Some(fixed), None) => Some(fixed),
                        (None, supplied) => supplied,
                    }
                },
                order_by
                    .as_ref()
                    .and_then(|order| order.to_sort_expression())
                    .into_iter()
                    .collect(),
                __gom_relation_pagination,
                auth_context.clone(),
            )
            #association
        }};

        let single_relation_query_key = quote! {
            ::graphql_orm::graphql::loaders::CompositeRelationQueryKey::__generated(
                #relation_loader_identity,
                relation_loader_key.clone(),
                relation_sql_values.clone(),
                vec![#(#fk_columns_sql),*],
                vec![#(#key_part_kind_tokens),*],
                None,
                None,
                None,
                #target_condition_filter,
                Vec::new(),
                None,
                auth_context.clone(),
            )
            #association
        };
        let fallback_predicate_parts = fk_columns_sql
            .iter()
            .enumerate()
            .map(|(index, column)| {
                let placeholder_index = index + 1;
                quote! {
                    format!("{} = {}", #column, <#backend_marker as ::graphql_orm::graphql::orm::OrmBackend>::placeholder(#placeholder_index))
                }
            })
            .collect::<Vec<_>>();
        let fallback_relation_clause = quote! {
            {
                let mut predicates = vec![#(#fallback_predicate_parts),*];
                if let Some(predicate) = #fallback_target_predicate {
                    predicates.push(predicate);
                }
                predicates.join(" AND ")
            }
        };

        if r.is_multiple {
            // One-to-many relation with smart batching
            Ok(quote! {
                #[doc = #description]
                #[graphql(name = #graphql_name, complexity = #list_relation_complexity)]
                async fn #field_name(
                    &self,
                    ctx: &::graphql_orm::async_graphql::Context<'_>,
                    #[graphql(name = #where_arg_name)] where_input: #where_rust_type,
                    #[graphql(name = #order_by_arg_name)] order_by: #order_by_rust_type,
                    #[graphql(name = #page_arg_name)] page: #page_rust_type,
                ) -> ::graphql_orm::async_graphql::Result<#connection_type> {
                    ::graphql_orm::graphql::input_validation::require_generated_input_validation(ctx)?;
                    use ::graphql_orm::graphql::orm::{DatabaseEntity, DatabaseFilter, DatabaseOrderBy, EntityQuery, SqlValue};

                    let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?;
                    #source_condition_multiple_guard
                    let db = ::graphql_orm::db::request_database::<#backend_marker>(ctx)?;
                    ::graphql_orm::graphql::orm::ensure_rls_request_context::<#target_type, #backend_marker>(
                        ctx,
                        <#target_type as ::graphql_orm::graphql::orm::Entity>::entity_name(),
                    )?;
                    #relation_read_check
                    // Traversal enforces the target's own auth mode and
                    // operation scopes, as its root read would.
                    ::graphql_orm::graphql::orm::authorize_relation_target::<#target_type>(ctx, true)?;
                    let auth_context = ctx
                        .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                        .cloned();
                    db.ensure_entity_access(
                        Some(ctx),
                        <#target_type as ::graphql_orm::graphql::orm::Entity>::entity_name(),
                        <#target_type as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                        ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlRelation,
                    ).await.map_err(::graphql_orm::graphql::loaders::relation_authorization_error)?;
                    let visibility = db.read_visibility::<#target_type>(
                        Some(ctx), ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlRelation,
                    ).await.map_err(::graphql_orm::graphql::loaders::relation_authorization_error)?;

                    if let Some(filter) = &where_input {
                        filter
                            .validate()
                            .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                    }
                    // Relation count ordering on the target is authorized
                    // against its own target's access and SQL visibility.
                    let mut order_by = order_by;
                    if let Some(order) = order_by.as_mut() {
                        order.__gom_prepare_read(ctx, &mut ::graphql_orm::graphql::orm::RelationFilterBudget::new()).await?;
                    }
                    // Filtering and ordering reveal the target fields they
                    // name: each needs the target's field read authority.
                    ::graphql_orm::graphql::orm::authorize_read_arguments::<#target_type, #backend_marker, _, _>(
                        db, ctx, where_input.as_ref(), order_by.as_slice(),
                    ).await?;

                    // Use DataLoader whenever the relation key can be batched.
                    #source_binding_multiple

                    let use_dataloader = !visibility.requires_residual_checks()
                        && ctx.data_opt::<::graphql_orm::graphql::loaders::DirectRelationQueries>().is_none()
                        && order_by
                            .as_ref()
                            .is_none_or(|order| !order.requires_context());

                    let loaded = if use_dataloader {
                        let loader = ::graphql_orm::graphql::loaders::request_relation_loader::<#target_type, #backend_marker>(ctx)?;
                        // Loaded with its own outcome: another caller's
                        // failing group in the same batch does not fail this one.
                        ::graphql_orm::graphql::loaders::isolated_load_result(loader
                            .load_one(::graphql_orm::graphql::loaders::IsolatedLoad(::graphql_orm::graphql::loaders::SelectedRelationQueryKey::__generated(
                                ::graphql_orm::graphql::loaders::with_relation_visibility::<#target_type, #backend_marker>(#relation_query_key, &visibility)?,
                                ::graphql_orm::graphql::pagination::ConnectionSelection::from_context(ctx),
                            )))
                            .await)
                            .map_err(::graphql_orm::graphql::errors::graphql_error_from_loader)?
                            .unwrap_or(::graphql_orm::graphql::loaders::RelationLoadResult {
                                entities: Vec::new(),
                                total_count: 0,
                                has_next_page: false,
                                has_previous_page: false,
                                offset: 0,
                            })
                    } else {
                        // Slow path: Use direct query with full SQL support
                        let mut relation_sql_values = relation_sql_values.clone();
                        if let Some(value) = #fallback_target_value {
                            relation_sql_values.push(value);
                        }
                        let mut query = EntityQuery::<#target_type, #backend_marker>::new()
                            .where_values(&#fallback_relation_clause, relation_sql_values);

                        if let Some(ref filter) = where_input {
                            query = query.filter(filter);
                        }

                        if let Some(ref order) = order_by {
                            query = query.order_by_with_context(order, ctx)?;
                        }

                        if query.order_clauses.is_empty() {
                            query = query.default_order();
                        }

                        let resolved_page =
                            db.client_pagination_config().resolve_client_page(page.as_ref(), true).map_err(|error| error.into_graphql_error())?;
                        if resolved_page.limit.is_some() || resolved_page.offset > 0 {
                            query = query.paginate(&::graphql_orm::graphql::orm::PageInput {
                                limit: resolved_page.limit,
                                offset: Some(resolved_page.offset),
                            });
                        }

                        if visibility.requires_residual_checks() {
                            ::graphql_orm::graphql::loaders::scan_authorized_relation(
                                db, ctx, query, auth_context.as_ref(), &visibility, resolved_page,
                            ).await?
                        } else {
                        query = query.with_read_visibility(&visibility)?;
                        let connection = query.fetch_connection_selected_with_auth(
                            db, auth_context.as_ref(), ::graphql_orm::graphql::pagination::ConnectionSelection::from_context(ctx),
                        ).await.map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                        ::graphql_orm::graphql::loaders::RelationLoadResult {
                            has_next_page: connection.page_info.has_next_page,
                            has_previous_page: connection.page_info.has_previous_page,
                            entities: connection.edges.into_iter().map(|edge| edge.node).collect(),
                            total_count: connection.page_info.total_count.unwrap_or(0),
                            offset: resolved_page.offset.max(0),
                        }
                        }
                    };

                    ::graphql_orm::graphql::loaders::authorize_relation_fields(db, ctx, &loaded.entities, true).await?;
                    let edges: Vec<#edge_type> = loaded.entities
                        .into_iter()
                        .enumerate()
                        .map(|(i, entity)| #edge_type {
                            cursor: ::graphql_orm::graphql::pagination::encode_cursor(loaded.offset + i as i64),
                                node: entity,
                            })
                            .collect();

                    let page_info = ::graphql_orm::graphql::pagination::PageInfo {
                        has_next_page: loaded.has_next_page,
                        has_previous_page: loaded.has_previous_page,
                        start_cursor: edges.first().map(|e| e.cursor.clone()),
                        end_cursor: edges.last().map(|e| e.cursor.clone()),
                        total_count: Some(loaded.total_count),
                    };

                    Ok(#connection_type { edges, page_info })
                }
            })
        } else {
            // Single relation (many-to-one) - uses DataLoader when the source key supports it
            Ok(quote! {
                #[doc = #description]
                #[graphql(name = #graphql_name, complexity = #single_relation_complexity)]
                async fn #field_name(
                    &self,
                    ctx: &::graphql_orm::async_graphql::Context<'_>,
                ) -> ::graphql_orm::async_graphql::Result<Option<#target_type>> {
                    use ::graphql_orm::graphql::orm::{DatabaseEntity, EntityQuery, SqlValue};

                    let _auth_subject = ::graphql_orm::graphql::auth::enforce_resolver_auth(ctx, #resolver_auth_mode)?;
                    #source_condition_single_guard
                    let db = ::graphql_orm::db::request_database::<#backend_marker>(ctx)?;
                    ::graphql_orm::graphql::orm::ensure_rls_request_context::<#target_type, #backend_marker>(
                        ctx,
                        <#target_type as ::graphql_orm::graphql::orm::Entity>::entity_name(),
                    )?;
                    #relation_read_check
                    ::graphql_orm::graphql::orm::authorize_relation_target::<#target_type>(ctx, false)?;
                    let auth_context = ctx
                        .data_opt::<::graphql_orm::graphql::orm::DbAuthContext>()
                        .cloned();
                    db.ensure_entity_access(
                        Some(ctx),
                        <#target_type as ::graphql_orm::graphql::orm::Entity>::entity_name(),
                        <#target_type as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                        ::graphql_orm::graphql::orm::EntityAccessKind::Read,
                        ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlRelation,
                    ).await.map_err(::graphql_orm::graphql::loaders::relation_authorization_error)?;
                    let visibility = db.read_visibility::<#target_type>(
                        Some(ctx), ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlRelation,
                    ).await.map_err(::graphql_orm::graphql::loaders::relation_authorization_error)?;
                    #source_binding_single

                    let result = if ctx.data_opt::<::graphql_orm::graphql::loaders::DirectRelationQueries>().is_none() {
                        let loader = ::graphql_orm::graphql::loaders::request_relation_loader::<#target_type, #backend_marker>(ctx)?;
                        ::graphql_orm::graphql::loaders::isolated_load_result(loader
                            .load_one(::graphql_orm::graphql::loaders::IsolatedLoad(::graphql_orm::graphql::loaders::with_relation_visibility::<#target_type, #backend_marker>(
                                #single_relation_query_key, &visibility,
                            )?))
                            .await)
                            .map_err(::graphql_orm::graphql::errors::graphql_error_from_loader)?
                            .and_then(|mut result| result.entities.drain(..).next())
                    } else {
                        let mut relation_sql_values = relation_sql_values.clone();
                        if let Some(value) = #fallback_target_value {
                            relation_sql_values.push(value);
                        }
                        EntityQuery::<#target_type, #backend_marker>::new()
                            .where_values(&#fallback_relation_clause, relation_sql_values)
                            .with_read_visibility(&visibility)?
                            .fetch_one_with_auth(db, auth_context.as_ref())
                            .await
                            .map_err(::graphql_orm::graphql::errors::graphql_error_from_sqlx)?
                    };

                    if visibility.requires_residual_checks() {
                        if let Some(row) = result.as_ref() {
                            if !db.can_read_row(
                                Some(ctx), <#target_type as ::graphql_orm::graphql::orm::Entity>::entity_name(),
                                <#target_type as ::graphql_orm::graphql::orm::Entity>::metadata().read_policy,
                                ::graphql_orm::graphql::orm::EntityAccessSurface::GraphqlRelation, row,
                            ).await.map_err(::graphql_orm::graphql::loaders::relation_authorization_error)? {
                                return Ok(None);
                            }
                        }
                    }
                    if let Some(row) = result.as_ref() {
                        ::graphql_orm::graphql::loaders::authorize_relation_fields(db, ctx, ::std::slice::from_ref(row), false).await?;
                    }
                    Ok(result)
                }
            })
        }
    })
    .collect::<syn::Result<Vec<_>>>()?;

    let struct_name_str = struct_name.to_string();
    let has_relations = !relations.is_empty();

    let (identity_fields, identity_names) = if legacy_graphql_complex
        && !entity_owns_composed_object(legacy_graphql_complex, &entity_meta, &parsed_fields)
        && (entity_meta.node_id.is_some()
            || parsed_fields.iter().any(|field| field.meta.managed_version))
    {
        crate::entity::generate_complex_identity_fields(input)?
    } else {
        (quote! {}, Vec::new())
    };
    let identity_witness = if !identity_fields.is_empty() {
        quote! {
            impl ::graphql_orm::graphql::orm::GeneratedComplexIdentityFields for #struct_name {}
        }
    } else {
        quote! {}
    };
    let complex_object_impl = if has_relations || !identity_fields.is_empty() {
        if legacy_graphql_complex && entity_meta.compose_complex_object {
            composed_object_tokens(
                input,
                quote! {
                    #identity_fields
                    #(#relation_resolvers)*
                },
                &identity_names,
            )
        } else if legacy_graphql_complex {
            quote! {
                #[::graphql_orm::async_graphql::ComplexObject]
                impl #struct_name {
                    #identity_fields
                    #(#relation_resolvers)*
                }
            }
        } else if entity_meta.schema_only {
            // A schema-only entity has no generated object, so the relation
            // resolvers are the only fields of its GraphQL object.
            quote! {
                #[::graphql_orm::async_graphql::Object(extends)]
                impl #struct_name {
                    #(#relation_resolvers)*
                }
            }
        } else {
            // `GraphQLEntity` always emits `#[Object]` for this type, so a second
            // object implementation can only produce conflicting `OutputType`,
            // `ContainerType` and `ObjectType` implementations. Say what to
            // declare instead of letting those surface.
            return Err(syn::Error::new(
                struct_name.span(),
                format!(
                    "GraphQLRelations needs a complex object: `{struct_name}` has generated relation fields, but the generated GraphQL object of GraphQLEntity cannot also carry them. \
                     Derive `async_graphql::SimpleObject` and add `#[graphql(complex)]` (with `#[graphql(skip)]` on each relation field); \
                     GraphQLRelations then adds the relation fields, and `NodeId` and `RecordVersion` when declared, to that object. \
                     For an entity with no GraphQL object, set `schema_only = true` in `#[graphql_entity(...)]`."
                ),
            ));
        }
    } else {
        quote! {}
    };

    // Only exposed output can own implementation skips. A composed wrapper
    // publishes its fields separately; graphql_complex_object proves mounting.
    let output_witness = if !complex_object_impl.is_empty() {
        let fields = relations.iter().map(|r| &r.graphql_name);
        // A `SimpleObject` derive on the same struct may serve a getter under
        // a relation's name, for a relation field without `#[graphql(skip)]`
        // or through a rename. Which definition the schema then serves is
        // async-graphql's choice, so such a relation is not reported as a
        // mounted generated resolver.
        let getters = legacy_graphql_complex
            .then(|| simple_object_getter_names(&parsed_fields, graphql_rename_fields));
        let is_shadowed = |name: &String| match &getters {
            None => false,
            Some(None) => true,
            Some(Some(names)) => names.contains(name),
        };
        let shadowed = relations
            .iter()
            .map(|r| &r.graphql_name)
            .filter(|name| is_shadowed(name));
        let mounted = relations
            .iter()
            .map(|r| &r.graphql_name)
            .filter(|name| !is_shadowed(name));
        let exposed = if !entity_meta.compose_complex_object {
            quote! {
                #[doc(hidden)]
                pub fn __gom_exposed_relation_fields() -> &'static [&'static str] {
                    Self::__gom_generated_relation_fields()
                }

                /// Relation fields whose generated resolver this object
                /// mounts and no `SimpleObject` getter shadows. Unlike the
                /// exposure witness above, this proves mounting only, not
                /// unrestricted output eligibility.
                #[doc(hidden)]
                pub fn __gom_mounted_relation_fields() -> &'static [&'static str] {
                    &[#(#mounted),*]
                }
            }
        } else {
            quote! {}
        };
        quote! {
            #[doc(hidden)]
            pub fn __gom_generated_relation_fields() -> &'static [&'static str] {
                &[#(#fields),*]
            }

            /// Generated relation fields that a `SimpleObject` getter of the
            /// same name may shadow.
            #[doc(hidden)]
            pub fn __gom_shadowed_relation_fields() -> &'static [&'static str] {
                &[#(#shadowed),*]
            }
            #exposed
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #(#relation_target_checks)*
        #identity_witness

        impl #struct_name {
            #output_witness

            fn __gom_selection_contains(
                selection: &[::graphql_orm::async_graphql::context::SelectionField<'_>],
                target: &str,
            ) -> bool {
                selection.iter().any(|field| {
                    field.name() == target
                        || {
                            let children = field.selection_set().collect::<Vec<_>>();
                            Self::__gom_selection_contains(&children, target)
                        }
                })
            }

            fn __gom_selected_field_has_arguments(
                selection: &[::graphql_orm::async_graphql::context::SelectionField<'_>],
                target: &str,
            ) -> bool {
                selection.iter().any(|field| {
                    if field.name() == target {
                        field.arguments().map(|arguments| !arguments.is_empty()).unwrap_or(false)
                    } else {
                        let children = field.selection_set().collect::<Vec<_>>();
                        if children.is_empty() {
                            false
                        } else {
                            Self::__gom_selected_field_has_arguments(&children, target)
                        }
                    }
                })
            }

            /// Get relation metadata for look_ahead traversal
            pub fn relation_metadata() -> &'static [::graphql_orm::graphql::orm::RelationMetadata] {
                #[allow(dead_code)]
                trait OutputFallback {
                    fn __gom_exposed_relation_fields() -> &'static [&'static str] { &[] }
                }
                impl<T> OutputFallback for T {}
                static RELATIONS: ::std::sync::OnceLock<::std::boxed::Box<[::graphql_orm::graphql::orm::RelationMetadata]>> =
                    ::std::sync::OnceLock::new();
                RELATIONS.get_or_init(|| {
                    let _generated_output = #struct_name::__gom_exposed_relation_fields();
                    vec![#(#relation_metadata),*].into_boxed_slice()
                })
            }

            /// Get entity name for relation registry
            pub fn entity_name() -> &'static str {
                #struct_name_str
            }
        }

        #complex_object_impl
    })
}

/// The GraphQL names a `SimpleObject` derive on this struct serves for the
/// struct's own fields, or `None` when they cannot be known here.
///
/// A `#[graphql(flatten)]` field serves the fields of another type, which
/// this derive cannot see; a `#[graphql(derived(...))]` entry serves a
/// further getter under its own name, even beside a skipped base field; and
/// an attribute this function cannot read may rename or hide a field. Each
/// gives `None`, and the caller then treats every relation as possibly
/// shadowed.
fn simple_object_getter_names(
    fields: &[crate::entity::ParsedField],
    rename_fields: Option<&str>,
) -> Option<std::collections::BTreeSet<String>> {
    let mut names = std::collections::BTreeSet::new();
    for parsed in fields {
        let mut skipped = false;
        for attr in parsed
            .field
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("graphql"))
        {
            let options = attr
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                )
                .ok()?;
            for option in &options {
                let enabled = match option {
                    syn::Meta::Path(_) => true,
                    syn::Meta::NameValue(value) => matches!(
                        &value.value,
                        syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Bool(flag), .. }) if flag.value
                    ),
                    syn::Meta::List(_) => false,
                };
                let path = option.path();
                if path.is_ident("flatten") || path.is_ident("derived") {
                    // `flatten = false` is not accepted by async-graphql, and
                    // a derived getter is served whatever the base field is.
                    return None;
                }
                skipped |= enabled && (path.is_ident("skip") || path.is_ident("skip_output"));
            }
        }
        if skipped {
            continue;
        }
        let rust_name = parsed.field.ident.as_ref()?.to_string();
        let rust_name = rust_name.strip_prefix("r#").unwrap_or(&rust_name);
        names.insert(
            parsed
                .meta
                .graphql_name
                .clone()
                .unwrap_or_else(|| crate::naming::async_graphql_rename(rust_name, rename_fields)),
        );
    }
    Some(names)
}

#[cfg(test)]
mod object_visibility_tests {
    use super::*;

    fn getters(fields: &str, rename_fields: Option<&str>) -> Option<Vec<String>> {
        let input: DeriveInput = syn::parse_str(&format!("struct Record {{ {fields} }}")).unwrap();
        let syn::Data::Struct(data) = &input.data else {
            unreachable!()
        };
        let parsed = collect_parsed_fields(data.fields.iter()).unwrap();
        simple_object_getter_names(&parsed, rename_fields).map(|names| names.into_iter().collect())
    }

    #[test]
    fn simple_object_getter_names_follow_skips_and_renames() {
        let fields = r#"
            id: String,
            owner_id: String,
            #[graphql(skip)]
            hidden: String,
            #[graphql(skip = false)]
            shown: String,
            #[graphql(skip_output)]
            input_only: String,
            #[graphql(name = "Renamed")]
            original: String,
        "#;
        assert_eq!(
            getters(fields, None).unwrap(),
            ["Renamed", "id", "ownerId", "shown"]
        );
        assert_eq!(
            getters(fields, Some("PascalCase")).unwrap(),
            ["Id", "OwnerId", "Renamed", "Shown"]
        );
        // A flattened field serves names this derive cannot see.
        assert_eq!(
            getters("id: String, #[graphql(flatten)] more: More,", None),
            None
        );
        // A derived getter is served under its own name, also when the base
        // field is skipped.
        for derived in [
            r#"#[graphql(derived(name = "other", into = "String"))] base: String,"#,
            r#"#[graphql(skip, owned, derived(name = "other", into = "String"))] base: String,"#,
            r#"#[graphql(derived(name = "a", into = "String"), derived(name = "b", into = "String"))] base: String,"#,
        ] {
            assert_eq!(
                getters(&format!("id: String, {derived}"), None),
                None,
                "{derived}"
            );
        }
    }

    /// A relation whose name a `SimpleObject` getter also serves is not in
    /// the mount witness; with a flattened field, none is.
    #[test]
    fn shadowed_relations_are_left_out_of_the_mount_witness() {
        let witness = |fields: &str| {
            let backend = crate::backend::enabled_backends()[0].name();
            let input: DeriveInput = syn::parse_str(&format!(
                r#"
                #[graphql(complex)]
                #[graphql_entity(backend = "{backend}", table = "records", plural = "Records")]
                struct Record {{
                    #[primary_key]
                    id: String,
                    {fields}
                }}
            "#
            ))
            .unwrap();
            let generated = generate_graphql_relations(&input).unwrap().to_string();
            let start = generated
                .find("fn __gom_mounted_relation_fields")
                .expect("mount witness");
            let body = &generated[start..];
            body[..body.find('}').unwrap()].to_owned()
        };
        let relation = r#"#[relation(target = "Target", from = "id", to = "id", emit_fk = false)]"#;
        let skipped = witness(&format!(
            "#[graphql(skip)] {relation} kept: Option<Target>,"
        ));
        assert!(skipped.contains("\"kept\""), "{skipped}");

        let both = witness(&format!(
            "#[graphql(skip)] {relation} kept: Option<Target>, {relation} shadowed: Option<Target>,"
        ));
        assert!(
            both.contains("\"kept\"") && !both.contains("\"shadowed\""),
            "{both}"
        );

        let renamed = witness(&format!(
            "#[graphql(skip)] {relation} kept: Option<Target>, #[graphql(name = \"kept\")] other: String,"
        ));
        assert!(!renamed.contains("\"kept\""), "{renamed}");

        let flattened = witness(&format!(
            "#[graphql(skip)] {relation} kept: Option<Target>, #[graphql(flatten)] #[skip_db] more: More,"
        ));
        assert!(!flattened.contains("\"kept\""), "{flattened}");

        let derived = witness(&format!(
            "#[graphql(skip)] {relation} kept: Option<Target>, #[graphql(skip, owned, derived(name = \"kept\", into = \"String\"))] label: String,"
        ));
        assert!(!derived.contains("\"kept\""), "{derived}");
    }

    #[test]
    fn generated_relations_use_fallible_loader_lookups() {
        let backend = crate::backend::enabled_backends()[0].name();
        let input: DeriveInput = syn::parse_quote! {
            #[graphql(complex)]
            #[graphql_entity(backend = #backend, table = "records")]
            struct Record {
                #[primary_key]
                id: String,
                #[graphql(skip)]
                #[relation(target = "Target", from = "id", to = "id", emit_fk = false)]
                target: Option<Target>,
                #[graphql(skip)]
                #[relation(target = "Target", from = "id", to = "id", multiple, emit_fk = false)]
                targets: Vec<Target>,
            }
        };
        let generated = generate_graphql_relations(&input).unwrap().to_string();
        assert_eq!(
            generated
                .matches("loaders :: request_relation_loader")
                .count(),
            2,
        );
        assert!(!generated.contains("data_unchecked"));
    }

    #[test]
    fn input_only_relations_do_not_regenerate_skipped_getters() {
        for backend in crate::backend::enabled_backends() {
            let backend = backend.name();
            let input: DeriveInput = syn::parse_str(&format!(
                r#"
                #[graphql(complex)]
                #[graphql_entity(backend = "{backend}", table = "records", plural = "Records")]
                struct Record {{
                    #[primary_key]
                    id: String,
                    #[input_only]
                    #[graphql(skip)]
                    #[relation(target = "Target", from = "id", to = "id", emit_fk = false)]
                    hidden_relation: Option<Target>,
                }}
            "#
            ))
            .unwrap();
            let generated = generate_graphql_relations(&input).unwrap().to_string();
            assert!(!generated.contains("async fn hidden_relation"));
        }
    }
}
